// SPDX-License-Identifier: GPL-2.0-or-later
//! Frame times for the §20 library budget (`gui.md` §7). With
//! `OSSTEAM_GUI_BENCH=library-scroll` the Library page, once the whole
//! library is shown, scrolls the grid and then the list from top to
//! bottom, one row per frame, prints how long the frames took and quits.
//!
//! A frame's work is the time from its start (the GUI's `before_view`)
//! to the end of the page's paint; its interval is the time to the start
//! of the next frame, which includes waiting for the display. The time
//! to the end of the present, the renderer's work included, is printed
//! too, but the budget is on the work.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ossteam_ui::element::canvas;
use ossteam_ui::style::{Edges, Length, Styled};
use ossteam_ui::window::Cx;

use crate::gui::{Gui, Page};
use crate::pages::library::{self, Layout};
use crate::widgets::El;

pub const ENV: &str = "OSSTEAM_GUI_BENCH";
/// "No frame over 16 ms" (SPEC §20).
pub const BUDGET: Duration = Duration::from_millis(16);

/// The frames of one run.
#[derive(Default)]
pub struct Frames {
    start: Option<Instant>,
    /// Set by the page's last paint.
    painted: Rc<Cell<Option<Instant>>>,
    presented: Option<Instant>,
    work: Vec<Duration>,
    to_present: Vec<Duration>,
    intervals: Vec<Duration>,
}

impl Frames {
    /// A frame starts at `now`, which ends the previous one.
    pub fn begin(&mut self, now: Instant) {
        if let Some(start) = self.start {
            let since = |t: Option<Instant>| t.filter(|&t| t >= start).map(|t| t - start);
            if let Some(work) = since(self.painted.take()) {
                self.work.push(work);
            }
            if let Some(d) = since(self.presented.take()) {
                self.to_present.push(d);
            }
            self.intervals.push(now - start);
        }
        self.start = Some(now);
    }

    /// The frame was presented at `now`.
    pub fn presented(&mut self, now: Instant) {
        self.presented = Some(now);
    }

    /// Forgets the frames, for the next run; the page keeps its cell.
    pub fn reset(&mut self) {
        let painted = self.painted.clone();
        painted.set(None);
        *self = Self {
            painted,
            ..Self::default()
        };
    }

    /// `refresh` is the display's refresh interval, if known: an interval
    /// over one and a half of it is a missed frame.
    pub fn summary(&self, refresh: Option<Duration>) -> Summary {
        let mut work = self.work.clone();
        work.sort_unstable();
        let mut intervals = self.intervals.clone();
        intervals.sort_unstable();
        let mut to_present = self.to_present.clone();
        to_present.sort_unstable();
        Summary {
            frames: work.len(),
            work_p50: percentile(&work, 50),
            work_p99: percentile(&work, 99),
            work_max: work.last().copied().unwrap_or_default(),
            over_budget: work.iter().filter(|&&w| w > BUDGET).count(),
            interval_p50: percentile(&intervals, 50),
            present_p99: percentile(&to_present, 99),
            present_max: to_present.last().copied().unwrap_or_default(),
            missed: refresh.map(|r| intervals.iter().filter(|&&i| i * 2 > r * 3).count()),
            refresh,
        }
    }
}

/// The scroll of `OSSTEAM_GUI_BENCH=library-scroll`: the grid, then the
/// list.
#[derive(Default)]
pub struct LibraryBench {
    running: bool,
    /// A frame began and was not presented yet.
    in_frame: bool,
    /// The next frame changes the layout: it is not counted.
    skip: bool,
    frames: Frames,
    results: Vec<(&'static str, Summary)>,
}

impl LibraryBench {
    /// A bench, if the environment asks for one.
    pub fn wanted() -> Option<Self> {
        std::env::var_os(ENV)
            .is_some_and(|v| v == "library-scroll")
            .then(Self::default)
    }

