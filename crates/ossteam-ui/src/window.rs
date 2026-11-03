// SPDX-License-Identifier: GPL-2.0-or-later
//! A window's contents, apart from the platform window and the renderer:
//! the application's state, the element tree its view last built, layout,
//! painting into a [`Scene`], focus, key bindings and timers.
//!
//! Nothing here talks to winit's window, so tests drive a [`Window`]
//! directly with [`Input`]s and read its scene (ADR 0004, "Headless");
//! `app.rs` drives one from winit's events.
//!
//! Each frame the view builds a tree from `&state`. `taffy` lays it out in
//! device pixels: style lengths are logical pixels times the scale, and
//! layout is rounded to whole device pixels, so edges stay sharp at any
//! scale. Handlers run between frames with `&mut state`; any handler, timer
//! or message that runs makes the next frame build the tree again.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use taffy::prelude as tf;
use winit::event::Ime;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

use crate::appearance::Appearance;
use crate::element::{Element, FocusHandle, GlobalId, Kind, Pointer, ScrollHandle, panel, text};
use crate::keys::{Action, KeyBinding, Keymap, Keystroke};
use crate::scene::{Color, Quad, Rect, Scene};
use crate::style::{Align, Axis, Justify, Length, Overflow, ResolvedText, Style, Styled};
use crate::text::{TextAttrs, TextLayout, TextSystem};
use crate::text_field::{TextClipboard, TextField, command_for};

/// How far a focused element is kept from the edge of the area that
/// scrolls to show it, in logical pixels.
const REVEAL_MARGIN: f32 = 16.0;
/// How far an overlay is kept from the window's edges, in logical pixels.
const OVERLAY_MARGIN: f32 = 8.0;
/// How long the pointer rests on an element before its tooltip shows.
const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
/// How far a wheel's line moves, in logical pixels.
pub const WHEEL_LINE: f32 = 40.0;

/// An application: its state, and the view that shows it.
pub trait App: Sized + 'static {
    /// What other threads send it through the runtime's sender.
    type Message: Send + 'static;

    /// Builds the window's contents from the state.
    fn view(&self, cx: &ViewCx) -> Element<Self>;

    /// A message from another thread.
    fn message(&mut self, message: Self::Message, cx: &mut Cx<Self>) {
        let _ = (message, cx);
    }

    /// The focused element is gone from the tree, so nothing has focus.
    fn focus_lost(&mut self, cx: &mut Cx<Self>) {
        let _ = cx;
    }

    /// The colour behind everything.
    fn background(&self, cx: &ViewCx) -> Color {
        let _ = cx;
        Color::rgb(0xffffff)
    }

    /// The window was asked to close; return `false` to keep it open.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        let _ = cx;
        true
    }

    /// What a tooltip looks like.
    fn tooltip(&self, tip: &str) -> Element<Self> {
        panel()
            .background(Color::rgb(0x202020))
            .text_color(Color::rgb(0xffffff))
            .font_size(12.0)
            .pad_xy(8.0, 4.0)
            .radius(4.0)
            .max_width(360.0)
            .child(text(tip))
    }
}

/// What a view can read about the window.
pub struct ViewCx {
    pub scale: f32,
    /// The window's size, logical pixels.
    pub width: f32,
    pub height: f32,
    pub appearance: Option<Appearance>,
    pub now: Instant,
    focused: Option<FocusHandle>,
    focus_path: Vec<FocusHandle>,
    focus_visible: bool,
}

impl ViewCx {
    pub fn focused(&self) -> Option<FocusHandle> {
        self.focused
    }

    pub fn is_focused(&self, h: FocusHandle) -> bool {
        self.focused == Some(h)
    }

    /// Whether `h` or an element inside the one that has it is focused,
    /// as of the last frame.
    pub fn contains_focused(&self, h: FocusHandle) -> bool {
        self.focus_path.contains(&h)
    }

    /// Whether focus came from the keyboard, so a focus ring should show.
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }
}

type Callback<S> = Box<dyn FnOnce(&mut S, &mut Cx<S>)>;

struct TimerEntry<S> {
    at: Instant,
    cancelled: Rc<Cell<bool>>,
    f: Callback<S>,
}

/// A timer from [`Cx::after`]. Dropping it cancels it, unless it was
/// detached.
#[must_use = "dropping a timer cancels it; call detach() to keep it"]
pub struct Timer {
    cancelled: Rc<Cell<bool>>,
    detached: bool,
}

impl Timer {
    /// Lets the timer fire although this handle is dropped.
    pub fn detach(mut self) {
        self.detached = true;
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if !self.detached {
            self.cancelled.set(true);
        }
    }
}

/// Window state handlers can change, apart from the tree.
pub(crate) struct Shared<S> {
    focus: Option<FocusHandle>,
    focus_visible: bool,
    timers: Vec<TimerEntry<S>>,
    next_frame: Vec<Callback<S>>,
    quit: bool,
    dirty: bool,
    repaint: bool,
    reveal: bool,
    propagate: bool,
    clipboard: Box<dyn TextClipboard>,
    scale: f32,
    width: u32,
    height: u32,
    appearance: Option<Appearance>,
    now: Instant,
}

/// What a handler can do besides changing the state.
pub struct Cx<'a, S> {
    shared: &'a mut Shared<S>,
    tree: &'a Tree<S>,
}

impl<S: 'static> Cx<'_, S> {
    /// Gives `h` focus, as from a press (no focus ring).
    pub fn focus(&mut self, h: FocusHandle) {
        self.shared.focus = Some(h);
        self.shared.focus_visible = false;
    }

    /// Gives `h` focus as the keyboard would: with a focus ring, and
    /// scrolled into view.
    pub fn focus_from_keyboard(&mut self, h: FocusHandle) {
        self.shared.focus = Some(h);
        self.shared.focus_visible = true;
        self.shared.reveal = true;
    }

    pub fn blur(&mut self) {
        self.shared.focus = None;
    }

    pub fn focused(&self) -> Option<FocusHandle> {
        self.shared.focus
    }

    pub fn is_focused(&self, h: FocusHandle) -> bool {
        self.shared.focus == Some(h)
    }

    /// Whether `h` or an element inside the one that has it is focused.
    pub fn contains_focused(&self, h: FocusHandle) -> bool {
        self.tree.focus_path(self.shared.focus).contains(&h)
    }

    /// Moves focus to the next tab stop, as Tab does.
    pub fn focus_next(&mut self) {
        if let Some(h) = self.tree.tab_neighbour(self.shared.focus, true) {
            self.focus_from_keyboard(h);
        }
    }

    pub fn focus_prev(&mut self) {
        if let Some(h) = self.tree.tab_neighbour(self.shared.focus, false) {
            self.focus_from_keyboard(h);
        }
    }

