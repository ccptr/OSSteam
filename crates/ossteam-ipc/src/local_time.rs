// SPDX-License-Identifier: GPL-2.0-or-later
//! Times as the leaf clients show them: in the system's local time zone,
//! or in UTC when it cannot be read (SPEC CLI-6).

/// A moment in a time zone, to the minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// `UTC`, the zone's abbreviation, or `UTC±HH:MM`.
    pub zone: String,
}

impl LocalTime {
    /// `unix` in the system's time zone, or in UTC if it cannot be read.
    pub fn at(unix: u64) -> Self {
        let secs = i64::try_from(unix).unwrap_or(i64::MAX);
        match os::zone_at(secs) {
            Some((offset, abbr)) => Self::at_offset(unix, offset, abbr.as_deref()),
            None => Self::at_offset(unix, 0, None),
        }
    }

    /// `unix` at `offset` seconds east of UTC, in a zone abbreviated
    /// `abbr`.
    pub fn at_offset(unix: u64, offset: i32, abbr: Option<&str>) -> Self {
        let secs = i64::try_from(unix)
            .unwrap_or(i64::MAX / 2)
            .saturating_add(i64::from(offset));
        let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
        let of_day = secs.rem_euclid(86_400);
        Self {
            year,
            month,
            day,
            hour: (of_day / 3600) as u32,
            minute: (of_day / 60 % 60) as u32,
            zone: zone_label(offset, abbr),
        }
    }

    /// `YYYY-MM-DD`.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `HH:MM ZONE`, with the date in front when it is not `today`'s.
    pub fn clock(&self, today: &LocalTime) -> String {
        let hm = format!("{:02}:{:02} {}", self.hour, self.minute, self.zone);
        if (self.year, self.month, self.day) == (today.year, today.month, today.day) {
            hm
        } else {
            format!("{} {hm}", self.date())
        }
    }
}

/// `unix` as `HH:MM ZONE` in the local time zone, with the local date
/// when it is not today.
pub fn clock(unix: u64) -> String {
    clock_at(unix, now())
}

/// Like [`clock`], with the date shown when it is not `now`'s.
pub fn clock_at(unix: u64, now: u64) -> String {
    LocalTime::at(unix).clock(&LocalTime::at(now))
}

/// `unix`'s local date, `YYYY-MM-DD`.
pub fn date(unix: u64) -> String {
    LocalTime::at(unix).date()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `UTC` at offset zero, a letters-only abbreviation, or the offset.
fn zone_label(offset: i32, abbr: Option<&str>) -> String {
    if offset == 0 {
        return "UTC".to_owned();
    }
    match abbr {
        Some(a) if !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphabetic()) => a.to_owned(),
        _ => {
            let sign = if offset < 0 { '-' } else { '+' };
            let m = offset.unsigned_abs() / 60;
            format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
        }
    }
}

/// Days since 1970-01-01 to a civil date, after Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A civil date to days since 1970-01-01, after `days_from_civil`.
#[cfg(any(windows, test))]
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(unix)]
mod os {
    use std::ffi::CStr;

    unsafe extern "C" {
        // POSIX; the libc crate does not declare it for Unix.
        fn tzset();
    }

    /// The offset east of UTC at `unix` and the zone's abbreviation, as
    /// the C library has them from `TZ` or `/etc/localtime`.
    pub fn zone_at(unix: i64) -> Option<(i32, Option<String>)> {
        let t = libc::time_t::try_from(unix).ok()?;
        // SAFETY: tm is plain data; localtime_r writes it and returns
        // null on failure. tzset and localtime_r read the environment,
        // which the leaf clients never change.
        let tm = unsafe {
            tzset();
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return None;
            }
            tm
        };
        let offset = i32::try_from(tm.tm_gmtoff).ok()?;
        let abbr = (!tm.tm_zone.is_null())
            // SAFETY: a non-null tm_zone points to a NUL-terminated
            // string the C library keeps.
            .then(|| unsafe { CStr::from_ptr(tm.tm_zone) })
            .and_then(|s| s.to_str().ok())
            .map(str::to_owned);
        Some((offset, abbr))
    }
}

#[cfg(windows)]
mod os {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

