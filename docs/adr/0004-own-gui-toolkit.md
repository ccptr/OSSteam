# ADR 0004: Our own GUI toolkit, `ossteam-ui`, replaces gpui

| | |
|---|---|
| Status | Proposed |
| Date | 2026-11-02 |
| Requirements | GUI-1 to GUI-5, CONC-3, SYNC-1, SYNC-2, WS-1, LIC-4, §20 |
| Code | `crates/ossteam-ui` (to be written), `crates/ossteam-gui` |

## Context

`ossteam-gui` is built on gpui, pinned to one zed revision (GUI-1, §5.3).
The maintainer has decided to replace it with a GUI toolkit of our own.
What gpui has cost so far, as recorded in this repository:

- **No stable release.** The API churns, so it is pinned to one zed
  revision and bumped by hand (§23). The toolchain follows that revision
  (`rust-toolchain.toml`), and the workspace repeats four of zed's own
  `[patch.crates-io]` entries, plus a proptest patch for its
  `test-support`.
- **Async.** gpui is async-native, so the GUI is the one place where
  SPEC §2.3's synchronous default does not hold (CONC-3 item 1): views
  call `cx.spawn`, and IPC events reach the UI through an async channel.
- **Behaviour we had to work around** (`docs/acceptance/m0.md`,
  `docs/spec/gui.md`): every window starts `Light` on Linux and learns
  the appearance after its first frame, which flashed a dark desktop
  white (`theme::StartupAppearance` works around it); windows open at
  scale 1 and learn the output's scale after the first commit; gpui's
  image loading keeps every image at full size until exit, 11.7 GB after
  scrolling 10,000 capsules (`thumbnails.rs` replaces it); an inactive
  window draws 30 frames a second while it scrolls (`m1e.md`).
- **Tests** of the GUI's connection and store need gpui's test platform
  and its executor clock (`ossteamd` `testing/gui.rs`), although nothing
  they check is drawn.

At the pinned revision gpui is itself built on crates we can use
directly: taffy, cosmic-text, swash and etagere are already in
`Cargo.lock` through it, and so are `ash` and `gpu-allocator`, through
its wgpu. Its platform layer (windows, input, IME, clipboard, dialogs)
is its own.

## Decision

A new workspace crate, `crates/ossteam-ui`, is ossteam's GUI toolkit.
`ossteam-gui` is ported onto it, and gpui is removed once the port
passes the checks below. The toolkit is ours from the platform layer up:
it does not wrap another GUI framework.

### What we use, and what we write

| Layer | Choice | Ours or a dependency |
|---|---|---|
| Windows, input events, IME events, DPI, event loop | `winit` | dependency |
| GPU | Vulkan through `ash`; `ash-window` for the surface, `gpu-allocator` for memory; MoltenVK on macOS | renderer, pipelines and shaders ours |
| Layout | `taffy` (flexbox), behind a builder like the subset of gpui's `div()` the GUI uses | builder ours |
| Text | `cosmic-text` for shaping, font fallback, line breaking and hit-testing; `swash` rasterises glyphs into an `etagere`-packed atlas | atlas and text field ours |
| Clipboard | `smithay-clipboard` on Wayland, on winit's connection; `arboard` elsewhere | dependency |
| Folder picker | `rfd`, its blocking API (the XDG portal on Linux) | dependency |
| Opening URLs (GUI-3) | `open` | dependency |
| Appearance | winit's theme where it reports one; on Linux the portal query in `theme.rs`, plus its change signal | ours |
| Elements, focus, key bindings, scrolling, virtual lists, images, tests | — | ours |

### How it works

- **Synchronous.** The main thread runs winit's loop with
  `ControlFlow::Wait`. Other threads wake it with an `ossteam_ui::Sender` (around
  winit's `EventLoopProxy`) that carries a message. Timers (the 150 ms
  search, the 250 ms library reads, the 8 s notices) set the loop's
  `WaitUntil`. There is no executor, no futures and no async channel in
  the GUI. A window that nothing changed does not wake and does not draw
  (GUI-5).
- **Views are functions.** The GUI's state is plain Rust: the store
  (gui.md §3) and view state. A view builds an element tree from `&state`
  whenever something changed. Event handlers are closures that get
  `&mut state` after the frame, so the store stays the only copy of
  daemon state (GUI-2) and needs no entity or context to be tested.
