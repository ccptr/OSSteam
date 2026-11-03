// SPDX-License-Identifier: GPL-2.0-or-later
//! The ossteam-ui spike (ADR 0004 step 2). Opens a window with a header, a
//! search box and a virtual list of 10,000 rows, then prints one JSON line
//! and exits:
//!
//! ```sh
//! cargo run --release -p ossteam-ui --example spike -- --renderer vulkan
//! cargo run --release -p ossteam-ui --example spike -- --renderer software --bench scroll
//! ```
//!
//! It reports the time to the first presented frame from the start of
//! `main` (as the gpui spike in m0.md did) with its parts, the scale
//! factor and appearance the first frame was drawn with and any later
//! changes, the renderer and why it was chosen, how often the event loop
//! woke while the window was idle (GUI-5), and the resident memory when
//! idle. With `--bench scroll` the list scrolls by one row per frame to
//! its end (or `--rows N`) and the report adds each frame's work and the
//! intervals between frames, and the resident memory while scrolling.
//!
//! The search box is a text field that filters the list as you type:
//! IME composition, the clipboard (Ctrl+C, Ctrl+X, Ctrl+V), selection
//! with Shift and the mouse. The report shows what the field holds, the
//! IME events it got and the time from each input event to the frame that
//! shows it. `--bench type` types a query and deletes it again, one edit
//! per frame.
//!
//! Without `--bench`, the window waits `--settle` seconds (3 by default)
//! before it reports; Page Up, Page Down and the arrows scroll the list
//! meanwhile, and Escape clears the field or, when it is empty, reports.
//! Ctrl+O (Cmd+O on macOS) opens the folder picker; the report shows what
//! was picked and how long the dialog took to answer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ossteam_ui::appearance::{Appearance, StartupAppearance};
use ossteam_ui::clipboard::Clipboard;
use ossteam_ui::folder_picker;
use ossteam_ui::render::{Choice, Renderer};
use ossteam_ui::scene::{Color, Quad, Rect, Scene};
use ossteam_ui::text::{ShapedText, TextSystem};
use ossteam_ui::text_field::{Command, FieldStyle, TextField, command_for};
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

const ROWS: usize = 10_000;
const ROW_HEIGHT: f32 = 28.0;
const HEADER: f32 = 48.0;
const SEARCH_TOP: f32 = 64.0;
const SEARCH_HEIGHT: f32 = 36.0;
const LIST_TOP: f32 = 116.0;
const STATUS: f32 = 28.0;
/// How long the window waits for the portal's appearance before it opens.
const APPEARANCE_WAIT: Duration = Duration::from_millis(50);
/// What `--bench type` types, one character per frame, before deleting it
/// one character per frame.
const TYPE_TEXT: &str = "hollow kingdom";

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    surface: Color,
    surface_raised: Color,
    border: Color,
    text: Color,
    muted: Color,
    accent: Color,
    shadow: Color,
}

impl Palette {
    /// The GUI's palettes (ossteam-gui `theme.rs`).
    fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self {
                background: Color::rgb(0xf6f7f9),
                surface: Color::rgb(0xffffff),
                surface_raised: Color::rgb(0xeceff3),
                border: Color::rgb(0xd5d9e0),
                text: Color::rgb(0x1b1f24),
                muted: Color::rgb(0x5b6470),
                accent: Color::rgb(0x2f6fdf),
                shadow: Color::rgb(0x000000).with_alpha(0.18),
            },
            Appearance::Dark => Self {
                background: Color::rgb(0x16181c),
                surface: Color::rgb(0x1f2228),
                surface_raised: Color::rgb(0x2a2e36),
                border: Color::rgb(0x343943),
                text: Color::rgb(0xe8eaed),
                muted: Color::rgb(0x9aa3ad),
                accent: Color::rgb(0x5b8def),
                shadow: Color::rgb(0x000000).with_alpha(0.5),
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BenchKind {
    Scroll,
    Type,
}

struct Args {
    renderer: Choice,
    bench: Option<BenchKind>,
    rows: usize,
    settle: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        renderer: Choice::Auto,
        bench: None,
        rows: ROWS,
        settle: Duration::from_secs(3),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--renderer" => args.renderer = value()?.parse()?,
            "--bench" => {
                args.bench = Some(match value()?.as_str() {
                    "scroll" => BenchKind::Scroll,
                    "type" => BenchKind::Type,
                    other => return Err(format!("unknown bench {other:?}: scroll or type")),
                })
            }
            "--rows" => args.rows = value()?.parse().map_err(|e| format!("--rows: {e}"))?,
            "--settle" => {
                args.settle =
                    Duration::from_secs_f64(value()?.parse().map_err(|e| format!("--settle: {e}"))?)
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

/// Milliseconds since `start`.
/// Command on macOS, Ctrl elsewhere, as in `text_field::command_for`.
fn command_key(mods: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        mods.super_key()
    } else {
        mods.control_key()
    }
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// `(VmRSS, VmHWM)` in KiB, where the platform reports them.
fn resident_kib() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

/// Each thread's name and voluntary context switches, by thread id: how
/// often each has slept and woken (the idle-wakeups note in AGENTS).
fn thread_switches() -> HashMap<u32, (String, u64)> {
    let mut out = HashMap::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return out;
    };
    for task in tasks.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|t| t.parse().ok()) else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .map(str::trim)
        };
        let name = field("Name:").unwrap_or("?").to_owned();
        let switches = field("voluntary_ctxt_switches:")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        out.insert(tid, (name, switches));
    }
    out
}

