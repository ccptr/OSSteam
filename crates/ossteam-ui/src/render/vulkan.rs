// SPDX-License-Identifier: LGPL-2.1-or-later
//! The Vulkan renderer (ADR 0004): Vulkan 1.2 through `ash`, one render
//! pass a frame, and a pipeline each for quads and glyphs, drawn as
//! instanced triangle strips in the scene's paint order. Glyphs come from
//! the shared glyph cache and are copied into an atlas the first time
//! they are drawn. The swapchain presents with FIFO, is rebuilt when the
//! window's size changes or it is out of date, and two frames are in
//! flight.
//!
//! Each primitive is clipped in its vertex shader, not with scissors, so
//! a frame is a handful of draws whatever its clips are.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::mem::ManuallyDrop;
use std::sync::{Arc, Weak};
use std::time::Instant;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use crate::Error;
use crate::image::{Image, ImageId};
use crate::render::FrameStats;
use crate::scene::{Primitive, Scene};
use crate::text::{GlyphCache, GlyphKey};

const FRAMES_IN_FLIGHT: usize = 2;
const ATLAS_SIZE: u32 = 1024;
const QUAD_SPV: &[u8] = include_bytes!("../../shaders/quad.spv");
const GLYPH_SPV: &[u8] = include_bytes!("../../shaders/glyph.spv");
const IMAGE_SPV: &[u8] = include_bytes!("../../shaders/image.spv");
/// How many image textures can exist at once.
const MAX_IMAGES: u32 = 4096;
const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// Bytes per quad instance: bounds, clip, background, border colour, params.
const QUAD_STRIDE: u32 = 5 * 16;
/// Bytes per glyph instance: dest, atlas slot, colour, clip.
const GLYPH_STRIDE: u32 = 4 * 16;
/// Bytes per image instance: dest, clip, params (opacity).
const IMAGE_STRIDE: u32 = 4 * 16;

fn vk_err(what: &'static str) -> impl FnOnce(vk::Result) -> Error {
    move |e| format!("{what}: {e}").into()
}

/// A buffer and the host-visible memory behind it.
struct GpuBuffer {
    buffer: vk::Buffer,
    allocation: Allocation,
    size: u64,
}

struct Frame {
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
    instances: Option<GpuBuffer>,
    staging: Option<GpuBuffer>,
}

struct Swapchain {
    handle: vk::SwapchainKHR,
    extent: vk::Extent2D,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    /// One per image: an image's semaphore is free again once the image
    /// is acquired again.
    render_finished: Vec<vk::Semaphore>,
}

struct Atlas {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
    packer: etagere::AtlasAllocator,
    /// Where each glyph is, or `None` if it has no pixels.
    slots: HashMap<GlyphKey, Option<(u32, u32)>>,
    /// Whether the image has left its initial, undefined layout.
    ready: bool,
}

/// A glyph image to copy into the atlas this frame.
struct Upload {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    offset: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Quad,
    Glyph,
    Image(ImageId),
}

impl Kind {
    fn stride(self) -> u64 {
        u64::from(match self {
            Kind::Quad => QUAD_STRIDE,
            Kind::Glyph => GLYPH_STRIDE,
            Kind::Image(_) => IMAGE_STRIDE,
        })
    }
}

/// An image's texture. The renderer holds only a weak reference to the
/// image: once the application drops it, the texture is retired.
struct GpuImage {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
    set: vk::DescriptorSet,
    source: Weak<Image>,
    width: u32,
    height: u32,
    /// Uploaded and in the layout shaders read.
    ready: bool,
}

/// An image to upload from the staging buffer this frame.
struct ImageUpload {
    id: ImageId,
    offset: u64,
}

/// A frame's batches, and the glyphs and images it must upload.
type Prepared = (Vec<Batch>, Vec<Upload>, Vec<ImageUpload>);

struct Batch {
    kind: Kind,
    offset: u64,
    count: u32,
}

pub struct VulkanRenderer {
    window: Arc<Window>,
    /// The device's name, for logs and reports.
    pub device_name: String,
    _entry: ash::Entry,
    instance: ash::Instance,
    surface_fn: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    swapchain_fn: ash::khr::swapchain::Device,
    format: vk::SurfaceFormatKHR,
    render_pass: vk::RenderPass,
    swapchain: Option<Swapchain>,
    rebuild: bool,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    quad_layout: vk::PipelineLayout,
    glyph_layout: vk::PipelineLayout,
    quad_pipeline: vk::Pipeline,
    glyph_pipeline: vk::Pipeline,
    atlas: Atlas,
    image_pipeline: vk::Pipeline,
    image_pool: vk::DescriptorPool,
    images: HashMap<ImageId, GpuImage>,
    /// Textures whose image is gone, with the frame count when they were
    /// retired: destroyed once no frame in flight can use them.
    retired: Vec<(u64, GpuImage)>,
    frames_drawn: u64,
    frames: Vec<Frame>,
    frame: usize,
    allocator: ManuallyDrop<Allocator>,
    /// Scratch space reused between frames.
    bytes: Vec<u8>,
    staged: Vec<u8>,
}