    /// Runs `f` once, `delay` from now.
    pub fn after(
        &mut self,
        delay: Duration,
        f: impl FnOnce(&mut S, &mut Cx<S>) + 'static,
    ) -> Timer {
        let cancelled = Rc::new(Cell::new(false));
        self.shared.timers.push(TimerEntry {
            at: self.shared.now + delay,
            cancelled: cancelled.clone(),
            f: Box::new(f),
        });
        Timer {
            cancelled,
            detached: false,
        }
    }

    /// Runs `f` after the next frame is presented.
    pub fn on_next_frame(&mut self, f: impl FnOnce(&mut S, &mut Cx<S>) + 'static) {
        self.shared.next_frame.push(Box::new(f));
        self.shared.repaint = true;
    }

    /// For an action handler: let the action go on to the next handler
    /// up the tree, as if this one had not handled it.
    pub fn propagate(&mut self) {
        self.shared.propagate = true;
    }

    pub fn quit(&mut self) {
        self.shared.quit = true;
    }

    pub fn read_clipboard(&mut self) -> Option<String> {
        self.shared.clipboard.read_text()
    }

    pub fn write_clipboard(&mut self, text: &str) {
        self.shared.clipboard.write_text(text);
    }

    pub fn scale(&self) -> f32 {
        self.shared.scale
    }

    /// The window's size, logical pixels.
    pub fn viewport(&self) -> (f32, f32) {
        (
            self.shared.width as f32 / self.shared.scale,
            self.shared.height as f32 / self.shared.scale,
        )
    }

    pub fn now(&self) -> Instant {
        self.shared.now
    }
}

/// Input, in device pixels where it has a position.
#[derive(Clone, Debug)]
pub enum Input {
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerLeft,
    /// The primary button.
    Pressed,
    Released,
    /// Logical pixels; positive scrolls the content up (towards its end).
    Wheel {
        dy: f32,
    },
    Key {
        key: Key,
        /// What the key types, if anything.
        text: Option<String>,
    },
    Modifiers(ModifiersState),
    Ime(Ime),
}

#[derive(Clone, Copy, Debug)]
enum Measure {
    Text(usize),
    Fixed(f32, f32),
}

struct Node<S> {
    el: Element<S>,
    text: ResolvedText,
    parent: Option<usize>,
    children: Vec<usize>,
    taffy: tf::NodeId,
    gid: GlobalId,
    bounds: Rect,
    content: Rect,
    clip: Rect,
    /// An overlay anchor's own child, laid out on its own.
    overlay_child: Option<usize>,
}

/// The element tree as last built and laid out.
pub(crate) struct Tree<S> {
    nodes: Vec<Node<S>>,
    taffy: tf::TaffyTree<Measure>,
    root: Option<usize>,
    /// Placed nodes in paint order, overlays last.
    order: Vec<usize>,
    tab_order: Vec<usize>,
    anchors: Vec<usize>,
}

impl<S> Default for Tree<S> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            taffy: tf::TaffyTree::new(),
            root: None,
            order: Vec::new(),
            tab_order: Vec::new(),
            anchors: Vec::new(),
        }
    }
}

impl<S> Tree<S> {
    fn node_with_focus(&self, h: FocusHandle) -> Option<usize> {
        self.order
            .iter()
            .copied()
            .find(|&i| self.nodes[i].el.focus == Some(h))
    }

    /// The focus handles from the focused element up to the root.
    fn focus_path(&self, focus: Option<FocusHandle>) -> Vec<FocusHandle> {
        let Some(i) = focus.and_then(|h| self.node_with_focus(h)) else {
            return Vec::new();
        };
        self.path(i)
            .iter()
            .filter_map(|&n| self.nodes[n].el.focus)
            .collect()
    }

    /// `i` and its ancestors, `i` first.
    fn path(&self, i: usize) -> Vec<usize> {
        let mut out = vec![i];
        let mut at = i;
        while let Some(p) = self.nodes[at].parent {
            out.push(p);
            at = p;
        }
        out
    }

    fn tab_neighbour(&self, focus: Option<FocusHandle>, forward: bool) -> Option<FocusHandle> {
        let stops: Vec<FocusHandle> = self
            .tab_order
            .iter()
            .filter_map(|&i| self.nodes[i].el.focus)
            .collect();
        if stops.is_empty() {
            return None;
        }
        let at = focus.and_then(|h| stops.iter().position(|&s| s == h));
        let n = stops.len();
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        Some(stops[next])
    }

    /// The nodes under a point, deepest first.
    fn hit(&self, x: f32, y: f32) -> Vec<usize> {
        for &i in self.order.iter().rev() {
            let n = &self.nodes[i];
            let r = n.bounds.intersect(&n.clip);
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return self.path(i);
            }
        }
        Vec::new()
    }
}

/// Shaped text kept for the frame it was used in and the one after.
#[derive(Default)]
struct TextCache {
    current: HashMap<TextKey, Rc<TextLayout>>,
    previous: HashMap<TextKey, Rc<TextLayout>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    size: u32,
    line_height: u32,
    weight: u16,
    width: Option<u32>,
}

impl TextCache {
    fn next_frame(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    fn get(
        &mut self,
        system: &mut TextSystem,
        text: &str,
        style: &ResolvedText,
        width: Option<f32>,
    ) -> Rc<TextLayout> {
        let key = TextKey {
            text: text.to_owned(),
            size: style.size.to_bits(),
            line_height: style.line_height().to_bits(),
            weight: style.weight,
            width: width.map(f32::to_bits),
        };
        if let Some(l) = self.current.get(&key) {
            return l.clone();
        }
        let laid = self.previous.remove(&key).unwrap_or_else(|| {
            let attrs = TextAttrs {
                size: style.size,
                line_height: style.line_height(),
                weight: style.weight,
            };
            Rc::new(system.layout(text, attrs, width))
        });
        self.current.insert(key, laid.clone());
        laid
    }
}

/// A window's contents.
pub struct Window<A: App> {
    pub state: A,
    pub text: TextSystem,
    keymap: Keymap,
    tree: Tree<A>,
    shared: Shared<A>,
    cache: TextCache,
    scene: Scene,
    mouse: Option<(f32, f32)>,
    modifiers: ModifiersState,
    hovered: Vec<usize>,
    pressed: HashSet<GlobalId>,
    dragging: Option<FocusHandle>,
    tooltip: Option<(GlobalId, Instant)>,
    tooltip_shown: Option<(String, f32, f32)>,
    cursor: CursorIcon,
    ime_area: Option<Rect>,
}

impl<A: App> Window<A> {
    /// `width` and `height` in device pixels.
    pub fn new(
        state: A,
        text: TextSystem,
        clipboard: Box<dyn TextClipboard>,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Self {
        Self {
            state,
            text,
            keymap: Keymap::default(),
            tree: Tree::default(),
            shared: Shared {
                focus: None,
                focus_visible: false,
                timers: Vec::new(),
                next_frame: Vec::new(),
                quit: false,
                dirty: true,
                repaint: true,
                reveal: false,
                propagate: false,
                clipboard,
                scale,
                width,
                height,
                appearance: None,
                now: Instant::now(),
            },
            cache: TextCache::default(),
            scene: Scene::new(width, height, Color::rgb(0xffffff)),
            mouse: None,
            modifiers: ModifiersState::empty(),
            hovered: Vec::new(),
            pressed: HashSet::new(),
            dragging: None,
            tooltip: None,
            tooltip_shown: None,
            cursor: CursorIcon::Default,
            ime_area: None,
        }
    }

    pub fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        self.keymap.bind(bindings);
    }