/// A deterministic name for row `i`.
fn row_name(i: usize) -> String {
    const A: &[&str] = &[
        "Hollow", "Crimson", "Silent", "Iron", "Paper", "Lunar", "Wild", "Glass", "Last", "Tiny",
    ];
    const B: &[&str] = &[
        "Kingdom", "Harbor", "Engine", "Garden", "Signal", "Frontier", "Machine", "Orchard",
    ];
    const C: &[&str] = &[
        "",
        " II",
        ": Remastered",
        " Online",
        " – Director's Cut",
        " 3",
        ": Origins",
        "",
    ];
    let h = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!(
        "{} {}{}",
        A[(h >> 7) as usize % A.len()],
        B[(h >> 17) as usize % B.len()],
        C[(h >> 29) as usize % C.len()]
    )
}

#[derive(Default)]
struct Bench {
    /// Each frame's work: building the scene, drawing and presenting,
    /// without waiting for an earlier frame.
    work_ms: Vec<f64>,
    /// From each present to the next.
    intervals_ms: Vec<f64>,
    last_present: Option<Instant>,
    redrawn_pixels: Vec<i64>,
    /// Of each frame's work, filtering the list.
    filter_ms: Vec<f64>,
    peak_rss_kib: u64,
}

/// What other threads send the event loop.
enum UserEvent {
    /// The folder picker's answer, and how long it took.
    Picked(Result<Option<std::path::PathBuf>, String>, f64),
}

struct Spike {
    args: Args,
    started: Instant,
    appearance_query: Option<StartupAppearance>,
    fonts_thread: Option<std::thread::JoinHandle<TextSystem>>,
    text: Option<TextSystem>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    renderer_why: String,
    device: Option<String>,
    scene: Scene,
    palette: Palette,
    appearance: Appearance,
    appearance_source: &'static str,
    /// Timeline of startup, ms from `main`.
    timeline: Vec<(&'static str, f64)>,
    scale_at_creation: f64,
    first_frame: Option<serde_json::Value>,
    frames: u64,
    frame_log: Vec<serde_json::Value>,
    changes: Vec<serde_json::Value>,
    /// Scroll offset in device pixels.
    offset: i32,
    shaped: HashMap<usize, (ShapedText, ShapedText)>,
    header: Option<ShapedText>,
    /// Event-loop wakeups since the window went idle.
    idle_wakeups: u64,
    idle_since: Option<Instant>,
    idle_events: Vec<serde_json::Value>,
    deadline: Option<Instant>,
    /// Each thread's context switches when the window went idle.
    idle_threads: HashMap<u32, (String, u64)>,
    bench: Bench,
    /// The next step of `--bench type`.
    type_step: usize,
    idle_rss: Option<(u64, u64)>,
    done: bool,

    // The search field.
    field: TextField,
    field_focused: bool,
    /// The search box and the field's text area, in device pixels.
    search_rect: Rect,
    field_area: Rect,
    /// The caret last given to the IME for its candidate window.
    ime_area: Option<Rect>,
    modifiers: ModifiersState,
    pointer: (f32, f32),
    pointer_over_field: bool,
    dragging: bool,
    clipboard: Option<Clipboard>,
    clipboard_ms: Option<f64>,
    /// The field's revision the list was last filtered for.
    filtered_for: u64,
    /// The rows matching the query; `None` when it is empty.
    filtered: Option<Vec<u32>>,
    /// Every row's name in lower case, made on the first search.
    names: Vec<String>,
    /// IME events and edits, for the report.
    input_log: Vec<serde_json::Value>,
    /// When the input the next frame shows arrived.
    pending_input: Option<Instant>,
    input_latency_ms: Vec<f64>,
    proxy: EventLoopProxy<UserEvent>,
    picking: bool,
    picked: Vec<serde_json::Value>,
}

impl Spike {
    fn scale(&self) -> f32 {
        self.window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32)
    }