impl VulkanRenderer {
    pub fn new(window: Arc<Window>) -> Result<Self, Error> {
        // SAFETY: loading the Vulkan loader runs its initialisation; we
        // trust the system's loader as every Vulkan program does.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| format!("loading Vulkan: {e}"))?;
        let display = window
            .display_handle()
            .map_err(|e| format!("display handle: {e}"))?
            .as_raw();
        let window_handle = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?
            .as_raw();

        let mut extensions: Vec<*const c_char> = ash_window::enumerate_required_extensions(display)
            .map_err(vk_err("surface extensions"))?
            .to_vec();
        let mut flags = vk::InstanceCreateFlags::empty();
        if cfg!(target_os = "macos") {
            extensions.push(ash::khr::portability_enumeration::NAME.as_ptr());
            extensions.push(ash::khr::get_physical_device_properties2::NAME.as_ptr());
            flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
        }
        let mut layers: Vec<*const c_char> = Vec::new();
        if cfg!(debug_assertions) {
            // SAFETY: plain query of the loader.
            let available =
                unsafe { entry.enumerate_instance_layer_properties() }.unwrap_or_default();
            if available
                .iter()
                .any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER))
            {
                layers.push(VALIDATION_LAYER.as_ptr());
            }
        }
        let app = vk::ApplicationInfo::default()
            .application_name(c"ossteam")
            .engine_name(c"ossteam-ui")
            .api_version(vk::API_VERSION_1_2);
        let info = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_extension_names(&extensions)
            .enabled_layer_names(&layers)
            .flags(flags);
        // SAFETY: the create info and the arrays it points to outlive the call.
        let instance = unsafe { entry.create_instance(&info, None) }
            .map_err(vk_err("creating the instance"))?;

        // From here on, a failure must destroy what was made; `Partial`
        // does that until the renderer owns it all.
        let mut partial = Partial {
            instance: Some(instance.clone()),
            surface: None,
            device: None,
        };
        let surface_fn = ash::khr::surface::Instance::new(&entry, &instance);
        // SAFETY: the handles come from a live window that `self` keeps.
        let surface =
            unsafe { ash_window::create_surface(&entry, &instance, display, window_handle, None) }
                .map_err(vk_err("creating the surface"))?;
        partial.surface = Some((surface_fn.clone(), surface));

        let (physical, family, device_name) = pick_device(&instance, &surface_fn, surface)?;
        // SAFETY: plain queries of a valid physical device.
        let device_extensions_available =
            unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
        let mut device_extensions = vec![ash::khr::swapchain::NAME.as_ptr()];
        if device_extensions_available
            .iter()
            .any(|e| e.extension_name_as_c_str() == Ok(ash::khr::portability_subset::NAME))
        {
            device_extensions.push(ash::khr::portability_subset::NAME.as_ptr());
        }
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&device_extensions);
        // SAFETY: as for the instance.
        let device = unsafe { instance.create_device(physical, &info, None) }
            .map_err(vk_err("creating the device"))?;
        partial.device = Some(device.clone());
        // SAFETY: the family was created with one queue.
        let queue = unsafe { device.get_device_queue(family, 0) };
        let swapchain_fn = ash::khr::swapchain::Device::new(&instance, &device);

        // SAFETY: plain query.
        let formats = unsafe { surface_fn.get_physical_device_surface_formats(physical, surface) }
            .map_err(vk_err("surface formats"))?;
        // UNORM, so blending happens in sRGB space as in the software renderer.
        let format = formats
            .iter()
            .copied()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .or_else(|| {
                formats
                    .iter()
                    .copied()
                    .find(|f| f.format == vk::Format::R8G8B8A8_UNORM)
            })
            .or_else(|| formats.first().copied())
            .ok_or("the surface has no formats")?;

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device: physical,
            debug_settings: Default::default(),
            buffer_device_address: false,
            // Small blocks: the GUI needs a few MB, not gpu-allocator's
            // default of 256 MB on the device and 64 MB on the host.
            allocation_sizes: gpu_allocator::AllocationSizes::new(8 << 20, 4 << 20),
        })
        .map_err(|e| format!("creating the allocator: {e}"))?;

        let mut renderer = VulkanRenderer {
            window,
            device_name,
            _entry: entry,
            instance,
            surface_fn,
            surface,
            physical,
            device,
            queue,
            swapchain_fn,
            format,
            render_pass: vk::RenderPass::null(),
            swapchain: None,
            rebuild: true,
            descriptor_layout: vk::DescriptorSetLayout::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            quad_layout: vk::PipelineLayout::null(),
            glyph_layout: vk::PipelineLayout::null(),
            quad_pipeline: vk::Pipeline::null(),
            glyph_pipeline: vk::Pipeline::null(),
            image_pipeline: vk::Pipeline::null(),
            image_pool: vk::DescriptorPool::null(),
            images: HashMap::new(),
            retired: Vec::new(),
            frames_drawn: 0,
            atlas: Atlas {
                image: vk::Image::null(),
                view: vk::ImageView::null(),
                allocation: None,
                packer: etagere::AtlasAllocator::new(etagere::size2(
                    ATLAS_SIZE as i32,
                    ATLAS_SIZE as i32,
                )),
                slots: HashMap::new(),
                ready: false,
            },
            frames: Vec::new(),
            frame: 0,
            allocator: ManuallyDrop::new(allocator),
            bytes: Vec::new(),
            staged: Vec::new(),
        };
        // The renderer now owns everything; its Drop cleans up.
        partial.disarm();
        renderer.create_objects(family)?;
        Ok(renderer)
    }

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    /// The render pass, pipelines, atlas and per-frame objects.
    fn create_objects(&mut self, family: u32) -> Result<(), Error> {
        let d = &self.device;
        // SAFETY (whole function): every create info is fully initialised
        // and outlives its call; the handles created are destroyed in Drop.
        unsafe {
            let attachments = [vk::AttachmentDescription::default()
                .format(self.format.format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
            let color = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color)];
            // The image is written only once the acquire semaphore,
            // waited on at this stage, has been signalled.
            let dependencies = [vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
            self.render_pass = d
                .create_render_pass(
                    &vk::RenderPassCreateInfo::default()
                        .attachments(&attachments)
                        .subpasses(&subpasses)
                        .dependencies(&dependencies),
                    None,
                )
                .map_err(vk_err("creating the render pass"))?;

            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            self.descriptor_layout = d
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(vk_err("creating the descriptor set layout"))?;
            let sizes = [vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 1,
            }];
            self.descriptor_pool = d
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(vk_err("creating the descriptor pool"))?;
            let layouts = [self.descriptor_layout];
            self.descriptor_set = d
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.descriptor_pool)
                        .set_layouts(&layouts),
                )
                .map_err(vk_err("allocating the descriptor set"))?[0];

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .size(8)];
            self.quad_layout = d
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push),
                    None,
                )
                .map_err(vk_err("creating the quad pipeline layout"))?;
            self.glyph_layout = d
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(vk_err("creating the glyph pipeline layout"))?;
            self.quad_pipeline =
                self.create_pipeline(QUAD_SPV, self.quad_layout, QUAD_STRIDE, 5)?;
            self.glyph_pipeline =
                self.create_pipeline(GLYPH_SPV, self.glyph_layout, GLYPH_STRIDE, 4)?;
            // Images bind one texture each, through the glyphs' layout.
            self.image_pipeline =
                self.create_pipeline(IMAGE_SPV, self.glyph_layout, IMAGE_STRIDE, 4)?;
            let sizes = [vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: MAX_IMAGES,
            }];
            self.image_pool = self
                .device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                        .max_sets(MAX_IMAGES)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(vk_err("creating the image descriptor pool"))?;

            // The atlas.
            let d = &self.device;
            self.atlas.image = d
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D {
                            width: ATLAS_SIZE,
                            height: ATLAS_SIZE,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(vk_err("creating the atlas"))?;
            let requirements = d.get_image_memory_requirements(self.atlas.image);
            let allocation = self
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "glyph atlas",
                    requirements,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("allocating the atlas: {e}"))?;
            d.bind_image_memory(self.atlas.image, allocation.memory(), allocation.offset())
                .map_err(vk_err("binding the atlas"))?;
            self.atlas.allocation = Some(allocation);
            self.atlas.view = d
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(self.atlas.image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .subresource_range(color_range()),
                    None,
                )
                .map_err(vk_err("creating the atlas view"))?;
            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(self.atlas.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let writes = [vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info)];
            d.update_descriptor_sets(&writes, &[]);

            for _ in 0..FRAMES_IN_FLIGHT {
                let pool = d
                    .create_command_pool(
                        &vk::CommandPoolCreateInfo::default()
                            .queue_family_index(family)
                            .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                        None,
                    )
                    .map_err(vk_err("creating a command pool"))?;
                let cmd = d
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(1),
                    )
                    .map_err(vk_err("allocating a command buffer"))?[0];
                let image_available = d
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(vk_err("creating a semaphore"))?;
                let in_flight = d
                    .create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                    .map_err(vk_err("creating a fence"))?;
                self.frames.push(Frame {
                    pool,
                    cmd,
                    image_available,
                    in_flight,
                    instances: None,
                    staging: None,
                });
            }
        }
        Ok(())
    }

    /// Makes the texture for an image; its pixels are uploaded when it is
    /// first drawn.
    fn create_image(&mut self, source: &Arc<Image>) -> Result<GpuImage, Error> {
        let d = &self.device;
        let (width, height) = (source.width().max(1), source.height().max(1));
        // SAFETY: plain object creation on a live device; each object is
        // destroyed with the texture (`destroy_image`).
        unsafe {
            let image = d
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(vk_err("creating an image"))?;
            let requirements = d.get_image_memory_requirements(image);
            let allocation = match self.allocator.allocate(&AllocationCreateDesc {
                name: "image",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            }) {
                Ok(a) => a,
                Err(e) => {
                    d.destroy_image(image, None);
                    return Err(format!("allocating an image: {e}").into());
                }
            };
            let mut gpu = GpuImage {
                image,
                view: vk::ImageView::null(),
                allocation: Some(allocation),
                set: vk::DescriptorSet::null(),
                source: Arc::downgrade(source),
                width,
                height,
                ready: false,
            };
            let a = gpu.allocation.as_ref().expect("set above");
            let made = d
                .bind_image_memory(image, a.memory(), a.offset())
                .map_err(vk_err("binding an image"))
                .and_then(|()| {
                    d.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(vk::Format::R8G8B8A8_UNORM)
                            .subresource_range(color_range()),
                        None,
                    )
                    .map_err(vk_err("creating an image view"))
                })
                .and_then(|view| {
                    gpu.view = view;
                    let layouts = [self.descriptor_layout];
                    d.allocate_descriptor_sets(
                        &vk::DescriptorSetAllocateInfo::default()
                            .descriptor_pool(self.image_pool)
                            .set_layouts(&layouts),
                    )
                    .map_err(vk_err("allocating an image's descriptor set"))
                });
            match made {
                Ok(sets) => gpu.set = sets[0],
                Err(e) => {
                    self.destroy_image(gpu);
                    return Err(e);
                }
            }
            let info = [vk::DescriptorImageInfo::default()
                .image_view(gpu.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let writes = [vk::WriteDescriptorSet::default()
                .dst_set(gpu.set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&info)];
            d.update_descriptor_sets(&writes, &[]);
            Ok(gpu)
        }
    }

    /// Destroys a texture no submitted frame uses.
    fn destroy_image(&mut self, mut gpu: GpuImage) {
        // SAFETY: the caller makes sure no frame in flight uses it.
        unsafe {
            if gpu.set != vk::DescriptorSet::null() {
                let _ = self
                    .device
                    .free_descriptor_sets(self.image_pool, &[gpu.set]);
            }
            if gpu.view != vk::ImageView::null() {
                self.device.destroy_image_view(gpu.view, None);
            }
            self.device.destroy_image(gpu.image, None);
        }
        if let Some(a) = gpu.allocation.take() {
            let _ = self.allocator.free(a);
        }
    }

    /// Retires the textures of images the application has dropped and
    /// this frame did not draw.
    fn retire_unused(&mut self, scene: &Scene) {
        let gone: Vec<ImageId> = self
            .images
            .iter()
            .filter(|(id, g)| g.source.strong_count() == 0 && !scene.images.contains_key(id))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(gpu) = self.images.remove(&id) {
                self.retired.push((self.frames_drawn, gpu));
            }
        }
    }

    /// Destroys retired textures once every frame that could use them
    /// has finished: each frame waits for the one `FRAMES_IN_FLIGHT`
    /// before it.
    fn free_retired(&mut self) {
        let now = self.frames_drawn;
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retired)
            .into_iter()
            .partition(|(at, _)| now >= at + FRAMES_IN_FLIGHT as u64);
        self.retired = keep;
        for (_, gpu) in done {
            self.destroy_image(gpu);
        }
    }

    fn create_pipeline(
        &self,
        spirv: &[u8],
        layout: vk::PipelineLayout,
        stride: u32,
        attributes: u32,
    ) -> Result<vk::Pipeline, Error> {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(spirv))
            .map_err(|e| format!("reading SPIR-V: {e}"))?;
        // SAFETY: the SPIR-V is ours, compiled and validated by `cargo xtask shaders`.
        let module = unsafe {
            self.device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)
        }
        .map_err(vk_err("creating a shader module"))?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(module)
                .name(c"vertexMain"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(module)
                .name(c"fragmentMain"),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(stride)
            .input_rate(vk::VertexInputRate::INSTANCE)];
        let attrs: Vec<_> = (0..attributes)
            .map(|i| {
                vk::VertexInputAttributeDescription::default()
                    .location(i)
                    .binding(0)
                    .format(vk::Format::R32G32B32A32_SFLOAT)
                    .offset(i * 16)
            })
            .collect();
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Premultiplied alpha over.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = [vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .render_pass(self.render_pass)
            .subpass(0)];
        // SAFETY: as above; the module may go once the pipeline exists.
        let pipeline = unsafe {
            let result =
                self.device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &info, None);
            self.device.destroy_shader_module(module, None);
            result
        }
        .map_err(|(_, e)| format!("creating a pipeline: {e}"))?;
        Ok(pipeline[0])
    }

    /// Makes the swapchain for the window's current size, replacing the old one.
    fn build_swapchain(&mut self, width: u32, height: u32) -> Result<bool, Error> {
        // SAFETY (whole function): as in `create_objects`; the device is
        // idle before the old swapchain's objects go.
        unsafe {
            self.device
                .device_wait_idle()
                .map_err(vk_err("waiting for the device"))?;
            let caps = self
                .surface_fn
                .get_physical_device_surface_capabilities(self.physical, self.surface)
                .map_err(vk_err("surface capabilities"))?;
            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                    height: height
                        .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
                }
            };
            if extent.width == 0 || extent.height == 0 {
                return Ok(false);
            }
            let mut count = caps.min_image_count + 1;
            if caps.max_image_count > 0 {
                count = count.min(caps.max_image_count);
            }
            let composite = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            ]
            .into_iter()
            .find(|c| caps.supported_composite_alpha.contains(*c))
            .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
            let old = self.swapchain.take();
            let info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(count)
                .image_format(self.format.format)
                .image_color_space(self.format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(composite)
                .present_mode(vk::PresentModeKHR::FIFO)
                .clipped(true)
                .old_swapchain(old.as_ref().map_or(vk::SwapchainKHR::null(), |s| s.handle));
            let handle = self.swapchain_fn.create_swapchain(&info, None);
            if let Some(old) = old {
                self.destroy_swapchain(old);
            }
            let handle = handle.map_err(vk_err("creating the swapchain"))?;
            let images = self
                .swapchain_fn
                .get_swapchain_images(handle)
                .map_err(vk_err("swapchain images"))?;
            let mut chain = Swapchain {
                handle,
                extent,
                views: Vec::new(),
                framebuffers: Vec::new(),
                render_finished: Vec::new(),
            };
            for image in images {
                let view = self
                    .device
                    .create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(self.format.format)
                            .subresource_range(color_range()),
                        None,
                    )
                    .map_err(vk_err("creating a swapchain view"))?;
                chain.views.push(view);
                let attachments = [view];
                chain.framebuffers.push(
                    self.device
                        .create_framebuffer(
                            &vk::FramebufferCreateInfo::default()
                                .render_pass(self.render_pass)
                                .attachments(&attachments)
                                .width(extent.width)
                                .height(extent.height)
                                .layers(1),
                            None,
                        )
                        .map_err(vk_err("creating a framebuffer"))?,
                );
                chain.render_finished.push(
                    self.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                        .map_err(vk_err("creating a semaphore"))?,
                );
            }
            self.swapchain = Some(chain);
            self.rebuild = false;
            Ok(true)
        }
    }

    fn destroy_swapchain(&self, chain: Swapchain) {
        // SAFETY: called with the device idle; nothing uses these any more.
        unsafe {
            for f in chain.framebuffers {
                self.device.destroy_framebuffer(f, None);
            }
            for v in chain.views {
                self.device.destroy_image_view(v, None);
            }
            for s in chain.render_finished {
                self.device.destroy_semaphore(s, None);
            }
            self.swapchain_fn.destroy_swapchain(chain.handle, None);
        }
    }

    pub fn draw(&mut self, scene: &Scene, glyphs: &GlyphCache) -> Result<FrameStats, Error> {
        let started = Instant::now();
        let frame = self.frame;
        let fence = self.frames[frame].in_flight;
        // SAFETY: the fence is ours and was submitted with this frame's work.
        unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
            .map_err(vk_err("waiting for a frame"))?;
        let mut waited = started.elapsed();

        let size_changed = self
            .swapchain
            .as_ref()
            .is_some_and(|s| s.extent.width != scene.width || s.extent.height != scene.height);
        if (self.rebuild || size_changed || self.swapchain.is_none())
            && !self.build_swapchain(scene.width, scene.height)?
        {
            return Ok(FrameStats {
                skipped: true,
                ..Default::default()
            });
        }
        let chain = self.swapchain.as_ref().expect("built above");
        // Acquiring and presenting can block until the display takes a
        // frame (Mesa's X11 driver does), which is waiting, not work.
        let acquiring = Instant::now();
        // SAFETY: the semaphore is unsignalled: its last wait has completed
        // with the fence waited on above.
        let acquired = unsafe {
            self.swapchain_fn.acquire_next_image(
                chain.handle,
                u64::MAX,
                self.frames[frame].image_available,
                vk::Fence::null(),
            )
        };
        waited += acquiring.elapsed();
        let image = match acquired {
            Ok((image, suboptimal)) => {
                self.rebuild |= suboptimal;
                image
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.rebuild = true;
                return Ok(FrameStats {
                    skipped: true,
                    ..Default::default()
                });
            }
            Err(e) => return Err(format!("acquiring an image: {e}").into()),
        };

        self.free_retired();
        let (batches, uploads, image_uploads) = self.prepare(scene, glyphs)?;
        self.record(
            frame,
            image as usize,
            scene,
            &batches,
            &uploads,
            &image_uploads,
        )?;
        self.retire_unused(scene);

        let chain = self.swapchain.as_ref().expect("built above");
        let waits = [self.frames[frame].image_available];
        let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signals = [chain.render_finished[image as usize]];
        let cmds = [self.frames[frame].cmd];
        let submit = [vk::SubmitInfo::default()
            .wait_semaphores(&waits)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&cmds)
            .signal_semaphores(&signals)];
        // SAFETY: the command buffer is recorded and every handle is live.
        unsafe {
            self.device
                .reset_fences(&[fence])
                .map_err(vk_err("resetting a fence"))?;
            self.device
                .queue_submit(self.queue, &submit, fence)
                .map_err(vk_err("submitting"))?;
        }
        let swapchains = [chain.handle];
        let images = [image];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&signals)
            .swapchains(&swapchains)
            .image_indices(&images);
        self.window.pre_present_notify();
        let presenting = Instant::now();
        // SAFETY: as above.
        let presented = unsafe { self.swapchain_fn.queue_present(self.queue, &present) };
        waited += presenting.elapsed();
        match presented {
            Ok(suboptimal) => self.rebuild |= suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.rebuild = true,
            Err(e) => return Err(format!("presenting: {e}").into()),
        }
        self.frame = (self.frame + 1) % FRAMES_IN_FLIGHT;
        self.frames_drawn += 1;
        Ok(FrameStats {
            ms: started.elapsed().as_secs_f64() * 1000.0,
            wait_ms: waited.as_secs_f64() * 1000.0,
            pixels: 0,
            skipped: false,
        })
    }

    /// Writes the frame's instances and the glyphs it must upload.
    fn prepare(&mut self, scene: &Scene, glyphs: &GlyphCache) -> Result<Prepared, Error> {
        // A full atlas is emptied once and the frame prepared again.
        for attempt in 0..2 {
            self.bytes.clear();
            self.staged.clear();
            let mut batches: Vec<Batch> = Vec::new();
            let mut uploads = Vec::new();
            let mut image_uploads = Vec::new();
            let mut full = false;
            for item in &scene.items {
                let clip = [
                    item.clip.x,
                    item.clip.y,
                    item.clip.right(),
                    item.clip.bottom(),
                ];
                let kind = match &item.primitive {
                    Primitive::Quad(q) => {
                        let b = q.bounds;
                        push_vec4(&mut self.bytes, [b.x, b.y, b.w, b.h]);
                        push_vec4(&mut self.bytes, clip);
                        push_vec4(&mut self.bytes, q.background.to_array());
                        push_vec4(&mut self.bytes, q.border_color.to_array());
                        push_vec4(
                            &mut self.bytes,
                            [q.corner_radius, q.border_width, q.blur, 0.0],
                        );
                        Kind::Quad
                    }
                    Primitive::Image(i) => {
                        let Some(source) = scene.images.get(&i.id) else {
                            continue;
                        };
                        if !self.images.contains_key(&i.id) {
                            let gpu = self.create_image(source)?;
                            self.images.insert(i.id, gpu);
                        }
                        let gpu = &self.images[&i.id];
                        if !gpu.ready && !image_uploads.iter().any(|u: &ImageUpload| u.id == i.id) {
                            image_uploads.push(ImageUpload {
                                id: i.id,
                                offset: self.staged.len() as u64,
                            });
                            self.staged.extend_from_slice(source.pixels());
                        }
                        let b = i.bounds();
                        push_vec4(&mut self.bytes, [b.x, b.y, b.w, b.h]);
                        push_vec4(&mut self.bytes, clip);
                        push_vec4(&mut self.bytes, [i.opacity, i.corner_radius, 0.0, 0.0]);
                        let r = i.round;
                        push_vec4(&mut self.bytes, [r.x, r.y, r.w, r.h]);
                        Kind::Image(i.id)
                    }
                    Primitive::Glyph(g) => {
                        let Some(image) = glyphs.get(&g.key) else {
                            continue;
                        };
                        let slot = match self.atlas.slots.get(&g.key) {
                            Some(slot) => *slot,
                            None => {
                                let alloc = self.atlas.packer.allocate(etagere::size2(
                                    image.width as i32,
                                    image.height as i32,
                                ));
                                let Some(alloc) = alloc else {
                                    full = true;
                                    break;
                                };
                                let (x, y) =
                                    (alloc.rectangle.min.x as u32, alloc.rectangle.min.y as u32);
                                let offset = self.staged.len() as u64;
                                if image.color {
                                    self.staged.extend_from_slice(&image.data);
                                } else {
                                    self.staged.extend(
                                        image.data.iter().flat_map(|&a| [255, 255, 255, a]),
                                    );
                                }
                                uploads.push(Upload {
                                    x,
                                    y,
                                    width: image.width,
                                    height: image.height,
                                    offset,
                                });
                                self.atlas.slots.insert(g.key, Some((x, y)));
                                Some((x, y))
                            }
                        };
                        let Some((sx, sy)) = slot else { continue };
                        let b = g.bounds;
                        push_vec4(&mut self.bytes, [b.x, b.y, b.w, b.h]);
                        push_vec4(
                            &mut self.bytes,
                            [
                                sx as f32,
                                sy as f32,
                                if image.color { 1.0 } else { 0.0 },
                                0.0,
                            ],
                        );
                        push_vec4(&mut self.bytes, g.color.to_array());
                        push_vec4(&mut self.bytes, clip);
                        Kind::Glyph
                    }
                };
                let stride = kind.stride();
                match batches.last_mut() {
                    Some(b) if b.kind == kind => b.count += 1,
                    _ => batches.push(Batch {
                        kind,
                        offset: self.bytes.len() as u64 - stride,
                        count: 1,
                    }),
                }
            }
            if !full {
                return Ok((batches, uploads, image_uploads));
            }
            if attempt == 0 {
                tracing::debug!("the glyph atlas is full; emptying it");
                self.atlas.packer.clear();
                self.atlas.slots.clear();
            }
        }
        Err("the frame's glyphs do not fit in the atlas".into())
    }

    fn record(
        &mut self,
        frame: usize,
        image: usize,
        scene: &Scene,
        batches: &[Batch],
        uploads: &[Upload],
        image_uploads: &[ImageUpload],
    ) -> Result<(), Error> {
        let instances_len = self.bytes.len().max(16) as u64;
        let staged_len = self.staged.len().max(16) as u64;
        ensure_buffer(
            &self.device,
            &mut self.allocator,
            &mut self.frames[frame].instances,
            instances_len,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let staging_used = !uploads.is_empty() || !image_uploads.is_empty();
        if staging_used {
            ensure_buffer(
                &self.device,
                &mut self.allocator,
                &mut self.frames[frame].staging,
                staged_len,
                vk::BufferUsageFlags::TRANSFER_SRC,
            )?;
        }
        let f = &mut self.frames[frame];
        let instances = f.instances.as_mut().expect("ensured above");
        instances
            .allocation
            .mapped_slice_mut()
            .ok_or("instance buffer is not mapped")?[..self.bytes.len()]
            .copy_from_slice(&self.bytes);
        if staging_used {
            let staging = f.staging.as_mut().expect("ensured above");
            staging
                .allocation
                .mapped_slice_mut()
                .ok_or("staging buffer is not mapped")?[..self.staged.len()]
                .copy_from_slice(&self.staged);
        }
        let chain = self.swapchain.as_ref().expect("built before recording");
        let d = &self.device;
        let cmd = f.cmd;
        // SAFETY (whole block): the frame's fence was waited on, so its pool
        // and buffers are not in use; every handle is live.
        unsafe {
            d.reset_command_pool(f.pool, vk::CommandPoolResetFlags::empty())
                .map_err(vk_err("resetting a pool"))?;
            d.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(vk_err("beginning a command buffer"))?;

            if !uploads.is_empty() || !self.atlas.ready {
                let to_transfer = [vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(if self.atlas.ready {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    })
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .image(self.atlas.image)
                    .subresource_range(color_range())];
                d.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &to_transfer,
                );
                if let Some(staging) = &f.staging
                    && !uploads.is_empty()
                {
                    let regions: Vec<_> = uploads
                        .iter()
                        .map(|u| {
                            vk::BufferImageCopy::default()
                                .buffer_offset(u.offset)
                                .image_subresource(vk::ImageSubresourceLayers {
                                    aspect_mask: vk::ImageAspectFlags::COLOR,
                                    mip_level: 0,
                                    base_array_layer: 0,
                                    layer_count: 1,
                                })
                                .image_offset(vk::Offset3D {
                                    x: u.x as i32,
                                    y: u.y as i32,
                                    z: 0,
                                })
                                .image_extent(vk::Extent3D {
                                    width: u.width,
                                    height: u.height,
                                    depth: 1,
                                })
                        })
                        .collect();
                    d.cmd_copy_buffer_to_image(
                        cmd,
                        staging.buffer,
                        self.atlas.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &regions,
                    );
                }
                let to_read = [vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(self.atlas.image)
                    .subresource_range(color_range())];
                d.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &to_read,
                );
                self.atlas.ready = true;
            }

            if let Some(staging) = f.staging.as_ref().filter(|_| !image_uploads.is_empty()) {
                for u in image_uploads {
                    let Some(gpu) = self.images.get_mut(&u.id) else {
                        continue;
                    };
                    let barrier = |old, new, src, dst| {
                        [vk::ImageMemoryBarrier::default()
                            .src_access_mask(src)
                            .dst_access_mask(dst)
                            .old_layout(old)
                            .new_layout(new)
                            .image(gpu.image)
                            .subresource_range(color_range())]
                    };
                    d.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::TOP_OF_PIPE,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &barrier(
                            vk::ImageLayout::UNDEFINED,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::AccessFlags::empty(),
                            vk::AccessFlags::TRANSFER_WRITE,
                        ),
                    );
                    let region = [vk::BufferImageCopy::default()
                        .buffer_offset(u.offset)
                        .image_subresource(vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        })
                        .image_extent(vk::Extent3D {
                            width: gpu.width,
                            height: gpu.height,
                            depth: 1,
                        })];
                    d.cmd_copy_buffer_to_image(
                        cmd,
                        staging.buffer,
                        gpu.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &region,
                    );
                    d.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::FRAGMENT_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &barrier(
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::AccessFlags::SHADER_READ,
                        ),
                    );
                    gpu.ready = true;
                }
            }

            let bg = scene.background;
            let clear = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [bg.r, bg.g, bg.b, 1.0],
                },
            }];
            let area = vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: chain.extent,
            };
            d.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(chain.framebuffers[image])
                    .render_area(area)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            d.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: chain.extent.width as f32,
                    height: chain.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            d.cmd_set_scissor(cmd, 0, &[area]);
            let viewport = [scene.width as f32, scene.height as f32];
            let push: Vec<u8> = viewport.iter().flat_map(|v| v.to_le_bytes()).collect();
            let mut bound = None;
            let mut bound_pipeline = vk::Pipeline::null();
            for batch in batches {
                if bound != Some(batch.kind) {
                    let (pipeline, layout, set) = match batch.kind {
                        Kind::Quad => (self.quad_pipeline, self.quad_layout, None),
                        Kind::Glyph => (
                            self.glyph_pipeline,
                            self.glyph_layout,
                            Some(self.descriptor_set),
                        ),
                        Kind::Image(id) => (
                            self.image_pipeline,
                            self.glyph_layout,
                            self.images.get(&id).map(|g| g.set),
                        ),
                    };
                    if pipeline != bound_pipeline {
                        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::VERTEX, 0, &push);
                        bound_pipeline = pipeline;
                    }
                    if let Some(set) = set {
                        d.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            layout,
                            0,
                            &[set],
                            &[],
                        );
                    }
                    bound = Some(batch.kind);
                }
                d.cmd_bind_vertex_buffers(cmd, 0, &[instances.buffer], &[batch.offset]);
                d.cmd_draw(cmd, 4, batch.count, 0, 0);
            }
            d.cmd_end_render_pass(cmd);
            d.end_command_buffer(cmd)
                .map_err(vk_err("ending a command buffer"))?;
        }
        Ok(())
    }
}

