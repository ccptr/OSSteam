// SPDX-License-Identifier: GPL-2.0-or-later
//! Frame times for the §20 library budget (`gui.md` §7). With
//! `OSSTEAM_GUI_BENCH=library-scroll` the Library page, once the library
//! has loaded, scrolls the grid and then the list from top to bottom, one
//! row per frame, prints how long the frames took and quits.
//!
//! A frame's work is the time from the start of the frame (when gpui runs
//! next-frame callbacks) to the end of the page's paint; its interval is
//! the time to the start of the next frame, which includes waiting for
//! the display.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const ENV: &str = "OSSTEAM_GUI_BENCH";
/// "No frame over 16 ms" (SPEC §20).
pub const BUDGET: Duration = Duration::from_millis(16);

/// Whether the library scroll was asked for.
pub fn library_scroll() -> bool {
    std::env::var_os(ENV).is_some_and(|v| v == "library-scroll")
}

/// The frames of one run.
#[derive(Default)]
pub struct Frames {
    start: Option<Instant>,
    /// Set by the page's last paint.
    painted: Rc<Cell<Option<Instant>>>,
    work: Vec<Duration>,
    intervals: Vec<Duration>,
}

impl Frames {
    /// The cell the end of the paint is written to.
    pub fn painted(&self) -> Rc<Cell<Option<Instant>>> {
        self.painted.clone()
    }

    /// A frame starts at `now`, which ends the previous one.
    pub fn begin(&mut self, now: Instant) {
        if let Some(start) = self.start {
            if let Some(end) = self.painted.take()
                && end >= start
            {
                self.work.push(end - start);
            }
            self.intervals.push(now - start);
        }
        self.start = Some(now);
    }

    /// `refresh` is the display's refresh interval, if known: an interval
    /// over one and a half of it is a missed frame.
    pub fn summary(&self, refresh: Option<Duration>) -> Summary {
        let mut work = self.work.clone();
        work.sort_unstable();
        let mut intervals = self.intervals.clone();
        intervals.sort_unstable();
        Summary {
            frames: work.len(),
            work_p50: percentile(&work, 50),
            work_p99: percentile(&work, 99),
            work_max: work.last().copied().unwrap_or_default(),
            over_budget: work.iter().filter(|&&w| w > BUDGET).count(),
            interval_p50: percentile(&intervals, 50),
            missed: refresh.map(|r| intervals.iter().filter(|&&i| i * 2 > r * 3).count()),
            refresh,
        }
    }
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
            "{} frames; work p50 {}, p99 {}, max {}, {} over {}; interval p50 {}",
            self.frames,
            ms(self.work_p50),
            ms(self.work_p99),
            ms(self.work_max),
            self.over_budget,
            ms(BUDGET),
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