    pub fn set_appearance(&mut self, a: Option<Appearance>) {
        if self.shared.appearance != a {
            self.shared.appearance = a;
            self.shared.dirty = true;
        }
    }

    pub fn appearance(&self) -> Option<Appearance> {
        self.shared.appearance
    }

    /// The window's new size in device pixels, and scale.
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        let s = &mut self.shared;
        if (s.width, s.height, s.scale) != (width, height, scale) {
            (s.width, s.height, s.scale) = (width, height, scale);
            s.dirty = true;
        }
    }

    pub fn scale(&self) -> f32 {
        self.shared.scale
    }

    pub fn focused(&self) -> Option<FocusHandle> {
        self.shared.focus
    }

    pub fn wants_quit(&self) -> bool {
        self.shared.quit
    }

    /// The pointer shape the element under the pointer asks for.
    pub fn cursor(&self) -> CursorIcon {
        self.cursor
    }

    /// Where the IME's candidate window should go (the caret of the focused
    /// text input, device pixels), or `None` with no input focused.
    pub fn ime_area(&self) -> Option<Rect> {
        self.ime_area
    }

    /// The bounds of the first element with id `id` in the last frame, in
    /// logical pixels: for tests that press things.
    pub fn bounds_of(&self, id: impl Into<crate::element::ElementId>) -> Option<Rect> {
        let id = Some(id.into());
        let scale = self.shared.scale;
        self.tree.order.iter().find_map(|&i| {
            let n = &self.tree.nodes[i];
            (n.el.id == id).then(|| {
                let b = n.bounds;
                Rect::new(b.x / scale, b.y / scale, b.w / scale, b.h / scale)
            })
        })
    }

    /// Moves the pointer to the centre of the element with id `id`,
    /// presses and releases it. Returns `false` if there is no such
    /// element.
    pub fn click(&mut self, id: impl Into<crate::element::ElementId>) -> bool {
        let Some(b) = self.bounds_of(id) else {
            return false;
        };
        let s = self.shared.scale;
        self.input(Input::PointerMoved {
            x: (b.x + b.w / 2.0) * s,
            y: (b.y + b.h / 2.0) * s,
        });
        self.input(Input::Pressed);
        self.input(Input::Released);
        true
    }

    /// Whether a frame needs building or painting.
    pub fn needs_frame(&self) -> bool {
        self.shared.dirty || self.shared.repaint
    }

    /// When the next timer is due.
    pub fn next_deadline(&self) -> Option<Instant> {
        let timers = self
            .shared
            .timers
            .iter()
            .filter(|t| !t.cancelled.get())
            .map(|t| t.at);
        timers.chain(self.tooltip.map(|t| t.1)).min()
    }

    /// Sets the clock and runs the timers that are due.
    pub fn advance(&mut self, now: Instant) {
        self.shared.now = now;
        self.shared.timers.retain(|t| !t.cancelled.get());
        while let Some(i) = self.shared.timers.iter().position(|t| t.at <= now) {
            let t = self.shared.timers.swap_remove(i);
            if t.cancelled.get() {
                continue;
            }
            self.run(|s, cx| (t.f)(s, cx));
        }
        if let Some((gid, at)) = self.tooltip
            && at <= now
        {
            self.tooltip = None;
            let tip = self.hovered.iter().find_map(|&i| {
                let n = &self.tree.nodes[i];
                n.el.tooltip.clone().filter(|_| n.gid == gid)
            });
            if let (Some(tip), Some((x, y))) = (tip, self.mouse) {
                self.tooltip_shown = Some((tip, x, y));
                self.shared.dirty = true;
            }
        }
    }

    /// A message from another thread.
    pub fn message(&mut self, m: A::Message) {
        self.run(|s, cx| s.message(m, cx));
    }

    /// The window was asked to close; whether it should.
    pub fn close_requested(&mut self) -> bool {
        let mut close = true;
        self.run(|s, cx| close = s.close_requested(cx));
        close
    }

    /// Runs the callbacks waiting for a frame to be presented.
    pub fn presented(&mut self) {
        for f in std::mem::take(&mut self.shared.next_frame) {
            self.run(f);
        }
    }

    /// Calls `f` with the state and a context, then deals with what it
    /// changed.
    fn run(&mut self, f: impl FnOnce(&mut A, &mut Cx<A>)) {
        let before = self.shared.focus;
        let mut cx = Cx {
            shared: &mut self.shared,
            tree: &self.tree,
        };
        f(&mut self.state, &mut cx);
        self.shared.dirty = true;
        self.focus_changed(before);
    }

    /// Calls `on_focus_out` on the elements focus left.
    fn focus_changed(&mut self, before: Option<FocusHandle>) {
        let now = self.shared.focus;
        if before == now {
            return;
        }
        let Some(old) = before.and_then(|h| self.tree.node_with_focus(h)) else {
            return;
        };
        let new_path: HashSet<usize> = now
            .and_then(|h| self.tree.node_with_focus(h))
            .map(|i| self.tree.path(i).into_iter().collect())
            .unwrap_or_default();
        for i in self.tree.path(old) {
            if new_path.contains(&i) {
                break;
            }
            let h = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .and_then(|h| h.focus_out.clone());
            if let Some(h) = h {
                let mut cx = Cx {
                    shared: &mut self.shared,
                    tree: &self.tree,
                };
                h(&mut self.state, &mut cx);
            }
        }
    }

    /// Builds and paints a frame if anything changed. Returns the scene
    /// to draw, or `None` when nothing needs drawing.
    pub fn frame(&mut self) -> Option<&Scene> {
        for _ in 0..4 {
            if !self.shared.dirty {
                break;
            }
            self.shared.dirty = false;
            self.shared.repaint = true;
            self.build();
            let focus = self.shared.focus;
            if focus.is_some_and(|h| self.tree.node_with_focus(h).is_none()) {
                self.shared.focus = None;
                self.run(|s, cx| s.focus_lost(cx));
                continue;
            }
            if self.shared.reveal {
                self.shared.reveal = false;
                if self.reveal_focused() {
                    self.shared.dirty = true;
                }
            }
        }
        if !self.shared.repaint {
            return None;
        }
        self.shared.repaint = false;
        self.update_hover();
        self.paint();
        Some(&self.scene)
    }