- **Identity.** Elements that keep state between frames (focus, scroll
  offsets, a text field's cursor and selection, an image) have an
  `ElementId`, as in gpui, so the port keeps the ids `AGENTS/gui/focus.md`
  describes.
- **Focus and keys.** Focus handles, Tab order in tree order, and key
  bindings scoped by key context, the parts of gpui's actions the GUI
  uses (`text_input.rs`, `page_scroll.rs`, `library.rs`, `root.rs`).
- **Images are explicit.** The toolkit uploads an image when asked and
  frees it when told, with no cache of its own; `thumbnails.rs` keeps
  deciding what stays (gui.md §4.2).
- **Rendering.** Vulkan 1.2 as the baseline, which MoltenVK also
  provides, with the portability extensions it needs on macOS. One render
  pass a frame, three pipelines: rounded rectangles with borders and
  shadows, glyphs from the atlas, and images. Clipping uses scissors.
  The swapchain presents with FIFO, is rebuilt on resize and when it is
  out of date or suboptimal, and two frames are in flight. Debug builds
  enable `VK_LAYER_KHRONOS_validation` when it is installed.
- **Shaders** are GLSL in `crates/ossteam-ui/shaders/`. Their SPIR-V is
  committed, generated by `cargo xtask shaders` with `naga`, and a check
  fails when it does not match the source, so building needs neither a
  shader compiler nor a C++ toolchain.
- **No Vulkan device.** When no device can be created, the GUI says so
  and exits with an error; there is no software fallback inside the
  toolkit. On Linux, Mesa's lavapipe is a Vulkan
  driver like any other, and serves headless sway tests.
- **First frame.** The window is created with the appearance and scale
  known before it is shown where the platform allows; the spike
  measures what each platform allows.

### Async that remains

Vulkan is synchronous, so the renderer has none. `ashpd`'s portal calls
are futures; each is run to completion on the thread that asked, with
`async_io::block_on` as `theme.rs` does today, the second case of
CONC-3. `rfd` does the same inside its blocking API.

### Steps

1. **Review** of this ADR and the SPEC changes made with it.
2. **Spike**: `crates/ossteam-ui/examples/spike.rs` reports what the
   gpui spike did (m0.md: first frame, scale and appearance at the first
   frame, clipboard), plus a text field with IME and a 10,000-row virtual
   list scrolled one row per frame, on Wayland and X11, and on Windows
   under Wine for what Wine can show (winevulkan on the host's driver),
   and on lavapipe under headless sway. It must meet §20 (first frame
   ≤ 500 ms; gpui's was 152 to 156 ms here) and draw no light frame on a
   dark desktop. If it cannot, this ADR is amended before the port.
3. **The toolkit**, up to what the GUI uses today and no further.
   `cargo xtask check-layering` learns WS-1's new rule for it.
4. **The port**, on a branch; `main` keeps the gpui GUI until it is
   done. The modules without gpui (`queue`, `rate`, `format`, `args`,
   the QR matrix, the store's event handling) move over unchanged; the
   store stops being an entity; `testing/gui.rs` drives it directly.
5. **Before merging**: the M1e rows already passed on Linux pass again
   (keyboard only, no daemon and Retry, the gui_states dry runs, the §20
   scroll bench). Then gpui, its four patches and the proptest patch are
   removed, and the toolchain is bumped deliberately on its own.

### Alternatives considered

- **Keep gpui.** The costs above stay; the maintainer has chosen not to.
- **Fork gpui into the workspace.** Ends the churn, but keeps the async
  model and a large codebase written for zed.
- **Another framework** (egui, iced, Slint, Xilem and the like). Not
  ours; each brings its own model, churn or runtime.
- **Our own platform layer instead of winit.** Windows, input and IME on
  Wayland, X11, Win32 and AppKit are the largest part of a toolkit and
  the least specific to ossteam. Revisit only for a measured problem.
- **wgpu.** Metal and Direct3D 12 back ends, so no MoltenVK and no
  dependence on Vulkan drivers on Windows; but a large, fast-moving layer
  whose device setup is async. The maintainer chose `ash`: we write to
  Vulkan directly and own the renderer.
- **CPU rendering** (`softbuffer`, `tiny-skia`). Simpler and free of GPU
  drivers, but a 10,000-app grid at 2× within 16 ms a frame is not shown.
  The spike times a full repaint on the CPU too; if it fits with room to
  spare, this ADR is amended to drop Vulkan.

## Consequences

- The GUI becomes synchronous, and CONC-3 loses its GUI exception.
- We own text editing and IME, focus, and drawing on three platforms.
  Everything gpui did on macOS and Windows must be checked again on real
  hardware, which the project does not have yet (status:
  hardware-mac-windows).
- The port is the size of the GUI (about 8,300 lines in 27 files today)
  plus the toolkit, and M1e's live and macOS checks wait behind it.
- The GUI needs a Vulkan driver: Mesa on Linux (`libvulkan1` is already
  in README's build packages), the GPU vendor's driver on Windows, and
  MoltenVK on macOS, which the app bundle ships (`libMoltenVK.dylib`,
  Apache-2.0) and loads with `ash::Entry::load_from`. Windows machines
  without a Vulkan driver (some virtual machines and remote sessions)
  get the error above. We own swapchain handling and synchronisation,
  which wgpu would have done, and every driver quirk is ours to meet.
- Fewer git dependencies and patches, and the toolchain no longer
  follows zed.
- winit is Apache-2.0 only, so LIC-4 still holds; it names winit instead
  of gpui.
- Accessibility through AccessKit (`accesskit_winit`) becomes possible,
  since we own the element tree. It is not needed for parity and is
  left for later.
- `README.md` (build packages, licence note) and the agent notes that
  describe gpui behaviour (`AGENTS/gui/`) change with the port, not
  before.