    fn row_height(&self) -> i32 {
        (ROW_HEIGHT * self.scale()).round() as i32
    }

    fn list_rect(&self, width: f32, height: f32) -> Rect {
        let s = self.scale();
        let top = (LIST_TOP * s).round();
        Rect::new(
            0.0,
            top,
            width,
            (height - (STATUS * s).round() - top).max(0.0),
        )
    }

    /// How many rows the list shows: all, or those matching the query.
    fn row_count(&self) -> usize {
        self.filtered.as_ref().map_or(ROWS, Vec::len)
    }

    fn max_offset(&self) -> i32 {
        let Some(window) = &self.window else { return 0 };
        let size = window.inner_size();
        let list = self.list_rect(size.width as f32, size.height as f32);
        (self.args.rows.min(self.row_count()) as i32 * self.row_height() - list.h as i32).max(0)
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Opens the folder picker; its answer comes back as a user event.
    fn pick_folder(&mut self) {
        if self.picking {
            return;
        }
        self.picking = true;
        self.log_input("folder picker opened".into());
        let proxy = self.proxy.clone();
        let pick = move || {
            let t = Instant::now();
            let answer =
                folder_picker::pick_folder("Add library folder").map_err(|e| e.to_string());
            let _ = proxy.send_event(UserEvent::Picked(answer, ms(t)));
        };
        // AppKit's panels run only on the main thread (folder_picker.rs).
        if cfg!(target_os = "macos") {
            pick();
        } else if let Err(e) = std::thread::Builder::new()
            .name("folder-picker".into())
            .spawn(pick)
        {
            self.picking = false;
            self.log_input(format!("folder picker: no thread: {e}"));
        }
    }

    fn log_input(&mut self, what: String) {
        if self.input_log.len() < 200 {
            self.input_log.push(json!([round1(ms(self.started)), what]));
        }
    }

    /// Filters the list when the query has changed since the last frame.
    fn filter(&mut self) {
        if self.field.revision() == self.filtered_for {
            return;
        }
        self.filtered_for = self.field.revision();
        let query = self.field.text().trim().to_lowercase();
        let filtered = if query.is_empty() {
            None
        } else {
            if self.names.is_empty() {
                self.names = (0..ROWS).map(|i| row_name(i).to_lowercase()).collect();
            }
            Some(
                (0..ROWS as u32)
                    .filter(|&i| self.names[i as usize].contains(&query))
                    .collect(),
            )
        };
        if filtered != self.filtered {
            self.filtered = filtered;
            self.offset = 0;
        }
    }

    fn focus_field(&mut self, focused: bool) {
        if focused != self.field_focused {
            self.field_focused = focused;
            if let Some(w) = &self.window {
                w.set_ime_allowed(focused);
            }
            self.ime_area = None;
            self.request_redraw();
        }
    }

    /// Applies an edit from the keyboard or `--bench type`.
    fn edit(&mut self, command: Command) {
        let what = format!("{command:?}");
        let changed = match &mut self.clipboard {
            Some(c) => self.field.apply(command, c),
            None => self.field.apply(command, &mut None::<String>),
        };
        if !self.args.bench.is_some() {
            self.log_input(format!(
                "{what}{}",
                if changed { "" } else { " (no change)" }
            ));
        }
        self.request_redraw();
    }

    fn build_scene(&mut self) {
        let window = self.window.clone().expect("a window");
        let size = window.inner_size();
        let (w, h) = (size.width as f32, size.height as f32);
        let s = self.scale();
        let p = self.palette;
        let text = self.text.as_mut().expect("fonts loaded");
        let renderer = self.renderer.as_ref().map_or("none", Renderer::kind);
        self.scene.clear(size.width, size.height, p.background);
        let scene = &mut self.scene;

        // Header.
        scene.quad(Quad::filled(
            Rect::new(0.0, 0.0, w, (HEADER * s).round()),
            p.surface,
        ));
        scene.quad(Quad::filled(
            Rect::new(0.0, (HEADER * s).round() - s.max(1.0), w, s.max(1.0)),
            p.border,
        ));
        let header = self.header.get_or_insert_with(|| {
            text.shape(&format!("ossteam-ui spike · {renderer} renderer"), 16.0)
        });
        text.paint(
            scene,
            header,
            (16.0 * s).round(),
            ((HEADER - header.height) / 2.0 * s).round(),
            s,
            p.text,
        );

        // Search box, with a shadow.
        let search = Rect::new(
            16.0 * s,
            SEARCH_TOP * s,
            (w - 32.0 * s).max(0.0),
            SEARCH_HEIGHT * s,
        );
        scene.quad(Quad {
            bounds: search.translate(0.0, 2.0 * s),
            background: p.shadow,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
            corner_radius: 6.0 * s,
            blur: 4.0 * s,
        });
        scene.quad(Quad {
            bounds: search,
            background: p.surface,
            border_color: if self.field_focused {
                p.accent
            } else {
                p.border
            },
            border_width: s.max(1.0),
            corner_radius: 6.0 * s,
            blur: 0.0,
        });
        let pad = (12.0 * s).round();
        let area = Rect::new(
            search.x + pad,
            search.y,
            (search.w - 2.0 * pad).max(0.0),
            search.h,
        );
        self.search_rect = search;
        self.field_area = area;
        let caret = self.field.paint(
            text,
            scene,
            area,
            s,
            &FieldStyle {
                text: p.text,
                placeholder: p.muted,
                selection: p.accent.with_alpha(0.35),
                caret: p.accent,
            },
            self.field_focused,
        );
        // Tell the IME where the caret is, for its candidate window.
        if self.field_focused && caret != self.ime_area {
            if let Some(r) = caret {
                window.set_ime_cursor_area(
                    PhysicalPosition::new(r.x, r.y),
                    PhysicalSize::new(r.w, r.h),
                );
            }
            self.ime_area = caret;
        }

        // The list.
        let list = {
            let top = (LIST_TOP * s).round();
            Rect::new(0.0, top, w, (h - (STATUS * s).round() - top).max(0.0))
        };
        let row_h = (ROW_HEIGHT * s).round() as i32;
        let offset = self.offset;
        let count = self.filtered.as_ref().map_or(ROWS, Vec::len);
        scene.push_scroll_layer(1, list, offset, p.background);
        let first = (offset / row_h.max(1)) as usize;
        let last = (((offset + list.h as i32) / row_h.max(1)) as usize + 1).min(count);
        let id_at = |pos: usize| self.filtered.as_ref().map_or(pos, |f| f[pos] as usize);
        if self.shaped.len() > 1000 {
            let keep: std::collections::HashSet<usize> = (first..last).map(id_at).collect();
            self.shaped.retain(|i, _| keep.contains(i));
        }
        for pos in first..last {
            let i = id_at(pos);
            let y = list.y + (pos as i32 * row_h - offset) as f32;
            if pos % 2 == 1 {
                scene.quad(Quad {
                    bounds: Rect::new(8.0 * s, y, w - 16.0 * s - 12.0 * s, row_h as f32),
                    background: p.surface_raised,
                    border_color: Color::TRANSPARENT,
                    border_width: 0.0,
                    corner_radius: 4.0 * s,
                    blur: 0.0,
                });
            }
            let (name, state) = self.shaped.entry(i).or_insert_with(|| {
                (
                    text.shape(&format!("{:05}   {}", i + 1, row_name(i)), 14.0),
                    text.shape(
                        if i % 3 == 0 {
                            "Installed"
                        } else {
                            "Not installed"
                        },
                        13.0,
                    ),
                )
            });
            let ty = (y + (row_h as f32 - name.height * s) / 2.0).round();
            text.paint(scene, name, (20.0 * s).round(), ty, s, p.text);
            let sy = (y + (row_h as f32 - state.height * s) / 2.0).round();
            text.paint(
                scene,
                state,
                (w - 32.0 * s - state.width * s).round(),
                sy,
                s,
                p.muted,
            );
        }
        scene.pop_scroll_layer();

        // Scrollbar thumb, over the list.
        let content = count as f32 * row_h as f32;
        if content > list.h && list.h > 0.0 {
            let thumb_h = (list.h * list.h / content).max(24.0 * s);
            let thumb_y = list.y + (list.h - thumb_h) * offset as f32 / (content - list.h);
            scene.quad(Quad {
                bounds: Rect::new(w - 10.0 * s, thumb_y, 6.0 * s, thumb_h),
                background: p.muted.with_alpha(0.6),
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                corner_radius: 3.0 * s,
                blur: 0.0,
            });
        }

        // Status bar.
        let status_top = (h - (STATUS * s).round()).max(0.0);
        scene.quad(Quad::filled(
            Rect::new(0.0, status_top, w, h - status_top),
            p.surface,
        ));
        let status = text.shape(
            &match &self.filtered {
                None => format!("row {} of {ROWS}", first + 1),
                Some(f) if f.is_empty() => format!("no row matches “{}”", self.field.text()),
                Some(f) => format!(
                    "row {} of {} matching “{}”",
                    first + 1,
                    f.len(),
                    self.field.text()
                ),
            },
            12.0,
        );
        text.paint(
            scene,
            &status,
            (16.0 * s).round(),
            (status_top + (6.0 * s)).round(),
            s,
            p.muted,
        );
        // Keep the accent in the scene: a focus-ring-coloured marker.
        scene.quad(Quad::filled(
            Rect::new(0.0, status_top, 3.0 * s, h - status_top),
            p.accent,
        ));
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.text.is_none() || self.window.is_none() {
            return;
        }
        let frame_start = Instant::now();
        self.filter();
        let filtered = frame_start.elapsed().as_secs_f64() * 1000.0;
        self.build_scene();
        let text = self.text.as_ref().expect("fonts loaded");
        let renderer = self.renderer.as_mut().expect("a renderer");
        let stats = match renderer.draw(&self.scene, &text.glyphs) {
            Ok(stats) => stats,
            Err(e) => {
                println!(
                    "{}",
                    json!({ "window": true, "error": format!("drawing: {e}") })
                );
                self.done = true;
                event_loop.exit();
                return;
            }
        };
        if stats.skipped {
            // The input changed nothing on screen (a copy, say).
            self.pending_input = None;
            return;
        }
        let now = Instant::now();
        let at = ms(self.started);
        self.frames += 1;
        if let Some(input) = self.pending_input.take() {
            let latency = (now - input).as_secs_f64() * 1000.0;
            self.input_latency_ms.push(latency);
            self.log_input(format!(
                "presented, {latency:.2} ms after the input ({:.2} ms waiting for the GPU)",
                stats.wait_ms
            ));
        }
        let scale = self.scale();
        if self.frame_log.len() < 50 {
            self.frame_log.push(json!([
                round1(at),
                format!("{:?}@{scale}", self.appearance)
            ]));
        }
        if self.first_frame.is_none() {
            self.timeline.push(("first frame presented", at));
            let window = self.window.clone().expect("a window");
            self.first_frame = Some(json!({
                "ms": round1(at),
                "scale_factor": scale,
                "scale_factor_at_window_creation": self.scale_at_creation,
                "appearance": format!("{:?}", self.appearance),
                "appearance_from": self.appearance_source,
                "winit_theme": window.theme().map(|t| format!("{t:?}")),
                "size": [self.scene.width, self.scene.height],
            }));
            self.go_idle();
            // Opened after the first frame, so it stays off the startup
            // path; on Wayland it must exist before the key press that
            // copies (clipboard.rs).
            let t = Instant::now();
            self.clipboard = Some(Clipboard::new(window.as_ref()));
            self.clipboard_ms = Some(round2(t.elapsed().as_secs_f64() * 1000.0));
        }
        if let Some(kind) = self.args.bench {
            let b = &mut self.bench;
            if let Some(last) = b.last_present {
                b.intervals_ms.push((now - last).as_secs_f64() * 1000.0);
                b.work_ms
                    .push((now - frame_start).as_secs_f64() * 1000.0 - stats.wait_ms);
                b.redrawn_pixels.push(stats.pixels);
                b.filter_ms.push(filtered);
            }
            b.last_present = Some(now);
            if let Some((rss, _)) = resident_kib() {
                b.peak_rss_kib = b.peak_rss_kib.max(rss);
            }
            let more = match kind {
                BenchKind::Scroll => {
                    let target = (self.args.rows as i32 * self.row_height()).min(self.max_offset());
                    let more = self.offset < target;
                    if more {
                        self.offset = (self.offset + self.row_height()).min(target);
                    }
                    more
                }
                BenchKind::Type => {
                    let typed = TYPE_TEXT.chars().count();
                    let step = self.type_step;
                    self.type_step += 1;
                    if step < typed {
                        let c = TYPE_TEXT.chars().nth(step).expect("in range");
                        self.edit(Command::Insert(c.to_string()));
                    } else if step < 2 * typed {
                        self.edit(Command::DeleteBack { word: false });
                    }
                    step < 2 * typed
                }
            };
            if more {
                self.request_redraw();
            } else {
                self.report(event_loop);
            }
        }
    }

    /// Records what woke or reached the loop while idle (GUI-5).
    fn idle_log(&mut self, since: Instant, what: String) {
        if self.idle_events.len() < 40 {
            let what: String = what.chars().take(120).collect();
            self.idle_events.push(json!([
                round1(since.elapsed().as_secs_f64() * 1000.0),
                what
            ]));
        }
    }

    fn go_idle(&mut self) {
        self.idle_wakeups = 0;
        self.idle_since = Some(Instant::now());
        self.idle_threads = thread_switches();
        if self.args.bench.is_none() {
            self.deadline = Some(Instant::now() + self.args.settle);
        }
    }

    fn report(&mut self, event_loop: &ActiveEventLoop) {
        if self.done {
            return;
        }
        self.done = true;
        let idle_s = self.idle_since.map_or(0.0, |t| t.elapsed().as_secs_f64());
        let rss = resident_kib();
        let mut threads: Vec<_> = thread_switches()
            .into_iter()
            .map(|(tid, (name, now))| {
                let before = self.idle_threads.get(&tid).map_or(0, |(_, n)| *n);
                json!([name, now - before.min(now)])
            })
            .collect();
        threads.sort_by_key(|t| t[0].as_str().unwrap_or("").to_owned());
        let preedit = self
            .field
            .preedit()
            .map(|p| json!({ "text": p.text, "cursor": p.cursor }));
        let selection = self.field.selection();
        let mut out = json!({
            "window": true,
            "os": std::env::consts::OS,
            "session": session_kind(),
            "renderer": self.renderer.as_ref().map(Renderer::kind),
            "renderer_why": self.renderer_why,
            "device": self.device,
            "first_frame": self.first_frame,
            "timeline": self.timeline.iter().map(|(what, at)| json!([what, round1(*at)])).collect::<Vec<_>>(),
            "fonts": self.text.as_ref().map(|t| json!({ "faces": t.face_count(), "load_ms": round1(t.load_ms) })),
            "appearance_changes": self.changes,
            "frames_drawn": self.frames,
            "frames": self.frame_log,
            "idle": {
                "seconds": round1(idle_s),
                "wakeups": self.idle_wakeups,
                "events": self.idle_events,
                "thread_switches": threads,
            },
            "field": {
                "focused": self.field_focused,
                "text": self.field.text(),
                "cursor": self.field.cursor(),
                "selection": [selection.start, selection.end],
                "preedit": preedit,
                "ime_cursor_area": self.ime_area.map(|r| [r.x, r.y, r.w, r.h]),
                "rows_matching": self.filtered.as_ref().map(Vec::len),
                "clipboard": self.clipboard.as_ref().map(Clipboard::kind),
                "clipboard_open_ms": self.clipboard_ms,
                "folder_picker": self.picked,
                "input": self.input_log,
                "input_to_present_ms": summary(&self.input_latency_ms),
            },
            "rss_kib": rss.map(|(now, peak)| json!({ "now": now, "peak": peak })),
            "idle_rss_kib": self.idle_rss.map(|(now, _)| now),
            "glyphs": self.text.as_ref().map(|t| json!({ "cached": t.glyphs.len(), "bytes": t.glyphs.bytes() })),
        });
        if let Some(kind) = self.args.bench {
            let b = &self.bench;
            let refresh_ms = self
                .window
                .as_ref()
                .and_then(|w| w.current_monitor())
                .and_then(|m| m.refresh_rate_millihertz())
                // Headless outputs report 0.
                .filter(|&mhz| mhz > 0)
                .map(|mhz| 1_000_000.0 / mhz as f64);
            let missed =
                refresh_ms.map(|r| b.intervals_ms.iter().filter(|&&i| i > 1.5 * r).count());
            out["bench"] = json!({
                "kind": match kind { BenchKind::Scroll => "scroll", BenchKind::Type => "type" },
                "rows_scrolled": self.offset / self.row_height().max(1),
                "frames": b.work_ms.len(),
                "work_ms": summary(&b.work_ms),
                "work_ms_each": (b.work_ms.len() <= 100)
                    .then(|| b.work_ms.iter().map(|&w| round2(w)).collect::<Vec<_>>()),
                "filter_ms_each": (b.filter_ms.len() <= 100)
                    .then(|| b.filter_ms.iter().map(|&w| round2(w)).collect::<Vec<_>>()),
                "interval_ms": summary(&b.intervals_ms),
                "refresh_ms": refresh_ms.map(round2),
                "missed_frames": missed,
                "frames_over_16ms_work": b.work_ms.iter().filter(|&&w| w > 16.0).count(),
                "redrawn_pixels_mean": if b.redrawn_pixels.is_empty() { 0 } else {
                    b.redrawn_pixels.iter().sum::<i64>() / b.redrawn_pixels.len() as i64
                },
                "peak_rss_kib": b.peak_rss_kib,
            });
        }
        println!("{out}");
        // Drop the renderer while the window still exists, and the
        // clipboard while winit's connection does.
        self.renderer = None;
        self.clipboard = None;
        event_loop.exit();
    }
}

fn contains(r: &Rect, (x, y): (f32, f32)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

impl Spike {
    /// Moves the field's cursor to the pointer.
    fn place_field_cursor(&mut self, extend: bool) {
        let Some(text) = self.text.as_mut() else {
            return;
        };
        let s = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32);
        let x = (self.pointer.0 - self.field_area.x) / s;
        let (before, sel) = (self.field.cursor(), self.field.selection());
        self.field.place_cursor(text, x, extend);
        if (before, sel) != (self.field.cursor(), self.field.selection()) {
            self.request_redraw();
        }
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn summary(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    json!({
        "p50": round2(at(0.5)),
        "p99": round2(at(0.99)),
        "max": round2(*v.last().expect("not empty")),
        "mean": round2(v.iter().sum::<f64>() / v.len() as f64),
    })
}

fn session_kind() -> &'static str {
    if cfg!(target_os = "linux") {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "wayland"
        } else if std::env::var_os("DISPLAY").is_some() {
            "x11"
        } else {
            "none"
        }
    } else {
        std::env::consts::OS
    }
}

impl ApplicationHandler<UserEvent> for Spike {
    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        if let Some(since) = self.idle_since {
            self.idle_log(since, "user event".into());
        }
        match event {
            UserEvent::Picked(answer, took_ms) => {
                self.picking = false;
                let answer = match answer {
                    Ok(Some(path)) => json!({ "path": path.display().to_string() }),
                    Ok(None) => json!("cancelled"),
                    Err(e) => json!({ "error": e }),
                };
                self.log_input(format!("folder picker answered {answer}"));
                self.picked
                    .push(json!({ "answer": answer, "ms": round1(took_ms) }));
                // A frame, so the idle measure starts again from here.
                self.request_redraw();
            }
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if let Some(since) = self.idle_since {
            self.idle_wakeups += 1;
            self.idle_log(since, format!("{cause:?}"));
        }
    }