    /// The scene as last painted.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    fn view_cx(&self) -> ViewCx {
        let s = &self.shared;
        ViewCx {
            scale: s.scale,
            width: s.width as f32 / s.scale,
            height: s.height as f32 / s.scale,
            appearance: s.appearance,
            now: s.now,
            focused: s.focus,
            focus_path: self.tree.focus_path(s.focus),
            focus_visible: s.focus_visible,
        }
    }

    fn build(&mut self) {
        let vcx = self.view_cx();
        let root_el = self.state.view(&vcx);
        let scale = self.shared.scale;
        let (w, h) = (self.shared.width as f32, self.shared.height as f32);
        self.cache.next_frame();
        let mut tree = Tree::default();
        let root = panel()
            .size(Length::Fraction(1.0), Length::Fraction(1.0))
            .child(root_el);
        let r = insert(
            &mut tree,
            root,
            None,
            GlobalId::ROOT,
            ResolvedText::default(),
            scale,
        );
        tree.root = Some(r);
        let full = Rect::new(0.0, 0.0, w, h);
        let mut lay = Layout {
            text: &mut self.text,
            cache: &mut self.cache,
            scale,
            state: &self.state,
            vcx: &vcx,
        };
        lay.compute(&mut tree, r, Some(w), Some(h));
        lay.place(&mut tree, r, 0.0, 0.0, full);
        // Overlays, then the tooltip, over everything.
        let mut a = 0;
        while a < tree.anchors.len() {
            let anchor = tree.anchors[a];
            a += 1;
            let Some(child) = tree.nodes[anchor].overlay_child else {
                continue;
            };
            let at = tree.nodes[anchor].bounds;
            lay.place_overlay(&mut tree, child, at.x, at.y, full);
        }
        if let Some((tip, x, y)) = self.tooltip_shown.clone() {
            let el = self.state.tooltip(&tip);
            let t = insert(
                &mut tree,
                el,
                None,
                GlobalId::ROOT.child("tooltip".into()),
                ResolvedText::default(),
                scale,
            );
            lay.place_overlay(&mut tree, t, x, y + 20.0 * scale, full);
        }
        self.tree = tree;
    }

    /// Scrolls the areas around the focused element to show it. Returns
    /// whether anything moved.
    fn reveal_focused(&mut self) -> bool {
        let Some(i) = self.shared.focus.and_then(|h| self.tree.node_with_focus(h)) else {
            return false;
        };
        let scale = self.shared.scale;
        let target = self.tree.nodes[i].bounds;
        let mut moved = false;
        for p in self.tree.path(i).into_iter().skip(1) {
            let n = &self.tree.nodes[p];
            let Some(scroll) = &n.el.scroll else {
                continue;
            };
            if matches!(n.el.kind, Kind::List { .. }) {
                continue;
            }
            let view = n.content;
            let m = REVEAL_MARGIN * scale;
            let (top, bottom) = (target.y - m, target.bottom() + m);
            let down = if top < view.y {
                top - view.y
            } else if bottom > view.bottom() {
                (bottom - view.bottom()).min(top - view.y)
            } else {
                0.0
            };
            if down != 0.0 {
                let before = scroll.offset();
                let max = scroll.max_offset();
                scroll.set_offset((before + down / scale).clamp(0.0, max));
                moved |= scroll.offset() != before;
            }
        }
        moved
    }

    fn update_hover(&mut self) {
        let hovered = match self.mouse {
            Some((x, y)) => self.tree.hit(x, y),
            None => Vec::new(),
        };
        self.cursor = hovered
            .iter()
            .find_map(|&i| self.tree.nodes[i].el.style.cursor)
            .unwrap_or(CursorIcon::Default);
        let tip = hovered
            .iter()
            .find(|&&i| self.tree.nodes[i].el.tooltip.is_some())
            .map(|&i| self.tree.nodes[i].gid);
        match (tip, self.tooltip, &self.tooltip_shown) {
            (Some(g), Some((pending, _)), _) if g == pending => {}
            (Some(_), _, Some(_)) => {}
            (Some(g), _, None) => self.tooltip = Some((g, self.shared.now + TOOLTIP_DELAY)),
            (None, _, shown) => {
                self.tooltip = None;
                if shown.is_some() {
                    self.tooltip_shown = None;
                    self.shared.dirty = true;
                }
            }
        }
        self.hovered = hovered;
    }

    fn paint(&mut self) {
        let background = self.state.background(&self.view_cx());
        let s = &self.shared;
        self.scene.clear(s.width, s.height, background);
        self.ime_area = None;
        let Some(root) = self.tree.root else {
            return;
        };
        let focused = s.focus.and_then(|h| self.tree.node_with_focus(h));
        let mut p = Painter {
            tree: &self.tree,
            text: &mut self.text,
            cache: &mut self.cache,
            state: &mut self.state,
            scene: &mut self.scene,
            hovered: &self.hovered,
            focused,
            focus_visible: s.focus_visible,
            scale: s.scale,
            ime_area: None,
        };
        let backdrop = background;
        p.node(root, 1.0, backdrop, None);
        // Overlays in the order they were placed: anchors' children, then
        // the tooltip, which has no parent.
        let overlays: Vec<usize> = self
            .tree
            .order
            .iter()
            .copied()
            .filter(|&i| {
                let n = &self.tree.nodes[i];
                n.parent
                    .is_none_or(|p| self.tree.nodes[p].overlay_child == Some(i))
                    && Some(i) != self.tree.root
            })
            .collect();
        for o in overlays {
            p.node(o, 1.0, backdrop, None);
        }
        self.ime_area = p.ime_area;
    }

    /// Handles input. Returns whether a frame is needed.
    pub fn input(&mut self, input: Input) -> bool {
        match input {
            Input::PointerMoved { x, y } => self.pointer_moved(x, y),
            Input::PointerLeft => {
                self.mouse = None;
                self.hover_changed();
            }
            Input::Pressed => self.pressed(),
            Input::Released => self.released(),
            Input::Wheel { dy } => self.wheel(dy),
            Input::Modifiers(m) => self.modifiers = m,
            Input::Key { key, text } => self.key(&key, text.as_deref()),
            Input::Ime(ime) => self.ime(&ime),
        }
        self.needs_frame()
    }

    /// Recomputes what is under the pointer; repaints if an element with
    /// a hover style came or went, or a tooltip did.
    fn hover_changed(&mut self) {
        let styled = |w: &Self| -> Vec<GlobalId> {
            w.hovered
                .iter()
                .filter(|&&i| w.tree.nodes[i].el.hover.is_some())
                .map(|&i| w.tree.nodes[i].gid)
                .collect()
        };
        let before = styled(self);
        let had_tip = self.tooltip_shown.is_some();
        self.update_hover();
        if styled(self) != before || had_tip != self.tooltip_shown.is_some() {
            self.shared.repaint = true;
        }
    }

    fn pointer(&self, node: usize) -> Pointer {
        let (x, y) = self.mouse.unwrap_or_default();
        let scale = self.shared.scale;
        let b = self.tree.nodes[node].bounds;
        Pointer {
            x: (x - b.x) / scale,
            y: (y - b.y) / scale,
            window_x: x / scale,
            window_y: y / scale,
            shift: self.modifiers.shift_key(),
        }
    }

    fn pointer_moved(&mut self, x: f32, y: f32) {
        self.mouse = Some((x, y));
        self.hover_changed();
        if let Some(h) = self.dragging
            && let Some(i) = self.tree.node_with_focus(h)
        {
            self.place_cursor(i, x, true);
        }
        let path = self.hovered.clone();
        for i in path {
            let h = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .and_then(|h| h.moved.clone());
            if let Some(h) = h {
                let p = self.pointer(i);
                self.run(|s, cx| h(s, &p, cx));
            }
        }
    }

    fn place_cursor(&mut self, i: usize, x: f32, extend: bool) {
        let n = &self.tree.nodes[i];
        let Kind::Input { field, .. } = &n.el.kind else {
            return;
        };
        let field = field.clone();
        let local = (x - n.content.x) / self.shared.scale;
        field(&mut self.state).place_cursor(&mut self.text, local, extend);
        self.shared.repaint = true;
    }

    fn pressed(&mut self) {
        let Some((x, y)) = self.mouse else {
            return;
        };
        let path = self.tree.hit(x, y);
        let outside: Vec<usize> = self
            .tree
            .order
            .iter()
            .copied()
            .filter(|i| !path.contains(i))
            .filter(|&i| {
                self.tree.nodes[i]
                    .el
                    .handlers
                    .as_ref()
                    .is_some_and(|h| h.press_outside.is_some())
            })
            .collect();
        for i in outside {
            let h = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .and_then(|h| h.press_outside.clone());
            if let Some(h) = h {
                self.run(|s, cx| h(s, cx));
            }
        }
        if let Some(&f) = path
            .iter()
            .find(|&&i| self.tree.nodes[i].el.focus.is_some())
        {
            let before = self.shared.focus;
            self.shared.focus = self.tree.nodes[f].el.focus;
            self.shared.focus_visible = false;
            self.shared.dirty = true;
            self.focus_changed(before);
            if matches!(self.tree.nodes[f].el.kind, Kind::Input { .. }) {
                self.place_cursor(f, x, self.modifiers.shift_key());
                self.dragging = self.tree.nodes[f].el.focus;
            }
        }
        self.pressed.clear();
        for &i in &path {
            let n = &self.tree.nodes[i];
            if n.el.handlers.as_ref().is_some_and(|h| h.click.is_some()) {
                self.pressed.insert(n.gid);
            }
        }
        for i in path {
            let h = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .and_then(|h| h.press.clone());
            if let Some(h) = h {
                let p = self.pointer(i);
                self.run(|s, cx| h(s, &p, cx));
            }
        }
    }

    fn released(&mut self) {
        self.dragging = None;
        let Some((x, y)) = self.mouse else {
            self.pressed.clear();
            return;
        };
        let path = self.tree.hit(x, y);
        for &i in &path {
            let h = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .and_then(|h| h.release.clone());
            if let Some(h) = h {
                let p = self.pointer(i);
                self.run(|s, cx| h(s, &p, cx));
            }
        }
        let pressed = std::mem::take(&mut self.pressed);
        let click = path.iter().find_map(|&i| {
            let n = &self.tree.nodes[i];
            let h = n.el.handlers.as_ref()?.click.clone()?;
            pressed.contains(&n.gid).then_some(h)
        });
        if let Some(h) = click {
            self.run(|s, cx| h(s, cx));
        }
    }

    fn wheel(&mut self, dy: f32) {
        let Some((x, y)) = self.mouse else {
            return;
        };
        for i in self.tree.hit(x, y) {
            let Some(scroll) = &self.tree.nodes[i].el.scroll else {
                continue;
            };
            let before = scroll.offset();
            scroll.set_offset((before + dy).clamp(0.0, scroll.max_offset()));
            if scroll.offset() != before {
                self.shared.dirty = true;
                return;
            }
        }
    }

    fn focused_node(&self) -> Option<usize> {
        self.shared.focus.and_then(|h| self.tree.node_with_focus(h))
    }

    fn key(&mut self, key: &Key, typed: Option<&str>) {
        let focused = self.focused_node();
        // A focused text input gets editing keys first.
        if let Some(f) = focused
            && let Kind::Input {
                field,
                on_change,
                on_submit,
                ..
            } = &self.tree.nodes[f].el.kind
        {
            let (field, on_change, on_submit) =
                (field.clone(), on_change.clone(), on_submit.clone());
            if let Some(cmd) = command_for(key, typed, self.modifiers) {
                let tf = field(&mut self.state);
                let before = tf.text().to_owned();
                tf.apply(cmd, &mut *self.shared.clipboard);
                let changed = tf.text() != before;
                self.shared.dirty = true;
                if changed && let Some(h) = on_change {
                    self.run(|s, cx| h(s, cx));
                }
                return;
            }
            if matches!(key, Key::Named(NamedKey::Enter))
                && self.modifiers.is_empty()
                && let Some(h) = on_submit
            {
                self.run(|s, cx| h(s, cx));
                return;
            }
        }
        let Some(stroke) = Keystroke::from_key(key, self.modifiers) else {
            return;
        };
        let path: Vec<usize> = match focused {
            Some(f) => self.tree.path(f),
            None => self.tree.root.into_iter().collect(),
        };
        let contexts: Vec<Vec<&'static str>> = path
            .iter()
            .rev()
            .map(|&i| self.tree.nodes[i].el.key_context.into_iter().collect())
            .collect();
        for action in self.keymap.matches(&stroke, &contexts) {
            if self.dispatch(&path, &*action) {
                return;
            }
        }
        // What no binding took.
        let plain = !(stroke.ctrl || stroke.alt || stroke.platform);
        match stroke.key.as_str() {
            "tab" if plain => {
                let next = self.tree.tab_neighbour(self.shared.focus, !stroke.shift);
                if let Some(h) = next {
                    let before = self.shared.focus;
                    self.shared.focus = Some(h);
                    self.shared.focus_visible = true;
                    self.shared.reveal = true;
                    self.shared.dirty = true;
                    self.focus_changed(before);
                }
            }
            "enter" | "space" if plain && !stroke.shift => {
                let click = focused.and_then(|f| {
                    self.tree.nodes[f]
                        .el
                        .handlers
                        .as_ref()
                        .and_then(|h| h.click.clone())
                });
                if let Some(h) = click {
                    self.run(|s, cx| h(s, cx));
                }
            }
            _ => {}
        }
    }

    /// Offers `action` to the handlers from `path[0]` up. Returns whether
    /// one handled it.
    fn dispatch(&mut self, path: &[usize], action: &dyn Action) -> bool {
        let t = action.action_type();
        for &i in path {
            let handlers: Vec<_> = self.tree.nodes[i]
                .el
                .handlers
                .as_ref()
                .map(|h| {
                    h.actions
                        .iter()
                        .filter(|(at, _)| *at == t)
                        .map(|(_, f)| f.clone())
                        .collect()
                })
                .unwrap_or_default();
            for h in handlers {
                self.shared.propagate = false;
                self.run(|s, cx| h(s, action, cx));
                if !self.shared.propagate {
                    return true;
                }
            }
        }
        false
    }

    /// Dispatches `action` as a key binding would.
    pub fn dispatch_action(&mut self, action: &dyn Action) -> bool {
        let path: Vec<usize> = match self.focused_node() {
            Some(f) => self.tree.path(f),
            None => self.tree.root.into_iter().collect(),
        };
        self.dispatch(&path, action)
    }

    fn ime(&mut self, ime: &Ime) {
        let Some(f) = self.focused_node() else {
            return;
        };
        let Kind::Input {
            field, on_change, ..
        } = &self.tree.nodes[f].el.kind
        else {
            return;
        };
        let (field, on_change) = (field.clone(), on_change.clone());
        let tf = field(&mut self.state);
        let before = tf.text().to_owned();
        if tf.ime(ime) {
            self.shared.dirty = true;
        }
        if tf.text() != before
            && let Some(h) = on_change
        {
            self.run(|s, cx| h(s, cx));
        }
    }
}

