// SPDX-License-Identifier: LGPL-2.1-or-later
//! The toolkit driven without a platform window: layout, input, focus,
//! key bindings, lists, overlays and timers (ADR 0004, "Headless").

use std::cell::RefCell;
use std::time::{Duration, Instant};

use ossteam_ui::element::{
    Element, FocusHandle, ScrollHandle, canvas, list, overlay, panel, text, text_input,
};
use ossteam_ui::keys::KeyBinding;
use ossteam_ui::scene::{Color, Primitive, Quad, Rect, Scene};
use ossteam_ui::style::{Edges, Styled};
use ossteam_ui::text::TextSystem;
use ossteam_ui::text_field::{FieldStyle, TextField};
use ossteam_ui::window::{App, Cx, Input, Timer, ViewCx, Window};
use winit::event::Ime;
use winit::keyboard::{Key, ModifiersState, NamedKey};

const RED: Color = Color::rgb(0xff0000);
const BLUE: Color = Color::rgb(0x0000ff);
const RING: Color = Color::rgb(0x00ff00);

fn fonts() -> TextSystem {
    TextSystem::load().expect("the test machine has fonts")
}

fn window<A: App>(app: A, w: u32, h: u32, scale: f32) -> Window<A> {
    let mut win = Window::new(app, fonts(), Box::new(None::<String>), w, h, scale);
    win.frame();
    win
}

fn named(k: NamedKey) -> Input {
    Input::Key {
        key: Key::Named(k),
        text: None,
    }
}

fn typed(s: &str) -> Input {
    Input::Key {
        key: Key::Character(s.into()),
        text: Some(s.into()),
    }
}

fn quads(scene: &Scene) -> Vec<Quad> {
    scene
        .items
        .iter()
        .filter_map(|i| match i.primitive {
            Primitive::Quad(q) => Some(q),
            _ => None,
        })
        .collect()
}

fn quad_with(scene: &Scene, c: Color) -> Option<Quad> {
    quads(scene).into_iter().find(|q| q.background == c)
}

struct Boxes;

impl App for Boxes {
    type Message = ();

    fn view(&self, _: &ViewCx) -> Element<Self> {
        panel()
            .fill()
            .row()
            .pad(10.0)
            .gap(5.0)
            .child(panel().size(20.0, 30.0).background(RED))
            .child(
                panel()
                    .grow()
                    .height(30.0)
                    .background(BLUE)
                    .border(1.0, RING),
            )
    }
}

#[test]
fn layout_is_in_whole_device_pixels_at_any_scale() {
    let mut w = window(Boxes, 200, 100, 1.0);
    let s = w.frame().cloned().unwrap_or_else(|| w.scene().clone());
    assert_eq!(
        quad_with(&s, RED).unwrap().bounds,
        Rect::new(10.0, 10.0, 20.0, 30.0)
    );
    let blue = quad_with(&s, BLUE).unwrap();
    assert_eq!(blue.bounds, Rect::new(35.0, 10.0, 155.0, 30.0));
    assert_eq!(blue.border_width, 1.0);

    // 1.5: 200x100 logical is 300x150 device.
    let w = window(Boxes, 300, 150, 1.5);
    let s = w.scene();
    let red = quad_with(s, RED).unwrap().bounds;
    assert_eq!(red, Rect::new(15.0, 15.0, 30.0, 45.0));
    let blue = quad_with(s, BLUE).unwrap().bounds;
    for v in [blue.x, blue.y, blue.w, blue.h] {
        assert_eq!(v, v.round(), "{blue:?} is not on whole pixels");
    }
    assert_eq!(blue.right(), 285.0);
}

#[derive(Clone)]
struct Activate;

#[derive(Clone)]
struct Escape;

struct Controls {
    a: FocusHandle,
    b: FocusHandle,
    clicks: Vec<&'static str>,
    actions: Vec<&'static str>,
    field: TextField,
    changes: u32,
    submitted: Option<String>,
}

impl Controls {
    fn new() -> Self {
        Self {
            a: FocusHandle::new(),
            b: FocusHandle::named("b"),
            clicks: Vec::new(),
            actions: Vec::new(),
            field: TextField::new("Search", 14.0),
            changes: 0,
            submitted: None,
        }
    }
}

