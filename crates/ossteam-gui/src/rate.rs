// SPDX-License-Identifier: GPL-2.0-or-later
//! Download rates from `JobProgress` events (`gui.md` §4.4). They are view
//! state: derived on this side from what the daemon reported.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ossteam_ipc::v1::{self, JobPhase};

/// The window rates are measured over, and how long a job must be sampled
/// before its time left is shown.
pub const WINDOW: Duration = Duration::from_secs(3);

#[derive(Debug, Default)]
pub struct Rate {
    phase: i32,
    /// When sampling of this phase began.
    since: Option<Instant>,
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    /// Records `p` as reported at `at`. The measured value is `downloaded`
    /// while downloading and file bytes otherwise; a new phase starts over.
    pub fn sample(&mut self, at: Instant, p: &v1::JobProgressInfo) {
        let value = measured(p);
        let restart =
            p.phase != self.phase || self.samples.back().is_some_and(|&(_, last)| value < last);
        if restart {
            self.phase = p.phase;
            self.since = Some(at);
            self.samples.clear();
        }
        self.samples.push_back((at, value));
        while self
            .samples
            .front()
            .is_some_and(|&(t, _)| at.duration_since(t) > WINDOW)
            && self.samples.len() > 2
        {
            self.samples.pop_front();
        }
    }

    /// Bytes per second over the window.
    pub fn per_second(&self) -> Option<f64> {
        let (&(t0, v0), &(t1, v1)) = (self.samples.front()?, self.samples.back()?);
        let secs = t1.duration_since(t0).as_secs_f64();
        (secs > 0.0).then(|| (v1 - v0) as f64 / secs)
    }

    /// The time left for `remaining` bytes, once the phase has been sampled
    /// for the whole window.
    pub fn time_left(&self, now: Instant, remaining: u64) -> Option<Duration> {
        if now.duration_since(self.since?) < WINDOW {
            return None;
        }
        let rate = self.per_second()?;
        (rate > 0.0).then(|| Duration::from_secs_f64(remaining as f64 / rate))
    }
}

/// The bytes a rate is measured in, and how many are left.
pub fn measured(p: &v1::JobProgressInfo) -> u64 {
    if p.phase() == JobPhase::Downloading {
        p.downloaded
    } else {
        p.bytes_done
    }
}

pub fn remaining(p: &v1::JobProgressInfo) -> u64 {
    if p.phase() == JobPhase::Downloading {
        p.download_total.saturating_sub(p.downloaded)
    } else {
        p.bytes_total.saturating_sub(p.bytes_done)
    }
}

/// The rates of every job seen to make progress.
#[derive(Debug, Default)]
pub struct Rates(HashMap<u64, Rate>);

impl Rates {
    pub fn sample(&mut self, job_id: u64, at: Instant, p: &v1::JobProgressInfo) {
        self.0.entry(job_id).or_default().sample(at, p);
    }

    pub fn get(&self, job_id: u64) -> Option<&Rate> {
        self.0.get(&job_id)
    }

    /// Forgets the jobs `keep` rejects.
    pub fn retain(&mut self, mut keep: impl FnMut(u64) -> bool) {
        self.0.retain(|&id, _| keep(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downloading(downloaded: u64) -> v1::JobProgressInfo {
        v1::JobProgressInfo {
            phase: JobPhase::Downloading as i32,
            download_total: 10_000,
            downloaded,
            bytes_done: 1,
            ..Default::default()
        }
    }

    #[test]
    fn rate_over_the_window_and_time_left_after_it() {
        let t = Instant::now();
        let mut r = Rate::default();
        r.sample(t, &downloading(0));
        assert_eq!(r.per_second(), None);
        r.sample(t + Duration::from_secs(1), &downloading(1000));
        assert_eq!(r.per_second(), Some(1000.0));
        assert_eq!(r.time_left(t + Duration::from_secs(1), 9000), None);
        r.sample(t + Duration::from_secs(3), &downloading(3000));
        assert_eq!(
            r.time_left(t + Duration::from_secs(3), 7000),
            Some(Duration::from_secs(7))
        );
        // Older samples leave the window.
        r.sample(t + Duration::from_secs(5), &downloading(7000));
        assert_eq!(r.per_second(), Some(2000.0));
    }

    #[test]
    fn a_new_phase_starts_over() {
        let t = Instant::now();
        let mut r = Rate::default();
        r.sample(t, &downloading(0));
        r.sample(t + Duration::from_secs(4), &downloading(4000));
        let checking = v1::JobProgressInfo {
            phase: JobPhase::Checking as i32,
            bytes_total: 100,
            bytes_done: 10,
            ..Default::default()
        };
        r.sample(t + Duration::from_secs(5), &checking);
        assert_eq!(r.per_second(), None);
        assert_eq!(r.time_left(t + Duration::from_secs(5), 90), None);
        assert_eq!(remaining(&checking), 90);
    }
}