    fn device_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let Some(since) = self.idle_since {
            self.idle_log(since, format!("device {event:?}"));
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("ossteam-ui spike")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 640.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                println!("{}", json!({ "window": false, "error": e.to_string() }));
                self.done = true;
                event_loop.exit();
                return;
            }
        };
        self.timeline.push(("window created", ms(self.started)));
        self.scale_at_creation = window.scale_factor();

        // The appearance: the portal's answer on Linux, else winit's.
        let portal = self
            .appearance_query
            .take()
            .and_then(|q| q.wait(APPEARANCE_WAIT));
        (self.appearance, self.appearance_source) = match (portal, window.theme()) {
            (Some(a), _) => (a, "portal"),
            (None, Some(t)) => (t.into(), "winit"),
            (None, None) => (Appearance::Light, "default"),
        };
        self.palette = Palette::for_appearance(self.appearance);
        self.timeline.push(("appearance known", ms(self.started)));

        match Renderer::new(window.clone(), self.args.renderer) {
            Ok((renderer, why)) => {
                if let Renderer::Vulkan(v) = &renderer {
                    self.device = Some(v.device_name.clone());
                }
                self.renderer = Some(renderer);
                self.renderer_why = why;
            }
            Err(e) => {
                println!(
                    "{}",
                    json!({ "window": true, "error": format!("renderer: {e}") })
                );
                self.done = true;
                event_loop.exit();
                return;
            }
        }
        self.timeline.push(("renderer created", ms(self.started)));
        if let Some(fonts) = self.fonts_thread.take() {
            self.text = Some(fonts.join().expect("the font thread"));
        }
        self.timeline.push(("fonts loaded", ms(self.started)));
        // The search field has the focus from the start.
        window.set_ime_allowed(self.field_focused);
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let Some(since) = self.idle_since {
            self.idle_log(since, format!("window {event:?}"));
        }
        match event {
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::CloseRequested => self.report(event_loop),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.changes.push(json!([
                    round1(ms(self.started)),
                    format!("scale {scale_factor}")
                ]));
                self.shaped.clear();
                self.header = None;
                self.field.reshape();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                self.changes.push(json!([
                    round1(ms(self.started)),
                    format!("winit theme {theme:?}")
                ]));
                // On Linux the portal is the source; its change signal comes
                // with the toolkit (step 3). Elsewhere winit is the source.
                if self.appearance_source != "portal" {
                    self.appearance = theme.into();
                    self.palette = Palette::for_appearance(self.appearance);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Ime(ime) => {
                self.log_input(format!("ime {ime:?}"));
                if self.field_focused && self.field.ime(&ime) {
                    self.pending_input.get_or_insert_with(Instant::now);
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if event.state == ElementState::Pressed => {
                let arrived = Instant::now();
                self.log_input(format!(
                    "key {:?} text {:?}{}",
                    event.logical_key,
                    event.text,
                    if is_synthetic { " synthetic" } else { "" }
                ));
                // While an IME composes, its keys are its own.
                let composing = self.field.composing();
                let row = self.row_height();
                let page = self.window.as_ref().map_or(0, |w| {
                    self.list_rect(0.0, w.inner_size().height as f32).h as i32 - row
                });
                let to = match event.logical_key {
                    Key::Named(NamedKey::Escape) if composing => return,
                    Key::Named(NamedKey::Escape) if self.field.text().is_empty() => {
                        return self.report(event_loop);
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.field.set_text("");
                        self.log_input("cleared".into());
                        self.pending_input.get_or_insert(arrived);
                        return self.request_redraw();
                    }
                    Key::Named(NamedKey::ArrowDown) => self.offset + row,
                    Key::Named(NamedKey::ArrowUp) => self.offset - row,
                    Key::Named(NamedKey::PageDown) => self.offset + page,
                    Key::Named(NamedKey::PageUp) => self.offset - page,
                    Key::Named(NamedKey::Home) if !self.field_focused => 0,
                    Key::Named(NamedKey::End) if !self.field_focused => self.max_offset(),
                    Key::Character(ref c) if c == "o" && command_key(self.modifiers) => {
                        return self.pick_folder();
                    }
                    _ => {
                        if self.field_focused && !composing {
                            let command = command_for(
                                &event.logical_key,
                                event.text.as_deref(),
                                self.modifiers,
                            );
                            if let Some(command) = command {
                                self.pending_input.get_or_insert(arrived);
                                self.edit(command);
                            }
                        }
                        return;
                    }
                };
                let to = to.clamp(0, self.max_offset());
                if to != self.offset {
                    self.offset = to;
                    self.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 3.0 * self.row_height() as f32,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32,
                };
                let to = (self.offset + dy.round() as i32).clamp(0, self.max_offset());
                if to != self.offset {
                    self.offset = to;
                    self.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x as f32, position.y as f32);
                let over = contains(&self.search_rect, self.pointer);
                if over != self.pointer_over_field {
                    self.pointer_over_field = over;
                    if let Some(w) = &self.window {
                        w.set_cursor(if over {
                            CursorIcon::Text
                        } else {
                            CursorIcon::Default
                        });
                    }
                }
                if self.dragging {
                    self.place_field_cursor(true);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if state == ElementState::Released {
                    self.dragging = false;
                    return;
                }
                let inside = contains(&self.search_rect, self.pointer);
                self.focus_field(inside);
                if inside {
                    self.dragging = true;
                    self.place_field_cursor(self.modifiers.shift_key());
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.done {
            return;
        }
        // An open folder picker holds the report until it answers.
        if let Some(deadline) = self.deadline.filter(|_| !self.picking) {
            if Instant::now() >= deadline {
                self.idle_rss = resident_kib();
                self.report(event_loop);
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn main() {
    let started = Instant::now();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("spike: {e}");
            std::process::exit(2);
        }
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    // Both start with the process, so they overlap opening the window.
    let appearance_query = Some(StartupAppearance::query());
    let fonts_thread = Some(TextSystem::load_in_background());
    let event_loop = match EventLoop::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            println!("{}", json!({ "window": false, "error": e.to_string() }));
            std::process::exit(1);
        }
    };
    let mut spike = Spike {
        args,
        started,
        appearance_query,
        fonts_thread,
        text: None,
        window: None,
        renderer: None,
        renderer_why: String::new(),
        device: None,
        scene: Scene::default(),
        palette: Palette::for_appearance(Appearance::Light),
        appearance: Appearance::Light,
        appearance_source: "default",
        timeline: vec![("event loop created", ms(started))],
        scale_at_creation: 0.0,
        first_frame: None,
        frames: 0,
        frame_log: Vec::new(),
        changes: Vec::new(),
        offset: 0,
        shaped: HashMap::new(),
        header: None,
        idle_wakeups: 0,
        idle_since: None,
        idle_events: Vec::new(),
        deadline: None,
        idle_threads: HashMap::new(),
        bench: Bench::default(),
        type_step: 0,
        idle_rss: None,
        done: false,
        field: TextField::new("Search the library…", 14.0),
        field_focused: true,
        search_rect: Rect::default(),
        field_area: Rect::default(),
        ime_area: None,
        modifiers: ModifiersState::empty(),
        pointer: (-1.0, -1.0),
        pointer_over_field: false,
        dragging: false,
        clipboard: None,
        clipboard_ms: None,
        filtered_for: 0,
        filtered: None,
        names: Vec::new(),
        input_log: Vec::new(),
        pending_input: None,
        input_latency_ms: Vec::new(),
        proxy: event_loop.create_proxy(),
        picking: false,
        picked: Vec::new(),
    };
    if let Err(e) = event_loop.run_app(&mut spike) {
        eprintln!("spike: event loop: {e}");
        std::process::exit(1);
    }
}