fn button(id: &'static str, focus: FocusHandle) -> Element<Controls> {
    panel()
        .id(id)
        .focus(focus)
        .tab_stop(true)
        .size(80.0, 24.0)
        .background(BLUE)
        .border(2.0, BLUE)
        .focus_style(|s| s.border_color(RING))
        .hover_style(|s| s.background(RED))
        .on_click(move |s: &mut Controls, _| s.clicks.push(id))
        .child(text(id))
}

impl App for Controls {
    type Message = &'static str;

    fn view(&self, _: &ViewCx) -> Element<Self> {
        let field = FieldStyle {
            text: Color::rgb(0),
            placeholder: Color::rgb(0x808080),
            selection: Color::rgb(0xccccff),
            caret: Color::rgb(0),
        };
        panel()
            .fill()
            .gap(4.0)
            .key_context("Page")
            .on_action(|s: &mut Self, _: &Activate, _| s.actions.push("page"))
            .on_action(|s: &mut Self, _: &Escape, _| s.actions.push("page-escape"))
            .child(button("a", self.a))
            .child(
                panel()
                    .key_context("Inner")
                    .on_action(|s: &mut Self, _: &Escape, cx: &mut Cx<Self>| {
                        s.actions.push("inner-escape");
                        cx.propagate();
                    })
                    .child(button("b", self.b)),
            )
            .child(
                text_input(FocusHandle::named("field"), field, |s: &mut Self| {
                    &mut s.field
                })
                .id("field")
                .height(24.0)
                .on_change(|s: &mut Self, _| s.changes += 1)
                .on_submit(|s: &mut Self, _| s.submitted = Some(s.field.text().to_owned())),
            )
    }

    fn message(&mut self, m: &'static str, _: &mut Cx<Self>) {
        self.actions.push(m);
    }
}

#[test]
fn a_press_and_release_clicks_and_focuses_without_a_ring() {
    let mut w = window(Controls::new(), 400, 300, 1.0);
    assert!(w.click("a"));
    assert_eq!(w.state.clicks, ["a"]);
    assert_eq!(w.focused(), Some(w.state.a));
    w.frame();
    // Hovered, so red; focused by the pointer, so no ring.
    let q = quad_with(w.scene(), RED).expect("hover style");
    assert_eq!(q.border_color, BLUE);
}

#[test]
fn a_release_elsewhere_is_no_click() {
    let mut w = window(Controls::new(), 400, 300, 1.0);
    let a = w.bounds_of("a").unwrap();
    w.input(Input::PointerMoved {
        x: a.x + 5.0,
        y: a.y + 5.0,
    });
    w.input(Input::Pressed);
    w.input(Input::PointerMoved { x: 300.0, y: 290.0 });
    w.input(Input::Released);
    assert!(w.state.clicks.is_empty());
}

#[test]
fn tab_walks_the_stops_in_tree_order_with_a_ring_and_enter_clicks() {
    let mut w = window(Controls::new(), 400, 300, 1.0);
    w.input(named(NamedKey::Tab));
    w.frame();
    assert_eq!(w.focused(), Some(w.state.a));
    let rings: Vec<_> = quads(w.scene())
        .into_iter()
        .filter(|q| q.border_color == RING)
        .collect();
    assert_eq!(rings.len(), 1);
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(FocusHandle::named("b")));
    w.input(named(NamedKey::Enter));
    w.input(Input::Key {
        key: Key::Named(NamedKey::Space),
        text: Some(" ".into()),
    });
    assert_eq!(w.state.clicks, ["b", "b"]);
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(FocusHandle::named("field")));
    // Wraps round; Shift+Tab goes back.
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(w.state.a));
    w.input(Input::Modifiers(ModifiersState::SHIFT));
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(FocusHandle::named("field")));
}

#[test]
fn bindings_follow_context_and_actions_bubble() {
    let mut w = window(Controls::new(), 400, 300, 1.0);
    w.bind_keys([
        KeyBinding::new("primary+enter", Activate, None),
        KeyBinding::new("escape", Escape, Some("Page")),
    ]);
    // Nothing focused: the app's root element takes actions, with its
    // context.
    w.input(named(NamedKey::Escape));
    assert_eq!(w.state.actions, ["page-escape"]);
    w.state.actions.clear();
    w.click("b");
    w.input(named(NamedKey::Escape));
    // The inner handler passes it on to the page's.
    assert_eq!(w.state.actions, ["inner-escape", "page-escape"]);
    let primary = if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL
    };
    w.input(Input::Modifiers(primary));
    w.input(named(NamedKey::Enter));
    assert_eq!(w.state.actions.last(), Some(&"page"));
    // Ctrl+Enter is not Enter: no click.
    assert!(w.state.clicks.iter().all(|c| *c == "b") && w.state.clicks.len() == 1);
}