impl Drop for VulkanRenderer {
    fn drop(&mut self) {
        // SAFETY: the device is idle once `device_wait_idle` returns, so
        // nothing destroyed here is in use; children go before parents.
        unsafe {
            let _ = self.device.device_wait_idle();
            if let Some(chain) = self.swapchain.take() {
                self.destroy_swapchain(chain);
            }
            for f in self.frames.drain(..) {
                for b in [f.instances, f.staging].into_iter().flatten() {
                    self.device.destroy_buffer(b.buffer, None);
                    let _ = self.allocator.free(b.allocation);
                }
                self.device.destroy_semaphore(f.image_available, None);
                self.device.destroy_fence(f.in_flight, None);
                self.device.destroy_command_pool(f.pool, None);
            }
            for gpu in std::mem::take(&mut self.images).into_values() {
                self.destroy_image(gpu);
            }
            for (_, gpu) in std::mem::take(&mut self.retired) {
                self.destroy_image(gpu);
            }
            self.device.destroy_pipeline(self.image_pipeline, None);
            self.device.destroy_descriptor_pool(self.image_pool, None);
            self.device.destroy_image_view(self.atlas.view, None);
            self.device.destroy_image(self.atlas.image, None);
            if let Some(a) = self.atlas.allocation.take() {
                let _ = self.allocator.free(a);
            }
            self.device.destroy_pipeline(self.quad_pipeline, None);
            self.device.destroy_pipeline(self.glyph_pipeline, None);
            self.device.destroy_pipeline_layout(self.quad_layout, None);
            self.device.destroy_pipeline_layout(self.glyph_layout, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
            ManuallyDrop::drop(&mut self.allocator);
            self.device.destroy_device(None);
            self.surface_fn.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

/// What `new` has made so far, destroyed if it fails before the renderer
/// owns it.
struct Partial {
    instance: Option<ash::Instance>,
    surface: Option<(ash::khr::surface::Instance, vk::SurfaceKHR)>,
    device: Option<ash::Device>,
}

impl Partial {
    fn disarm(&mut self) {
        self.instance = None;
        self.surface = None;
        self.device = None;
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        // SAFETY: nothing else holds these handles while `new` fails.
        unsafe {
            if let Some(d) = self.device.take() {
                d.destroy_device(None);
            }
            if let Some((f, s)) = self.surface.take() {
                f.destroy_surface(s, None);
            }
            if let Some(i) = self.instance.take() {
                i.destroy_instance(None);
            }
        }
    }
}

/// The first device with a queue that draws and presents to `surface`,
/// discrete GPUs first. Vulkan 1.2 is required.
fn pick_device(
    instance: &ash::Instance,
    surface_fn: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32, String), Error> {
    // SAFETY: plain queries.
    let devices =
        unsafe { instance.enumerate_physical_devices() }.map_err(vk_err("listing devices"))?;
    let mut best: Option<(u32, vk::PhysicalDevice, u32, String)> = None;
    for device in devices {
        // SAFETY: plain queries of an enumerated device.
        let props = unsafe { instance.get_physical_device_properties(device) };
        let name = props
            .device_name_as_c_str()
            .map_or_else(|_| "?".into(), |n| n.to_string_lossy().into_owned());
        if props.api_version < vk::API_VERSION_1_2 {
            tracing::debug!("{name}: Vulkan {} is below 1.2", props.api_version);
            continue;
        }
        let families = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let family = families.iter().enumerate().position(|(i, f)| {
            f.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                && unsafe {
                    surface_fn.get_physical_device_surface_support(device, i as u32, surface)
                }
                .unwrap_or(false)
        });
        let Some(family) = family else { continue };
        let rank = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 0,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
            vk::PhysicalDeviceType::CPU => 3,
            _ => 4,
        };
        if best.as_ref().is_none_or(|b| rank < b.0) {
            best = Some((rank, device, family as u32, name));
        }
    }
    best.map(|(_, d, f, n)| (d, f, n))
        .ok_or_else(|| "no Vulkan 1.2 device can draw to this window".into())
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

fn push_vec4(bytes: &mut Vec<u8>, v: [f32; 4]) {
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
}

/// Makes `slot` a mapped buffer of at least `len` bytes, growing it by
/// doubling.
fn ensure_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    slot: &mut Option<GpuBuffer>,
    len: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(), Error> {
    if slot.as_ref().is_some_and(|b| b.size >= len) {
        return Ok(());
    }
    let size = len.next_power_of_two().max(64 << 10);
    // SAFETY: the old buffer belongs to a frame whose fence was waited on.
    unsafe {
        if let Some(old) = slot.take() {
            device.destroy_buffer(old.buffer, None);
            allocator
                .free(old.allocation)
                .map_err(|e| format!("freeing a buffer: {e}"))?;
        }
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(vk_err("creating a buffer"))?;
        let requirements = device.get_buffer_memory_requirements(buffer);
        let allocation = allocator
            .allocate(&AllocationCreateDesc {
                name: "frame buffer",
                requirements,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("allocating a buffer: {e}"))?;
        device
            .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
            .map_err(vk_err("binding a buffer"))?;
        *slot = Some(GpuBuffer {
            buffer,
            allocation,
            size,
        });
    }
    Ok(())
}
