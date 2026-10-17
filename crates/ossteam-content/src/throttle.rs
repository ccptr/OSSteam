// SPDX-License-Identifier: GPL-2.0-or-later
//! The bandwidth limit shared by every download (`content.md` §7, CONT-7).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ossteam_types::CancelToken;

/// The longest one sleep lasts before cancellation is checked (CONC-2).
const SLICE: Duration = Duration::from_millis(250);

/// A token bucket. Readers take bytes before reading them; when the bucket
/// is in debt they sleep until it is not. The rate can change at any time.
#[derive(Debug)]
pub struct Throttle {
    /// Bytes per second; 0 is no limit.
    rate: AtomicU64,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// Bytes that may be read now; negative is debt.
    available: f64,
    last: Instant,
}

impl Throttle {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            state: Mutex::new(State {
                available: 0.0,
                last: Instant::now(),
            }),
        }
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Accounts for `n` bytes, sleeping while over the limit. Returns false
    /// if cancelled while waiting.
    pub fn take(&self, n: usize, cancel: &CancelToken) -> bool {
        let wait = {
            let rate = self.rate();
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if rate == 0 {
                s.available = 0.0;
                s.last = now;
                return true;
            }
            let rate = rate as f64;
            // At most one second's worth may build up while idle.
            let elapsed = now.duration_since(s.last).as_secs_f64();
            s.available = (s.available + elapsed * rate).min(rate);
            s.last = now;
            s.available -= n as f64;
            if s.available >= 0.0 {
                return true;
            }
            Duration::from_secs_f64(-s.available / rate)
        };
        let until = Instant::now() + wait;
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(SLICE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        let t = Throttle::new(0);
        let start = Instant::now();
        for _ in 0..1000 {
            assert!(t.take(1 << 20, &CancelToken::new()));
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn a_limit_paces_reads() {
        // 100 KiB/s; 30 KiB beyond an empty bucket takes ~0.3 s.
        let t = Throttle::new(100 * 1024);
        let start = Instant::now();
        for _ in 0..3 {
            assert!(t.take(10 * 1024, &CancelToken::new()));
        }
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(250), "{took:?}");
        assert!(took < Duration::from_millis(1000), "{took:?}");
    }

    #[test]
    fn cancellation_ends_the_wait() {
        let t = Throttle::new(1);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(!t.take(1 << 20, &cancel));
    }
}