/// Moves an element into the tree, with its children; returns its index.
fn insert<S>(
    tree: &mut Tree<S>,
    mut el: Element<S>,
    parent: Option<usize>,
    parent_gid: GlobalId,
    inherited: ResolvedText,
    scale: f32,
) -> usize {
    // What scrolls keeps its children at their own size: they overflow
    // instead of being squeezed into the view.
    if parent.is_some_and(|p| tree.nodes[p].el.scroll.is_some()) {
        el.style.shrink = 0.0;
    }
    let text = inherited.inherit(&el.style.text);
    let gid = el.id.map_or(parent_gid, |id| parent_gid.child(id));
    let style = taffy_style(&el.style, el.scroll.is_some(), scale);
    let i = tree.nodes.len();
    let children_el = match &mut el.kind {
        Kind::Panel(c) => std::mem::take(c),
        _ => Vec::new(),
    };
    let overlay_el = match &mut el.kind {
        Kind::Overlay(c) => Some(std::mem::replace(c, Box::new(panel()))),
        _ => None,
    };
    let measure = match &el.kind {
        Kind::Text(_) => Some(Measure::Text(i)),
        Kind::Image(img) => Some(Measure::Fixed(img.width() as f32, img.height() as f32)),
        Kind::Input { .. } => Some(Measure::Fixed(
            0.0,
            (TextField::line_height(text.size) * scale).ceil(),
        )),
        Kind::Overlay(_) => Some(Measure::Fixed(0.0, 0.0)),
        _ => None,
    };
    let taffy = match measure {
        Some(m) => tree.taffy.new_leaf_with_context(style, m),
        None => tree.taffy.new_leaf(style),
    }
    .expect("taffy has no failing case for a new node");
    tree.nodes.push(Node {
        el,
        text,
        parent,
        children: Vec::new(),
        taffy,
        gid,
        bounds: Rect::default(),
        content: Rect::default(),
        clip: Rect::default(),
        overlay_child: None,
    });
    for c in children_el {
        let ci = insert(tree, c, Some(i), gid, text, scale);
        let ct = tree.nodes[ci].taffy;
        tree.nodes[i].children.push(ci);
        let _ = tree.taffy.add_child(taffy, ct);
    }
    if let Some(o) = overlay_el {
        let oi = insert(tree, *o, Some(i), gid, text, scale);
        tree.nodes[i].overlay_child = Some(oi);
    }
    i
}

