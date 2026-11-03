// SPDX-License-Identifier: GPL-2.0-or-later
//! The element tree a view builds (ADR 0004, "Views are functions").
//!
//! A view is a function from the application's state `S` to an
//! [`Element<S>`]. Elements are plain data: a style, children, and
//! handlers that get `&mut S` and a [`Cx`] when an event reaches them,
//! after the frame is built. The tree is built again whenever something
//! changed; what lasts between frames is keyed by [`ElementId`]s,
//! [`FocusHandle`]s and [`ScrollHandle`]s.

use std::any::TypeId;
use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::image::Image;
use crate::keys::Action;
use crate::scene::{Rect, Scene};
use crate::style::{Style, Styled};
use crate::text_field::{FieldStyle, TextField};
use crate::window::{Cx, ViewCx};

/// Names an element so that what it keeps between frames (being
/// pressed, a scroll offset, being focused through [`FocusHandle::named`])
/// stays with it. Ids need only be unique among the ids of the same
/// parent element: an element's identity is the path of ids above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(u64);

impl ElementId {
    fn of(v: impl Hash) -> Self {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        ElementId(h.finish())
    }
}

impl From<&str> for ElementId {
    fn from(s: &str) -> Self {
        Self::of(s)
    }
}

impl From<String> for ElementId {
    fn from(s: String) -> Self {
        Self::of(s.as_str())
    }
}

impl From<(&str, u64)> for ElementId {
    fn from(v: (&str, u64)) -> Self {
        Self::of(v)
    }
}

impl From<u64> for ElementId {
    fn from(v: u64) -> Self {
        Self::of(v)
    }
}

/// An element's identity: the hash of the ids from the root down to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GlobalId(pub(crate) u64);

impl GlobalId {
    pub(crate) const ROOT: GlobalId = GlobalId(0);

    pub(crate) fn child(self, id: ElementId) -> GlobalId {
        let mut h = DefaultHasher::new();
        (self.0, id.0).hash(&mut h);
        GlobalId(h.finish())
    }
}

/// Something that can hold keyboard focus. The state that owns a control
/// keeps its handle; an element takes focus through it with
/// [`Element::focus`]. Two elements that replace each other (Pause and
/// Resume) can share one, so focus survives the swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FocusHandle(u64);

static NEXT_FOCUS: AtomicU64 = AtomicU64::new(1);

impl FocusHandle {
    /// A handle of its own, unlike any other.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // The top bit marks handles made here, so they never equal a named one.
        FocusHandle(NEXT_FOCUS.fetch_add(1, Ordering::Relaxed) | 1 << 63)
    }

    /// The same handle for the same name, every frame, so a control
    /// needs no state of its own to keep focus.
    pub fn named(name: impl Into<ElementId>) -> Self {
        FocusHandle(name.into().0 & !(1 << 63))
    }
}

/// How far a scrolling element or list is scrolled, shared between the
/// state that keeps it and the toolkit. Offsets are logical pixels down
/// from the top.
#[derive(Clone, Debug)]
pub struct ScrollHandle(Rc<RefCell<ScrollState>>);

#[derive(Debug)]
pub(crate) struct ScrollState {
    pub(crate) id: u32,
    pub(crate) offset: f32,
    /// The scrolled area on screen, logical pixels, as last laid out.
    pub(crate) viewport: Rect,
    pub(crate) content_height: f32,
    /// A list's row height, logical pixels, as last laid out.
    pub(crate) row_height: f32,
    pub(crate) reveal_row: Option<usize>,
}

static NEXT_SCROLL: AtomicU64 = AtomicU64::new(1);

impl Default for ScrollHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollHandle {
    pub fn new() -> Self {
        // Scroll layer ids are u32 and 0 is the window's.
        let id = (NEXT_SCROLL.fetch_add(1, Ordering::Relaxed) % u64::from(u32::MAX)) as u32 + 1;
        ScrollHandle(Rc::new(RefCell::new(ScrollState {
            id,
            offset: 0.0,
            viewport: Rect::default(),
            content_height: 0.0,
            row_height: 0.0,
            reveal_row: None,
        })))
    }

    pub fn offset(&self) -> f32 {
        self.0.borrow().offset
    }

    /// Scrolls to `offset`, kept between 0 and [`ScrollHandle::max_offset`]
    /// when next laid out.
    pub fn set_offset(&self, offset: f32) {
        self.0.borrow_mut().offset = offset.max(0.0);
    }

    pub fn max_offset(&self) -> f32 {
        let s = self.0.borrow();
        (s.content_height - s.viewport.h).max(0.0)
    }

    /// The scrolled area on screen, in logical pixels.
    pub fn viewport(&self) -> Rect {
        self.0.borrow().viewport
    }

    /// For a list: scrolls just far enough to show row `row` whole.
    pub fn reveal_row(&self, row: usize) {
        self.0.borrow_mut().reveal_row = Some(row);
    }

    pub(crate) fn state(&self) -> std::cell::RefMut<'_, ScrollState> {
        self.0.borrow_mut()
    }

    pub(crate) fn id(&self) -> u32 {
        self.0.borrow().id
    }
}