#[test]
fn a_text_input_takes_typing_ime_and_enter() {
    let mut w = window(Controls::new(), 400, 300, 1.0);
    w.click("field");
    assert_eq!(w.focused(), Some(FocusHandle::named("field")));
    for c in ["h", "i", " "] {
        w.input(typed(c));
    }
    w.input(named(NamedKey::Backspace));
    w.input(Input::Ime(Ime::Preedit("にほ".into(), Some((0, 6)))));
    w.frame();
    assert!(w.ime_area().is_some());
    w.input(Input::Ime(Ime::Commit("日本".into())));
    assert_eq!(w.state.field.text(), "hi日本");
    assert_eq!(w.state.changes, 5);
    w.input(named(NamedKey::Enter));
    assert_eq!(w.state.submitted.as_deref(), Some("hi日本"));
    // Space typed into the field is text, not a click.
    assert!(w.state.clicks.is_empty());
}

#[test]
fn messages_and_timers_run_handlers_and_dropping_a_timer_cancels_it() {
    struct Timed {
        fired: Vec<&'static str>,
        kept: Option<Timer>,
    }
    impl App for Timed {
        type Message = &'static str;
        fn view(&self, _: &ViewCx) -> Element<Self> {
            panel()
        }
        fn message(&mut self, m: &'static str, cx: &mut Cx<Self>) {
            match m {
                "start" => {
                    self.kept =
                        Some(cx.after(Duration::from_millis(150), |s, _| s.fired.push("kept")));
                    cx.after(Duration::from_millis(100), |s, _| s.fired.push("detached"))
                        .detach();
                    let _dropped =
                        cx.after(Duration::from_millis(50), |s, _| s.fired.push("dropped"));
                }
                "cancel" => self.kept = None,
                _ => {}
            }
        }
    }
    let mut w = window(
        Timed {
            fired: vec![],
            kept: None,
        },
        100,
        100,
        1.0,
    );
    let t0 = Instant::now();
    w.advance(t0);
    w.message("start");
    assert_eq!(w.next_deadline(), Some(t0 + Duration::from_millis(100)));
    w.advance(t0 + Duration::from_millis(120));
    assert_eq!(w.state.fired, ["detached"]);
    w.message("cancel");
    w.advance(t0 + Duration::from_secs(1));
    assert_eq!(w.state.fired, ["detached"]);
    assert_eq!(w.next_deadline(), None);
}

#[test]
fn before_view_keeps_timers_in_step_with_the_state() {
    /// Holds a deadline its messages move; `before_view` keeps a timer
    /// for it, as the GUI does for its model.
    struct Due {
        due: Option<Instant>,
        timer: Option<(Instant, Timer)>,
        fired: u32,
        prepared: u32,
    }
    impl App for Due {
        type Message = Option<Instant>;
        fn view(&self, _: &ViewCx) -> Element<Self> {
            panel()
        }
        fn message(&mut self, due: Option<Instant>, _: &mut Cx<Self>) {
            self.due = due;
        }
        fn before_view(&mut self, cx: &mut Cx<Self>) {
            self.prepared += 1;
            if self.timer.as_ref().map(|(t, _)| *t) == self.due {
                return;
            }
            self.timer = self.due.map(|t| {
                let delay = t.saturating_duration_since(cx.now());
                let timer = cx.after(delay, |s: &mut Self, _| {
                    s.fired += 1;
                    s.due = None;
                });
                (t, timer)
            });
        }
    }
    let mut w = window(
        Due {
            due: None,
            timer: None,
            fired: 0,
            prepared: 0,
        },
        100,
        100,
        1.0,
    );
    assert_eq!(w.state.prepared, 1, "once before the first build");
    let t0 = Instant::now();
    w.advance(t0);
    w.message(Some(t0 + Duration::from_millis(50)));
    assert_eq!(w.next_deadline(), None, "not before the next build");
    w.frame();
    assert_eq!(w.next_deadline(), Some(t0 + Duration::from_millis(50)));
    // Moved: the old timer goes with its handle.
    w.message(Some(t0 + Duration::from_millis(80)));
    w.frame();
    assert_eq!(w.next_deadline(), Some(t0 + Duration::from_millis(80)));
    w.advance(t0 + Duration::from_millis(100));
    w.frame();
    assert_eq!(w.state.fired, 1);
    assert_eq!(w.next_deadline(), None);
}