fn taffy_style(s: &Style, scrolls: bool, scale: f32) -> tf::Style {
    let dim = |l: Length| match l {
        Length::Auto => tf::Dimension::auto(),
        Length::Px(v) => tf::Dimension::length(v * scale),
        Length::Fraction(f) => tf::Dimension::percent(f),
    };
    let lpa = |l: Length| match l {
        Length::Auto => tf::LengthPercentageAuto::auto(),
        Length::Px(v) => tf::LengthPercentageAuto::length(v * scale),
        Length::Fraction(f) => tf::LengthPercentageAuto::percent(f),
    };
    let lp = |v: f32| tf::LengthPercentage::length(v * scale);
    let lpa_px = |v: f32| tf::LengthPercentageAuto::length(v * scale);
    let align = |a: Align| match a {
        Align::Start => tf::AlignItems::FLEX_START,
        Align::Center => tf::AlignItems::CENTER,
        Align::End => tf::AlignItems::FLEX_END,
        Align::Stretch => tf::AlignItems::STRETCH,
    };
    let overflow = if scrolls {
        taffy::Overflow::Scroll
    } else {
        match s.overflow {
            Overflow::Visible => taffy::Overflow::Visible,
            Overflow::Clip => taffy::Overflow::Hidden,
        }
    };
    tf::Style {
        display: if s.hidden {
            tf::Display::None
        } else {
            tf::Display::Flex
        },
        flex_direction: match s.axis {
            Axis::Row => tf::FlexDirection::Row,
            Axis::Column => tf::FlexDirection::Column,
        },
        flex_wrap: if s.wrap {
            tf::FlexWrap::Wrap
        } else {
            tf::FlexWrap::NoWrap
        },
        flex_grow: s.grow,
        flex_shrink: s.shrink,
        flex_basis: dim(s.basis),
        align_items: Some(align(s.align_items)),
        align_self: s.align_self.map(align),
        justify_content: Some(match s.justify {
            Justify::Start => tf::JustifyContent::FLEX_START,
            Justify::Center => tf::JustifyContent::CENTER,
            Justify::End => tf::JustifyContent::FLEX_END,
            Justify::Between => tf::JustifyContent::SPACE_BETWEEN,
        }),
        gap: tf::Size {
            width: lp(s.gap),
            height: lp(s.gap),
        },
        size: tf::Size {
            width: dim(s.width),
            height: dim(s.height),
        },
        min_size: tf::Size {
            width: dim(s.min_width),
            height: dim(s.min_height),
        },
        max_size: tf::Size {
            width: dim(s.max_width),
            height: dim(s.max_height),
        },
        padding: tf::Rect {
            left: lp(s.padding.left),
            right: lp(s.padding.right),
            top: lp(s.padding.top),
            bottom: lp(s.padding.bottom),
        },
        margin: tf::Rect {
            left: lpa_px(s.margin.left),
            right: lpa_px(s.margin.right),
            top: lpa_px(s.margin.top),
            bottom: lpa_px(s.margin.bottom),
        },
        border: tf::Rect {
            left: lp(s.border_widths.left),
            right: lp(s.border_widths.right),
            top: lp(s.border_widths.top),
            bottom: lp(s.border_widths.bottom),
        },
        position: if s.absolute {
            tf::Position::Absolute
        } else {
            tf::Position::Relative
        },
        inset: tf::Rect {
            left: lpa(s.inset.left),
            right: lpa(s.inset.right),
            top: lpa(s.inset.top),
            bottom: lpa(s.inset.bottom),
        },
        overflow: taffy::Point {
            x: if scrolls {
                taffy::Overflow::Hidden
            } else {
                overflow
            },
            y: overflow,
        },
        scrollbar_width: 0.0,
        ..Default::default()
    }
}