/// Where the pointer is, relative to the element it is delivered to and
/// in the window, logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub window_x: f32,
    pub window_y: f32,
    pub shift: bool,
}

pub(crate) type Handler<S> = Rc<dyn Fn(&mut S, &mut Cx<S>)>;
pub(crate) type PointerHandler<S> = Rc<dyn Fn(&mut S, &Pointer, &mut Cx<S>)>;
pub(crate) type ActionHandler<S> = Rc<dyn Fn(&mut S, &dyn Action, &mut Cx<S>)>;
pub(crate) type RowFn<S> = Rc<dyn Fn(&S, usize, &ViewCx) -> Element<S>>;
pub(crate) type FieldAccess<S> = Rc<dyn Fn(&mut S) -> &mut TextField>;

/// Paints a [`canvas`]: its bounds in device pixels, the scene, and the
/// scale in device pixels per logical pixel.
pub type PaintFn = Rc<dyn Fn(Rect, &mut Scene, f32)>;

#[derive(Default)]
pub(crate) struct Handlers<S> {
    pub(crate) click: Option<Handler<S>>,
    pub(crate) press: Option<PointerHandler<S>>,
    pub(crate) release: Option<PointerHandler<S>>,
    pub(crate) moved: Option<PointerHandler<S>>,
    pub(crate) press_outside: Option<Handler<S>>,
    pub(crate) focus_out: Option<Handler<S>>,
    pub(crate) actions: Vec<(TypeId, ActionHandler<S>)>,
}

pub(crate) enum Kind<S> {
    Panel(Vec<Element<S>>),
    Text(String),
    Image(Arc<Image>),
    Canvas(PaintFn),
    List {
        count: usize,
        row: RowFn<S>,
    },
    Input {
        field: FieldAccess<S>,
        style: FieldStyle,
        on_change: Option<Handler<S>>,
        on_submit: Option<Handler<S>>,
    },
    /// Laid out on its own and painted over everything, at the place it
    /// would have taken in its parent, moved to stay inside the window.
    Overlay(Box<Element<S>>),
}

/// One element of a view.
pub struct Element<S> {
    pub(crate) style: Style,
    pub(crate) kind: Kind<S>,
    pub(crate) id: Option<ElementId>,
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) tab_stop: bool,
    pub(crate) key_context: Option<&'static str>,
    pub(crate) hover: Option<Box<Style>>,
    pub(crate) focus_style: Option<Box<Style>>,
    pub(crate) scroll: Option<ScrollHandle>,
    pub(crate) tooltip: Option<String>,
    pub(crate) handlers: Option<Box<Handlers<S>>>,
}

impl<S> Styled for Element<S> {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

fn element<S>(kind: Kind<S>) -> Element<S> {
    Element {
        style: Style::default(),
        kind,
        id: None,
        focus: None,
        tab_stop: false,
        key_context: None,
        hover: None,
        focus_style: None,
        scroll: None,
        tooltip: None,
        handlers: None,
    }
}

/// A box that holds other elements.
pub fn panel<S>() -> Element<S> {
    element(Kind::Panel(Vec::new()))
}

/// Text, wrapped to the width it is given unless its style says
/// [`Styled::nowrap`].
pub fn text<S>(s: impl Into<String>) -> Element<S> {
    element(Kind::Text(s.into()))
}

/// An image, drawn pixel for pixel: it is decoded and scaled to the size
/// it is drawn at before it reaches the toolkit (ADR 0004). It is centred
/// in the element and clipped to it; with no size of its own the element
/// takes the image's.
pub fn image<S>(image: Arc<Image>) -> Element<S> {
    element(Kind::Image(image))
}

/// Paints whatever `paint` adds to the scene, inside its bounds.
pub fn canvas<S>(paint: impl Fn(Rect, &mut Scene, f32) + 'static) -> Element<S> {
    element(Kind::Canvas(Rc::new(paint)))
}

/// A list of `count` rows of equal height, of which only the rows in
/// view are built: `row` builds row `i`. Row 0 is built to measure the
/// height. The list scrolls through `scroll`.
pub fn list<S: 'static>(
    scroll: &ScrollHandle,
    count: usize,
    row: impl Fn(&S, usize, &ViewCx) -> Element<S> + 'static,
) -> Element<S> {
    let mut e = element(Kind::List {
        count,
        row: Rc::new(row),
    });
    e.scroll = Some(scroll.clone());
    e.style.overflow = crate::style::Overflow::Clip;
    e
}

/// A single-line text field over the [`TextField`] that `field` finds in
/// the state. It takes focus through `focus` and is a tab stop.
pub fn text_input<S: 'static>(
    focus: FocusHandle,
    style: FieldStyle,
    field: impl Fn(&mut S) -> &mut TextField + 'static,
) -> Element<S> {
    let mut e = element(Kind::Input {
        field: Rc::new(field),
        style,
        on_change: None,
        on_submit: None,
    });
    e.focus = Some(focus);
    e.tab_stop = true;
    e.style.cursor = Some(winit::window::CursorIcon::Text);
    e
}