    /// The offset east of UTC at `unix` in the system's time zone. Its
    /// names are not abbreviations, so there is none.
    pub fn zone_at(unix: i64) -> Option<(i32, Option<String>)> {
        let (year, month, day) = super::civil_from_days(unix.div_euclid(86_400));
        let of_day = unix.rem_euclid(86_400);
        let utc = SYSTEMTIME {
            wYear: u16::try_from(year).ok()?,
            wMonth: month as u16,
            wDayOfWeek: 0,
            wDay: day as u16,
            wHour: (of_day / 3600) as u16,
            wMinute: (of_day / 60 % 60) as u16,
            wSecond: (of_day % 60) as u16,
            wMilliseconds: 0,
        };
        let mut local = utc;
        // SAFETY: both point to valid SYSTEMTIMEs; a null zone means the
        // system's current one.
        if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0 {
            return None;
        }
        let local_secs = super::days_from_civil(
            i64::from(local.wYear),
            u32::from(local.wMonth),
            u32::from(local.wDay),
        ) * 86_400
            + i64::from(local.wHour) * 3600
            + i64::from(local.wMinute) * 60
            + i64::from(local.wSecond);
        let offset = i32::try_from(local_secs - unix).ok()?;
        Some((offset, None))
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    pub fn zone_at(_: i64) -> Option<(i32, Option<String>)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-01-15 12:00:00 UTC and 2026-07-15 12:00:00 UTC.
    const WINTER: u64 = 1_768_478_400;
    const SUMMER: u64 = 1_784_116_800;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        for days in [-800_000, -1, 0, 59, 60, 11_016, 20_468, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn zones_are_labelled() {
        let t = LocalTime::at_offset(WINTER, 3600, Some("CET"));
        assert_eq!((t.hour, t.minute, t.zone.as_str()), (13, 0, "CET"));
        let t = LocalTime::at_offset(WINTER, 19_800, Some("IST"));
        assert_eq!((t.hour, t.minute), (17, 30));
        assert_eq!(
            LocalTime::at_offset(WINTER, 10_800, Some("+03")).zone,
            "UTC+03:00"
        );
        assert_eq!(
            LocalTime::at_offset(WINTER, -34_200, None).zone,
            "UTC-09:30"
        );
        // A TZ the C library does not know gives offset zero and its own
        // text: that is UTC.
        assert_eq!(LocalTime::at_offset(WINTER, 0, Some("Nowhere")).zone, "UTC");
    }

    #[test]
    fn the_date_shows_when_it_is_not_today() {
        let today = LocalTime::at_offset(WINTER, -25_200, Some("MST"));
        let later = LocalTime::at_offset(WINTER + 3600, -25_200, Some("MST"));
        assert_eq!(later.clock(&today), "06:00 MST");
        // 18:00 UTC is still the 15th in UTC, but the 16th in UTC+08:00.
        let today = LocalTime::at_offset(WINTER, 28_800, Some("AWST"));
        let evening = LocalTime::at_offset(WINTER + 6 * 3600, 28_800, Some("AWST"));
        assert_eq!(evening.clock(&today), "2026-01-16 02:00 AWST");
    }

    /// Run by [`the_system_zone_is_read`] in a child process with `TZ`
    /// set: `OSSTEAM_TZ_EXPECT` is `winter|summer` clocks as `HH:MM ZONE`.
    #[test]
    fn tz_child() {
        let Ok(expect) = std::env::var("OSSTEAM_TZ_EXPECT") else {
            return;
        };
        let got = format!(
            "{} {:02}:{:02} {}|{:02}:{:02} {}",
            std::env::var("TZ").unwrap_or_default(),
            LocalTime::at(WINTER).hour,
            LocalTime::at(WINTER).minute,
            LocalTime::at(WINTER).zone,
            LocalTime::at(SUMMER).hour,
            LocalTime::at(SUMMER).minute,
            LocalTime::at(SUMMER).zone,
        );
        let tz = std::env::var("TZ").unwrap_or_default();
        assert_eq!(got, format!("{tz} {expect}"));
    }

    /// `TZ` sets the zone on Linux and macOS. Windows has its own zone
    /// setting and ignores `TZ`; Wine reads `TZ` from the Unix side when a
    /// process is started from there, so the Windows build is checked by
    /// running [`tz_child`] under Wine (`AGENTS/testing/windows-wine.md`).
    #[cfg(unix)]
    #[test]
    fn the_system_zone_is_read() {
        let cases = [
            ("Europe/Berlin", "13:00 CET|14:00 CEST"),
            ("America/New_York", "07:00 EST|08:00 EDT"),
            ("Asia/Kolkata", "17:30 IST|17:30 IST"),
            ("UTC", "12:00 UTC|12:00 UTC"),
            ("", "12:00 UTC|12:00 UTC"),
        ];
        for (tz, expect) in cases {
            child(tz, expect);
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_system_zone_is_read() {
        let (offset, abbr) = os::zone_at(WINTER as i64).expect("the system's zone");
        assert_eq!(offset % 900, 0, "{offset}");
        assert!(offset.abs() <= 14 * 3600, "{offset}");
        assert_eq!(abbr, None);
    }

    /// Runs [`tz_child`] with `TZ` set.
    #[cfg(unix)]
    fn child(tz: &str, expect: &str) {
        let exe = std::env::current_exe().expect("the test binary");
        let out = std::process::Command::new(&exe)
            .args(["--exact", "local_time::tests::tz_child", "--nocapture"])
            .env("TZ", tz)
            .env("OSSTEAM_TZ_EXPECT", expect)
            .output()
            .expect("run the child");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "TZ={tz:?}: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// On Unix a `TZ` that names no zone falls back to UTC.
    #[cfg(unix)]
    #[test]
    fn an_unknown_zone_is_utc() {
        for tz in ["Nowhere/Fake", "garbage!!"] {
            child(tz, "12:00 UTC|12:00 UTC");
        }
    }
}