/// What laying out a tree needs.
struct Layout<'a, S> {
    text: &'a mut TextSystem,
    cache: &'a mut TextCache,
    scale: f32,
    state: &'a S,
    vcx: &'a ViewCx,
}

impl<S: 'static> Layout<'_, S> {
    /// Lays out the subtree at `root` in the space given (device pixels;
    /// `None` for as much as it wants).
    fn compute(
        &mut self,
        tree: &mut Tree<S>,
        root: usize,
        width: Option<f32>,
        height: Option<f32>,
    ) {
        let space =
            |v: Option<f32>| v.map_or(tf::AvailableSpace::MaxContent, tf::AvailableSpace::Definite);
        let scale = self.scale;
        let (nodes, taffy) = (&tree.nodes, &mut tree.taffy);
        let (text, cache) = (&mut *self.text, &mut *self.cache);
        let result = taffy.compute_layout_with_measure(
            nodes[root].taffy,
            tf::Size {
                width: space(width),
                height: space(height),
            },
            |known, available, _id, ctx, _style| {
                let Some(m) = ctx else {
                    return tf::Size::ZERO;
                };
                match *m {
                    Measure::Fixed(w, h) => tf::Size {
                        width: known.width.unwrap_or(w),
                        height: known.height.unwrap_or(h),
                    },
                    Measure::Text(i) => {
                        let n = &nodes[i];
                        let Kind::Text(s) = &n.el.kind else {
                            return tf::Size::ZERO;
                        };
                        let max = if n.text.nowrap {
                            None
                        } else {
                            match (known.width, available.width) {
                                (Some(w), _) => Some(w / scale),
                                (None, tf::AvailableSpace::Definite(w)) => Some(w / scale),
                                (None, tf::AvailableSpace::MinContent) => Some(0.0),
                                (None, tf::AvailableSpace::MaxContent) => None,
                            }
                        };
                        let laid = cache.get(text, s, &n.text, max);
                        tf::Size {
                            width: known.width.unwrap_or((laid.width * scale).ceil()),
                            height: known.height.unwrap_or((laid.height * scale).ceil()),
                        }
                    }
                }
            },
        );
        if let Err(e) = result {
            tracing::error!("layout failed: {e}");
        }
    }

    /// Sets the bounds of the subtree at `i`, whose parent's border box
    /// starts at `(ox, oy)`, and lists it in paint order.
    fn place(&mut self, tree: &mut Tree<S>, i: usize, ox: f32, oy: f32, clip: Rect) {
        let l = *tree
            .taffy
            .layout(tree.nodes[i].taffy)
            .expect("node is in the tree");
        let bounds = Rect::new(
            ox + l.location.x,
            oy + l.location.y,
            l.size.width,
            l.size.height,
        );
        let content = Rect::new(
            bounds.x + l.border.left + l.padding.left,
            bounds.y + l.border.top + l.padding.top,
            (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right)
                .max(0.0),
            (l.size.height - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom)
                .max(0.0),
        );
        {
            let n = &mut tree.nodes[i];
            n.bounds = bounds;
            n.content = content;
            n.clip = clip;
            if n.el.style.hidden {
                return;
            }
        }
        tree.order.push(i);
        let n = &tree.nodes[i];
        if n.el.tab_stop && n.el.focus.is_some() {
            tree.tab_order.push(i);
        }
        if n.overlay_child.is_some() {
            tree.anchors.push(i);
        }
        let child_clip = if n.el.style.overflow == Overflow::Clip || n.el.scroll.is_some() {
            clip.intersect(&bounds)
        } else {
            clip
        };
        let scale = self.scale;
        let mut dy = 0.0;
        if let Some(scroll) = n.el.scroll.clone() {
            if let Kind::List { count, row } = &n.el.kind {
                let (count, row) = (*count, row.clone());
                self.place_rows(tree, i, &scroll, count, &row, child_clip);
                return;
            }
            let mut st = scroll.state();
            st.viewport = Rect::new(
                content.x / scale,
                content.y / scale,
                content.w / scale,
                content.h / scale,
            );
            st.content_height =
                (l.content_size.height - l.border.top - l.padding.top + l.padding.bottom).max(0.0)
                    / scale;
            let max = (st.content_height - st.viewport.h).max(0.0);
            st.offset = st.offset.clamp(0.0, max);
            dy = (st.offset * scale).round();
        }
        let children = tree.nodes[i].children.clone();
        for c in children {
            self.place(tree, c, bounds.x, bounds.y - dy, child_clip);
        }
    }

    /// Builds and places the rows of a list that are in view.
    fn place_rows(
        &mut self,
        tree: &mut Tree<S>,
        list: usize,
        scroll: &ScrollHandle,
        count: usize,
        row: &crate::element::RowFn<S>,
        clip: Rect,
    ) {
        let scale = self.scale;
        let view = tree.nodes[list].content;
        let (gid, text) = (tree.nodes[list].gid, tree.nodes[list].text);
        let build = |this: &mut Self, tree: &mut Tree<S>, r: usize| {
            let el = row(this.state, r, this.vcx);
            let ri = insert(tree, el, Some(list), gid, text, scale);
            // A row is as wide as the list.
            let t = tree.nodes[ri].taffy;
            if let Ok(style) = tree.taffy.style(t) {
                let mut style = style.clone();
                if style.size.width == tf::Dimension::auto() {
                    style.size.width = tf::Dimension::length(view.w);
                }
                let _ = tree.taffy.set_style(t, style);
            }
            this.compute(tree, ri, Some(view.w), None);
            ri
        };
        let mut st = scroll.state();
        st.viewport = Rect::new(
            view.x / scale,
            view.y / scale,
            view.w / scale,
            view.h / scale,
        );
        if count == 0 {
            st.content_height = 0.0;
            st.offset = 0.0;
            st.row_height = 0.0;
            return;
        }
        drop(st);
        let first = build(self, tree, 0);
        let row_h = tree
            .taffy
            .layout(tree.nodes[first].taffy)
            .map_or(0.0, |l| l.size.height)
            .max(1.0);
        let mut st = scroll.state();
        st.row_height = row_h / scale;
        st.content_height = count as f32 * row_h / scale;
        if let Some(r) = st.reveal_row.take() {
            let top = r.min(count - 1) as f32 * row_h;
            let off = st.offset * scale;
            if top < off {
                st.offset = top / scale;
            } else if top + row_h > off + view.h {
                st.offset = (top + row_h - view.h) / scale;
            }
        }
        let max = (st.content_height - st.viewport.h).max(0.0);
        st.offset = st.offset.clamp(0.0, max);
        let dy = (st.offset * scale).round();
        drop(st);
        let start = (dy / row_h).floor() as usize;
        let end = (((dy + view.h) / row_h).ceil() as usize).min(count);
        for r in start..end {
            let ri = if r == 0 { first } else { build(self, tree, r) };
            tree.nodes[list].children.push(ri);
            self.place(tree, ri, view.x, view.y + r as f32 * row_h - dy, clip);
        }
    }