/// Lays `child` out on its own, over everything else, where the overlay
/// itself sits in its parent, moved as little as it takes to stay inside
/// the window. For menus.
pub fn overlay<S>(child: Element<S>) -> Element<S> {
    element(Kind::Overlay(Box::new(child)))
}

impl<S> From<&str> for Element<S> {
    fn from(s: &str) -> Self {
        text(s)
    }
}

impl<S> From<String> for Element<S> {
    fn from(s: String) -> Self {
        text(s)
    }
}

impl<S: 'static> Element<S> {
    fn handlers(&mut self) -> &mut Handlers<S> {
        self.handlers.get_or_insert_with(|| {
            Box::new(Handlers {
                click: None,
                press: None,
                release: None,
                moved: None,
                press_outside: None,
                focus_out: None,
                actions: Vec::new(),
            })
        })
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn child(mut self, child: impl Into<Element<S>>) -> Self {
        if let Kind::Panel(children) = &mut self.kind {
            children.push(child.into());
        } else {
            debug_assert!(false, "only a panel has children");
        }
        self
    }

    pub fn children<E: Into<Element<S>>>(mut self, iter: impl IntoIterator<Item = E>) -> Self {
        if let Kind::Panel(children) = &mut self.kind {
            children.extend(iter.into_iter().map(Into::into));
        } else {
            debug_assert!(false, "only a panel has children");
        }
        self
    }

    /// Takes keyboard focus through `focus`: from a press on it, and, if
    /// it is a tab stop, from Tab.
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    /// Whether Tab reaches it; it needs a focus handle.
    pub fn tab_stop(mut self, stop: bool) -> Self {
        self.tab_stop = stop;
        self
    }

    /// Bindings with this context apply while focus is inside the element.
    pub fn key_context(mut self, context: &'static str) -> Self {
        self.key_context = Some(context);
        self
    }

    /// Paint properties while the pointer is over the element.
    pub fn hover_style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.hover = Some(Box::new(f(Style::default())));
        self
    }

    /// Paint properties while the element has focus from the keyboard
    /// (not from a press): the focus ring.
    pub fn focus_style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.focus_style = Some(Box::new(f(Style::default())));
        self
    }

    /// Clips the content and scrolls it vertically through `scroll`. Its
    /// children keep their own height (they do not shrink to fit).
    pub fn scroll_y(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self.style.overflow = crate::style::Overflow::Clip;
        self
    }

    /// Shown near the pointer after it rests on the element.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// A press and release of the primary button on the element, or Enter
    /// or Space while it has focus.
    pub fn on_click(mut self, f: impl Fn(&mut S, &mut Cx<S>) + 'static) -> Self {
        self.handlers().click = Some(Rc::new(f));
        self
    }

    pub fn on_press(mut self, f: impl Fn(&mut S, &Pointer, &mut Cx<S>) + 'static) -> Self {
        self.handlers().press = Some(Rc::new(f));
        self
    }

    pub fn on_release(mut self, f: impl Fn(&mut S, &Pointer, &mut Cx<S>) + 'static) -> Self {
        self.handlers().release = Some(Rc::new(f));
        self
    }

    pub fn on_pointer_move(mut self, f: impl Fn(&mut S, &Pointer, &mut Cx<S>) + 'static) -> Self {
        self.handlers().moved = Some(Rc::new(f));
        self
    }

    /// A press anywhere outside the element: closes menus.
    pub fn on_press_outside(mut self, f: impl Fn(&mut S, &mut Cx<S>) + 'static) -> Self {
        self.handlers().press_outside = Some(Rc::new(f));
        self
    }

    /// Focus left the element and everything in it.
    pub fn on_focus_out(mut self, f: impl Fn(&mut S, &mut Cx<S>) + 'static) -> Self {
        self.handlers().focus_out = Some(Rc::new(f));
        self
    }

    /// Handles action `A` when it comes up from the focused element.
    pub fn on_action<A: Action>(mut self, f: impl Fn(&mut S, &A, &mut Cx<S>) + 'static) -> Self {
        let f: ActionHandler<S> = Rc::new(move |s, a, cx| {
            if let Some(a) = a.as_any().downcast_ref::<A>() {
                f(s, a, cx);
            }
        });
        self.handlers().actions.push((TypeId::of::<A>(), f));
        self
    }

    /// For a text input: after each edit.
    pub fn on_change(mut self, f: impl Fn(&mut S, &mut Cx<S>) + 'static) -> Self {
        if let Kind::Input { on_change, .. } = &mut self.kind {
            *on_change = Some(Rc::new(f));
        }
        self
    }

    /// For a text input: Enter.
    pub fn on_submit(mut self, f: impl Fn(&mut S, &mut Cx<S>) + 'static) -> Self {
        if let Kind::Input { on_submit, .. } = &mut self.kind {
            *on_submit = Some(Rc::new(f));
        }
        self
    }
}