struct Rows {
    scroll: ScrollHandle,
    built: RefCell<Vec<usize>>,
    opened: Option<usize>,
}

impl App for Rows {
    type Message = ();

    fn view(&self, _: &ViewCx) -> Element<Self> {
        self.built.borrow_mut().clear();
        panel().fill().child(
            list(&self.scroll, 10_000, |s: &Self, i, _| {
                s.built.borrow_mut().push(i);
                panel()
                    .id(("row", i as u64))
                    .height(20.0)
                    .on_click(move |s: &mut Self, _| s.opened = Some(i))
                    .child(text(format!("Row {i}")))
            })
            .fill(),
        )
    }
}

#[test]
fn a_list_builds_only_the_rows_in_view_and_scrolls() {
    let mut w = window(
        Rows {
            scroll: ScrollHandle::new(),
            built: RefCell::default(),
            opened: None,
        },
        300,
        100,
        1.0,
    );
    // Row 0 to measure, then rows 0 to 4 in view (row 0 reused).
    assert_eq!(*w.state.built.borrow(), [0, 1, 2, 3, 4]);
    assert_eq!(w.state.scroll.max_offset(), 10_000.0 * 20.0 - 100.0);
    w.input(Input::PointerMoved { x: 10.0, y: 10.0 });
    w.input(Input::Wheel { dy: 50.0 });
    w.frame();
    assert_eq!(*w.state.built.borrow(), [0, 2, 3, 4, 5, 6, 7]);
    // Row 3 is at 60 - 50 = 10.
    assert_eq!(w.bounds_of(("row", 3)).unwrap().y, 10.0);
    w.input(Input::Pressed);
    w.input(Input::Released);
    // y 10 is 60 in the list: row 3.
    assert_eq!(w.state.opened, Some(3));
    w.state.scroll.reveal_row(9_999);
    w.input(Input::Wheel { dy: 0.0 });
    w.message(());
    w.frame();
    assert_eq!(w.state.scroll.offset(), w.state.scroll.max_offset());
    assert!(w.state.built.borrow().contains(&9_999));
    // The rows move as one scroll layer.
    assert_eq!(w.scene().layers.len(), 1);
    assert_eq!(w.scene().layers[0].offset_y, w.state.scroll.offset() as i32);
}

struct Page {
    scroll: ScrollHandle,
    handles: Vec<FocusHandle>,
    menu: bool,
    lost: u32,
}

impl App for Page {
    type Message = &'static str;

    fn view(&self, _: &ViewCx) -> Element<Self> {
        let mut page = panel().grow().scroll_y(&self.scroll).gap(10.0);
        for (i, h) in self.handles.iter().enumerate() {
            page = page.child(
                panel()
                    .id(("field", i as u64))
                    .focus(*h)
                    .tab_stop(true)
                    .height(40.0)
                    .background(BLUE),
            );
        }
        let menu = panel()
            .id("menu")
            .size(150.0, 80.0)
            .background(RED)
            .on_press_outside(|s: &mut Self, _| s.menu = false);
        panel()
            .fill()
            .child(
                panel()
                    .row()
                    .justify(ossteam_ui::style::Justify::End)
                    .child(
                        panel()
                            .size(40.0, 20.0)
                            .child(overlay(menu).hidden(!self.menu)),
                    ),
            )
            .child(page)
    }

    fn message(&mut self, m: &'static str, _: &mut Cx<Self>) {
        match m {
            "open" => self.menu = true,
            "drop-focused" => self.handles.truncate(1),
            _ => {}
        }
    }

    fn focus_lost(&mut self, cx: &mut Cx<Self>) {
        self.lost += 1;
        cx.focus(self.handles[0]);
    }
}

fn page() -> Page {
    Page {
        scroll: ScrollHandle::new(),
        handles: (0..10).map(|_| FocusHandle::new()).collect(),
        menu: false,
        lost: 0,
    }
}