    /// Lays out an overlay's content on its own and places it at `(x, y)`,
    /// moved to stay inside `window`.
    fn place_overlay(&mut self, tree: &mut Tree<S>, i: usize, x: f32, y: f32, window: Rect) {
        self.compute(tree, i, None, None);
        let l = *tree
            .taffy
            .layout(tree.nodes[i].taffy)
            .expect("node is in the tree");
        let m = OVERLAY_MARGIN * self.scale;
        let (w, h) = (l.size.width, l.size.height);
        let x = x.min(window.right() - m - w).max(window.x + m);
        let y = y.min(window.bottom() - m - h).max(window.y + m);
        self.place(tree, i, x - l.location.x, y - l.location.y, window);
    }
}

/// What painting a tree needs.
struct Painter<'a, S> {
    tree: &'a Tree<S>,
    text: &'a mut TextSystem,
    cache: &'a mut TextCache,
    state: &'a mut S,
    scene: &'a mut Scene,
    hovered: &'a [usize],
    focused: Option<usize>,
    focus_visible: bool,
    scale: f32,
    ime_area: Option<Rect>,
}

impl<S: 'static> Painter<'_, S> {
    /// Paints node `i` and what is inside it. `text_color` is a hover or
    /// focus style's text colour from an ancestor, which the text inside
    /// takes over its own.
    fn node(&mut self, i: usize, alpha: f32, backdrop: Color, text_color: Option<Color>) {
        let tree = self.tree;
        let n = &tree.nodes[i];
        if n.el.style.hidden || n.bounds.is_empty() && n.children.is_empty() {
            return;
        }
        let scale = self.scale;
        let mut style = n.el.style.clone();
        let mut text_color = text_color;
        if let Some(h) = &n.el.hover
            && self.hovered.contains(&i)
        {
            style.refine_paint(h);
            text_color = h.text.color.or(text_color);
        }
        if let Some(f) = &n.el.focus_style
            && self.focus_visible
            && self.focused == Some(i)
        {
            style.refine_paint(f);
            text_color = f.text.color.or(text_color);
        }
        let alpha = alpha * style.opacity.unwrap_or(1.0);
        let fade = |c: Color| c.with_alpha(c.a * alpha);
        let b = n.bounds;
        let radius = (style.radius * scale).min(b.w.min(b.h) / 2.0);
        for s in &style.shadows {
            let spread = s.spread * scale;
            self.scene.quad(Quad {
                bounds: b
                    .translate(s.offset_x * scale, s.offset_y * scale)
                    .outset(spread),
                background: fade(s.color),
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                corner_radius: (radius + spread).max(0.0),
                blur: s.blur * scale / 2.0,
            });
        }
        let widths = style.border_widths;
        let even =
            widths.top == widths.right && widths.top == widths.bottom && widths.top == widths.left;
        let border_color = style.border_color.map(fade).unwrap_or(Color::TRANSPARENT);
        let background = style.background.map(fade).unwrap_or(Color::TRANSPARENT);
        if background.a > 0.0 || even && widths.top > 0.0 && border_color.a > 0.0 {
            self.scene.quad(Quad {
                bounds: b,
                background,
                border_color: if even {
                    border_color
                } else {
                    Color::TRANSPARENT
                },
                border_width: if even { widths.top * scale } else { 0.0 },
                corner_radius: radius,
                blur: 0.0,
            });
        }
        if !even && border_color.a > 0.0 {
            let w = |v: f32| (v * scale).round();
            let sides = [
                Rect::new(b.x, b.y, b.w, w(widths.top)),
                Rect::new(b.x, b.bottom() - w(widths.bottom), b.w, w(widths.bottom)),
                Rect::new(b.x, b.y, w(widths.left), b.h),
                Rect::new(b.right() - w(widths.right), b.y, w(widths.right), b.h),
            ];
            for r in sides.into_iter().filter(|r| !r.is_empty()) {
                self.scene.quad(Quad::filled(r, border_color));
            }
        }
        let c = n.content;
        match &n.el.kind {
            Kind::Text(s) => {
                let max = (!n.text.nowrap).then_some(c.w / scale);
                let laid = self.cache.get(self.text, s, &n.text, max);
                let color = fade(text_color.unwrap_or(n.text.color));
                self.text
                    .paint_layout(self.scene, &laid, c.x, c.y, scale, color);
            }
            Kind::Image(img) => {
                let x = (c.x + (c.w - img.width() as f32) / 2.0).round() as i32;
                let y = (c.y + (c.h - img.height() as f32) / 2.0).round() as i32;
                self.scene.push_clip(b);
                self.scene.image(img, x, y, alpha);
                self.scene.pop_clip();
            }
            Kind::Canvas(paint) => {
                self.scene.push_clip(n.clip.intersect(&b));
                paint(b, self.scene, scale);
                self.scene.pop_clip();
            }
            Kind::Input {
                field, style: fs, ..
            } => {
                let focused = self.focused == Some(i);
                let fs = *fs;
                let tf = field(self.state);
                tf.set_font_size(n.text.size);
                let caret = tf.paint(self.text, self.scene, c, scale, &fs, focused);
                if focused {
                    self.ime_area = caret.or(Some(c));
                }
            }
            Kind::Panel(_) | Kind::List { .. } | Kind::Overlay(_) => {}
        }
        if n.children.is_empty() {
            return;
        }
        // An opaque backdrop for the children: this element's background,
        // or what was behind it.
        let backdrop = style
            .background
            .filter(|c| c.a >= 1.0 && alpha >= 1.0)
            .unwrap_or(backdrop);
        let clips = style.overflow == Overflow::Clip || n.el.scroll.is_some();
        let layer =
            n.el.scroll
                .as_ref()
                .filter(|_| !self.scene.in_scroll_layer() && radius == 0.0 && alpha >= 1.0)
                .map(|s| (s.id(), (s.offset() * scale).round() as i32));
        match layer {
            Some((id, offset)) => {
                let inner = Rect::new(
                    b.x + widths.left * scale,
                    b.y + widths.top * scale,
                    b.w - (widths.left + widths.right) * scale,
                    b.h - (widths.top + widths.bottom) * scale,
                );
                self.scene.push_scroll_layer(id, inner, offset, backdrop);
            }
            None if clips => self.scene.push_clip(b),
            None => {}
        }
        for &child in &n.children {
            self.node(child, alpha, backdrop, text_color);
        }
        match layer {
            Some(_) => self.scene.pop_scroll_layer(),
            None if clips => self.scene.pop_clip(),
            None => {}
        }
    }
}