    /// Painted after the rest of the page: it notes the end of the
    /// page's paint.
    pub fn painted(&self) -> El {
        let painted = self.frames.painted.clone();
        canvas(move |_, _, _| painted.set(Some(Instant::now())))
            .absolute()
            .size(1.0, 1.0)
            .inset(Edges {
                left: Length::Px(0.0),
                top: Length::Px(0.0),
                ..Edges::all(Length::Auto)
            })
    }
}

/// Starts the bench once the whole library is shown, and starts the
/// count of each frame it scrolls.
pub fn library_before_view(g: &mut Gui, cx: &mut Cx<Gui>) {
    let complete = g
        .model()
        .and_then(|m| m.store.library.as_ref())
        .is_some_and(|l| l.complete);
    let on_library = g.page == Page::Library && g.app.is_none() && !g.showing_login();
    let apps = library::shown_count(g);
    let Some(b) = &mut g.library.bench else {
        return;
    };
    if b.running {
        if !b.in_frame && !std::mem::take(&mut b.skip) {
            b.in_frame = true;
            b.frames.begin(Instant::now());
        }
        return;
    }
    if !complete || !on_library || apps == 0 {
        return;
    }
    b.running = true;
    println!("library scroll: {apps} apps");
    g.library.layout = Layout::Grid;
    g.library.scroll.set_offset(0.0);
    cx.on_next_frame(step);
}

/// After each present: one row further, until the bottom.
fn step(g: &mut Gui, cx: &mut Cx<Gui>) {
    let now = Instant::now();
    let layout = g.library.layout;
    let scroll = g.library.scroll.clone();
    let Some(b) = &mut g.library.bench else {
        return;
    };
    if std::mem::take(&mut b.in_frame) {
        b.frames.presented(now);
    }
    let (max, y) = (scroll.max_offset(), scroll.offset());
    if max > 0.0 && y < max {
        scroll.set_offset((y + library::row_height(layout)).min(max));
    } else {
        let name = match layout {
            Layout::Grid => "grid",
            Layout::List => "list",
        };
        let summary = b.frames.summary(cx.refresh_interval());
        b.frames.reset();
        println!("library scroll, {name}: {summary}");
        b.results.push((name, summary));
        if layout == Layout::List {
            let met = b.results.iter().all(|(_, s)| s.met());
            println!(
                "library scroll: budget {}",
                if met { "met" } else { "missed" }
            );
            cx.quit();
            return;
        }
        b.skip = true;
        g.library.layout = Layout::List;
        scroll.set_offset(0.0);
    }
    cx.on_next_frame(step);
}

/// The value below which `p` percent of `sorted` lie (nearest rank).
fn percentile(sorted: &[Duration], p: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (sorted.len() * p).div_ceil(100).max(1);
    sorted[rank - 1]
}

#[derive(Debug, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub work_p50: Duration,
    pub work_p99: Duration,
    pub work_max: Duration,
    pub over_budget: usize,
    pub interval_p50: Duration,
    /// To the end of the present: the renderer's work too.
    pub present_p99: Duration,
    pub present_max: Duration,
    pub missed: Option<usize>,
    pub refresh: Option<Duration>,
}

impl Summary {
    pub fn met(&self) -> bool {
        self.frames > 0 && self.over_budget == 0 && self.missed.unwrap_or(0) == 0
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} frames; work p50 {}, p99 {}, max {}, {} over {}; to present p99 {}, max {}; \
             interval p50 {}",
            self.frames,
            ms(self.work_p50),
            ms(self.work_p99),
            ms(self.work_max),
            self.over_budget,
            ms(BUDGET),
            ms(self.present_p99),
            ms(self.present_max),
            ms(self.interval_p50),
        )?;
        match (self.missed, self.refresh) {
            (Some(n), Some(r)) => write!(f, ", {n} missed at {}", ms(r)),
            _ => write!(f, ", refresh interval unknown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(frames: &[(u64, u64)]) -> Frames {
        // (work, interval) in ms, each frame starting where the last ended.
        let t0 = Instant::now();
        let mut f = Frames::default();
        let mut t = t0;
        for &(work, interval) in frames {
            f.begin(t);
            f.painted.set(Some(t + Duration::from_millis(work)));
            f.presented(t + Duration::from_millis(work + 1));
            t += Duration::from_millis(interval);
        }
        f.begin(t);
        f
    }

    #[test]
    fn frames_are_measured_from_their_start() {
        let f = run(&[(2, 16), (3, 17), (20, 33)]);
        let s = f.summary(Some(Duration::from_micros(16_667)));
        assert_eq!(s.frames, 3);
        assert_eq!(s.work_p50, Duration::from_millis(3));
        assert_eq!(s.work_max, Duration::from_millis(20));
        assert_eq!(s.present_max, Duration::from_millis(21));
        assert_eq!(s.over_budget, 1);
        assert_eq!(s.interval_p50, Duration::from_millis(17));
        assert_eq!(s.missed, Some(1));
        assert!(!s.met());
    }

    #[test]
    fn a_frame_without_a_paint_has_no_work() {
        let t0 = Instant::now();
        let mut f = Frames::default();
        f.begin(t0);
        f.begin(t0 + Duration::from_millis(16));
        let s = f.summary(None);
        assert_eq!(s.frames, 0);
        assert_eq!(s.missed, None);
        assert!(!s.met(), "no frames measured is not a pass");
    }

    #[test]
    fn smooth_frames_meet_the_budget() {
        let f = run(&[(4, 16); 100]);
        let s = f.summary(Some(Duration::from_micros(16_667)));
        assert!(s.met(), "{s}");
        assert_eq!(s.work_p99, Duration::from_millis(4));
    }

    #[test]
    fn nearest_rank_percentiles() {
        let d: Vec<_> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&d, 50), Duration::from_millis(50));
        assert_eq!(percentile(&d, 99), Duration::from_millis(99));
        assert_eq!(percentile(&d[..1], 99), Duration::from_millis(1));
        assert_eq!(percentile(&[], 50), Duration::ZERO);
    }
}