#[test]
fn tab_scrolls_the_focused_element_into_view() {
    let mut w = window(page(), 300, 220, 1.0);
    for _ in 0..5 {
        w.input(named(NamedKey::Tab));
    }
    w.frame();
    // Field 4 is at 4 * 50 = 200 in the page, 40 high; the page shows
    // 200 (220 less the 20 high bar) and keeps 16 of margin.
    let b = w.bounds_of(("field", 4)).unwrap();
    assert_eq!(b.bottom(), 220.0 - 16.0);
    assert_eq!(w.state.scroll.offset(), 240.0 + 16.0 - 200.0);
}

/// A page with blocks above a control, and an error below it.
struct Growing {
    scroll: ScrollHandle,
    control: FocusHandle,
    above: usize,
    error: bool,
}

impl App for Growing {
    type Message = &'static str;

    fn view(&self, _: &ViewCx) -> Element<Self> {
        let mut page = panel().fill().scroll_y(&self.scroll);
        for i in 0..self.above {
            page = page.child(panel().id(("block", i as u64)).height(100.0));
        }
        page = page.child(
            panel()
                .id("control")
                .focus(self.control)
                .tab_stop(true)
                .height(40.0),
        );
        if self.error {
            page = page
                .child(panel().height(150.0))
                .child(panel().id("error").height(30.0).reveal_when_shown());
        }
        page.child(panel().height(1000.0))
    }

    fn message(&mut self, m: &'static str, _: &mut Cx<Self>) {
        match m {
            "grow" => self.above += 1,
            "fail" => self.error = true,
            _ => {}
        }
    }
}

#[test]
fn the_keyboard_focused_control_stays_in_view_until_the_page_scrolls_otherwise() {
    let mut w = window(
        Growing {
            scroll: ScrollHandle::new(),
            control: FocusHandle::new(),
            above: 3,
            error: false,
        },
        300,
        200,
        1.0,
    );
    w.input(named(NamedKey::Tab));
    w.frame();
    let shown = |w: &Window<Growing>| {
        let b = w.bounds_of("control").unwrap();
        b.y >= 0.0 && b.bottom() <= 200.0
    };
    assert!(shown(&w));
    // Content added above pushes the control down: the page follows it.
    w.message("grow");
    w.message("grow");
    w.frame();
    assert!(shown(&w), "{:?}", w.bounds_of("control"));
    assert_eq!(w.bounds_of("control").unwrap().bottom(), 200.0 - 16.0);

    // An error that the control's action brings up, below the edge: it
    // is shown, and kept in view from then on.
    w.message("fail");
    w.frame();
    let e = w.bounds_of("error").unwrap();
    assert_eq!(e.bottom(), 200.0 - 16.0, "the error is shown");
    w.message("grow");
    w.frame();
    let e = w.bounds_of("error").unwrap();
    assert_eq!(e.bottom(), 200.0 - 16.0, "and kept in view");

    // Scrolled by the wheel: the page lets the control go.
    w.input(Input::PointerMoved { x: 50.0, y: 50.0 });
    w.input(Input::Wheel { dy: -3.0 });
    w.frame();
    let offset = w.state.scroll.offset();
    w.message("grow");
    w.frame();
    assert_eq!(w.state.scroll.offset(), offset, "not scrolled back");
}

#[test]
fn an_overlay_stays_inside_the_window_and_closes_on_a_press_outside() {
    let mut w = window(page(), 300, 220, 1.0);
    w.message("open");
    w.frame();
    // Its anchor is at x 260; 150 wide, it moves left to 300 - 8 - 150.
    let m = w.bounds_of("menu").unwrap();
    assert_eq!((m.x, m.y), (142.0, 0.0f32.max(8.0)));
    // Drawn after (over) the page.
    let s = w.scene();
    let red = s
        .items
        .iter()
        .rposition(|i| matches!(i.primitive, Primitive::Quad(q) if q.background == RED));
    let blue = s
        .items
        .iter()
        .rposition(|i| matches!(i.primitive, Primitive::Quad(q) if q.background == BLUE));
    assert!(red > blue);
    w.input(Input::PointerMoved { x: 150.0, y: 30.0 });
    w.input(Input::Pressed);
    assert!(w.state.menu, "a press on the menu keeps it");
    w.input(Input::PointerMoved { x: 10.0, y: 200.0 });
    w.input(Input::Pressed);
    assert!(!w.state.menu);
}

