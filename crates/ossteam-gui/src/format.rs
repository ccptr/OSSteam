// SPDX-License-Identifier: GPL-2.0-or-later
//! Words and numbers as the GUI shows them: the same units and names as
//! the CLI prints (LEAF-3).

use std::time::Duration;

use ossteam_ipc::v1::{self, JobKind, JobPhase, JobState};

/// A byte count in binary units.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// A duration as hours, minutes and seconds, largest two units.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s} s"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

/// A Unix time as a UTC calendar date, `YYYY-MM-DD`.
pub fn date(unix: u64) -> String {
    // Days to civil date, after Howard Hinnant's `civil_from_days`.
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn kind(k: JobKind) -> &'static str {
    match k {
        JobKind::Install => "install",
        JobKind::Update => "update",
        JobKind::Verify => "verify",
        JobKind::Uninstall => "uninstall",
        JobKind::Unspecified => "unknown",
    }
}

pub fn state(s: JobState) -> &'static str {
    match s {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Paused => "paused",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Unspecified => "unknown",
    }
}

pub fn phase(p: JobPhase) -> &'static str {
    match p {
        JobPhase::Preparing => "preparing",
        JobPhase::Checking => "checking",
        JobPhase::Copying => "copying",
        JobPhase::Downloading => "downloading",
        JobPhase::Finishing => "finishing",
        JobPhase::Unspecified => "starting",
    }
}

/// The fraction of a job done, for its progress bar: downloaded bytes
/// while downloading, file bytes otherwise.
pub fn fraction(p: &v1::JobProgressInfo) -> Option<f32> {
    let (done, total) = if p.phase() == JobPhase::Downloading {
        (p.downloaded, p.download_total)
    } else {
        (p.bytes_done, p.bytes_total)
    };
    (total > 0).then(|| done.min(total) as f32 / total as f32)
}

/// A job's progress in a few words, as the CLI prints it.
pub fn progress(p: &v1::JobProgressInfo) -> String {
    let percent = |done: u64, total: u64| {
        if total == 0 {
            String::new()
        } else {
            format!(" {:.0}%", done.min(total) as f64 * 100.0 / total as f64)
        }
    };
    let name = phase(p.phase());
    match p.phase() {
        JobPhase::Downloading => format!(
            "{name}{} ({} of {})",
            percent(p.downloaded, p.download_total),
            size(p.downloaded),
            size(p.download_total)
        ),
        JobPhase::Checking | JobPhase::Copying => {
            format!("{name}{}", percent(p.bytes_done, p.bytes_total))
        }
        _ => name.to_owned(),
    }
}

pub fn persona_state(s: v1::PersonaState) -> &'static str {
    match s {
        v1::PersonaState::Online => "Online",
        v1::PersonaState::Invisible => "Invisible",
        v1::PersonaState::Offline => "Offline",
        v1::PersonaState::Unspecified => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_durations_and_progress() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1_048_576), "1.0 MiB");
        assert_eq!(duration(Duration::from_secs(42)), "42 s");
        assert_eq!(duration(Duration::from_secs(125)), "2 min 5 s");
        assert_eq!(duration(Duration::from_secs(7322)), "2 h 2 min");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        let p = v1::JobProgressInfo {
            phase: JobPhase::Downloading as i32,
            download_total: 4 << 20,
            downloaded: 1 << 20,
            ..Default::default()
        };
        assert_eq!(progress(&p), "downloading 25% (1.0 MiB of 4.0 MiB)");
        assert_eq!(fraction(&p), Some(0.25));
        assert_eq!(fraction(&v1::JobProgressInfo::default()), None);
    }
}