#[test]
fn losing_the_focused_element_tells_the_app() {
    let mut w = window(page(), 300, 220, 1.0);
    w.input(named(NamedKey::Tab));
    w.input(named(NamedKey::Tab));
    w.message("drop-focused");
    w.frame();
    assert_eq!(w.state.lost, 1);
    assert_eq!(w.focused(), Some(w.state.handles[0]));
}

#[test]
fn hover_repaints_without_building_and_canvas_paints_in_device_pixels() {
    struct Hover;
    impl App for Hover {
        type Message = ();
        fn view(&self, _: &ViewCx) -> Element<Self> {
            panel()
                .fill()
                .padding(Edges::all(10.0))
                .child(
                    panel()
                        .id("h")
                        .size(50.0, 50.0)
                        .background(BLUE)
                        .hover_style(|s| s.background(RED)),
                )
                .child(
                    canvas(|b, scene, scale| {
                        scene.quad(Quad::filled(
                            Rect::new(b.x, b.y, 2.0 * scale, 2.0 * scale),
                            RING,
                        ));
                    })
                    .size(10.0, 10.0),
                )
        }
    }
    let mut w = window(Hover, 200, 200, 2.0);
    assert!(quad_with(w.scene(), RED).is_none());
    assert!(!w.needs_frame());
    w.input(Input::PointerMoved { x: 40.0, y: 40.0 });
    assert!(w.needs_frame());
    w.frame();
    assert!(quad_with(w.scene(), RED).is_some());
    // Moving within it changes nothing.
    assert!(!w.input(Input::PointerMoved { x: 50.0, y: 50.0 }));
    let ring = quad_with(w.scene(), RING).unwrap().bounds;
    assert_eq!(ring, Rect::new(20.0, 120.0, 4.0, 4.0));
}

#[test]
fn the_software_renderer_redraws_what_input_changed() {
    use ossteam_ui::render::software::Canvas;
    let mut w = window(Controls::new(), 400, 300, 1.0);
    let mut canvas = Canvas::new();
    canvas.draw(w.scene(), &w.text.glyphs);
    w.input(named(NamedKey::Tab));
    assert!(w.frame().is_some());
    let stats = canvas.draw(w.scene(), &w.text.glyphs);
    assert!(stats.pixels > 0, "the focus ring was not drawn");
}

#[test]
fn scrolling_to_the_focused_element_draws_what_a_full_redraw_draws() {
    use ossteam_ui::render::software::Canvas;
    let mut w = window(page(), 300, 220, 1.5);
    let mut canvas = Canvas::new();
    canvas.draw(w.scene(), &w.text.glyphs);
    for step in 0..9 {
        w.input(named(NamedKey::Tab));
        w.frame();
        canvas.draw(w.scene(), &w.text.glyphs);
        let mut fresh = Canvas::new();
        fresh.draw(w.scene(), &w.text.glyphs);
        let bad = canvas
            .pixels()
            .iter()
            .zip(fresh.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            bad,
            0,
            "step {step}: {bad} pixels differ, offset {}",
            w.state.scroll.offset()
        );
    }
}

/// A page that takes focus without being a tab stop, between two stops.
struct FocusedPage {
    focus_page: bool,
}

impl App for FocusedPage {
    type Message = ();

    fn before_view(&mut self, cx: &mut Cx<Self>) {
        if std::mem::take(&mut self.focus_page) {
            cx.focus(FocusHandle::named("page"));
        }
    }

    fn view(&self, _: &ViewCx) -> Element<Self> {
        let stop = |id: &'static str| {
            panel()
                .id(id)
                .focus(FocusHandle::named(id))
                .tab_stop(true)
                .size(20.0, 20.0)
        };
        panel()
            .fill()
            .child(stop("before"))
            .child(
                panel()
                    .id("page")
                    .focus(FocusHandle::named("page"))
                    .child(stop("inner-1"))
                    .child(stop("inner-2")),
            )
            .child(stop("after"))
    }
}

#[test]
fn tab_from_a_focused_page_goes_on_from_the_page() {
    let mut w = window(FocusedPage { focus_page: true }, 400, 300, 1.0);
    assert_eq!(w.focused(), Some(FocusHandle::named("page")));
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(FocusHandle::named("inner-1")));

    let mut w = window(FocusedPage { focus_page: true }, 400, 300, 1.0);
    w.input(Input::Modifiers(ModifiersState::SHIFT));
    w.input(named(NamedKey::Tab));
    assert_eq!(w.focused(), Some(FocusHandle::named("before")));
}
