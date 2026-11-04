// SPDX-License-Identifier: GPL-2.0-or-later
//! Cookie containers (`docs/spec/steamworks/isteamhttp.md` §7): the
//! storage and matching rules of RFC 6265 §5, in memory.

use std::time::{SystemTime, UNIX_EPOCH};

use ureq::http::Uri;

/// The limits of §7.
const MAX_COOKIES: usize = 3000;
const MAX_COOKIE: usize = 4096;

/// Unix seconds now.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

struct Cookie {
    name: String,
    value: String,
    /// Lower case; the host itself for a host-only cookie.
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    /// Unix seconds; `None` lasts as long as the container.
    expires: Option<i64>,
    /// The order cookies were first stored in.
    created: u64,
}

/// One container.
pub struct Jar {
    /// Responses may add and change cookies.
    pub allow_modify: bool,
    cookies: Vec<Cookie>,
    next: u64,
}

impl Jar {
    pub fn new(allow_modify: bool) -> Self {
        Self {
            allow_modify,
            cookies: Vec::new(),
            next: 0,
        }
    }

    /// Stores `Set-Cookie: line` received from `uri` (§5.2, §5.3).
    /// Returns whether it was taken.
    pub fn store(&mut self, uri: &Uri, line: &str, now: i64) -> bool {
        let Some(host) = uri.host().map(str::to_ascii_lowercase) else {
            return false;
        };
        if line.len() > MAX_COOKIE {
            return false;
        }
        let mut parts = line.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else {
            return false;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            return false;
        }
        let mut expires = None;
        let mut max_age = None;
        let mut domain = None;
        let mut path = None;
        let mut secure = false;
        for attr in parts {
            let (key, val) = match attr.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => (attr.trim(), ""),
            };
            match key.to_ascii_lowercase().as_str() {
                "expires" => {
                    if let Some(t) = parse_date(val) {
                        expires = Some(t);
                    }
                }
                "max-age" => {
                    let digits = val.strip_prefix('-').unwrap_or(val);
                    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                        max_age = Some(val.parse::<i64>().unwrap_or(if val.starts_with('-') {
                            i64::MIN
                        } else {
                            i64::MAX
                        }));
                    }
                }
                "domain" if !val.is_empty() => {
                    domain = Some(val.trim_start_matches('.').to_ascii_lowercase());
                }
                "path" => {
                    path = val.starts_with('/').then(|| val.to_owned());
                }
                "secure" => secure = true,
                _ => {}
            }
        }
        let expires = match max_age {
            Some(a) if a <= 0 => Some(i64::MIN),
            Some(a) => Some(now.saturating_add(a)),
            None => expires,
        };
        let (domain, host_only) = match domain {
            Some(d) if d == host => (d, true),
            // No public suffix list: a domain without a dot inside is
            // refused (§7).
            Some(d) if !d.contains('.') => return false,
            Some(d) if domain_matches(&host, &d) => (d, false),
            Some(_) => return false,
            None => (host, true),
        };
        let path = path.unwrap_or_else(|| default_path(uri.path()));
        let old = self
            .cookies
            .iter()
            .position(|c| c.name == name && c.domain == domain && c.path == path);
        let created = match old {
            Some(i) => self.cookies.remove(i).created,
            None => {
                self.next += 1;
                self.next
            }
        };
        if expires.is_some_and(|e| e <= now) {
            return true;
        }
        if self.cookies.len() >= MAX_COOKIES
            && let Some(oldest) = self
                .cookies
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| c.created)
                .map(|(i, _)| i)
        {
            self.cookies.remove(oldest);
        }
        self.cookies.push(Cookie {
            name: name.to_owned(),
            value: value.to_owned(),
            domain,
            host_only,
            path,
            secure,
            expires,
            created,
        });
        true
    }

    /// The `Cookie` header for a request to `uri` (§5.4), if any cookie
    /// matches.
    pub fn header(&mut self, uri: &Uri, now: i64) -> Option<String> {
        self.cookies.retain(|c| c.expires.is_none_or(|e| e > now));
        let host = uri.host()?.to_ascii_lowercase();
        let https = uri.scheme_str() == Some("https");
        let path = if uri.path().is_empty() {
            "/"
        } else {
            uri.path()
        };
        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                let domain = if c.host_only {
                    c.domain == host
                } else {
                    domain_matches(&host, &c.domain)
                };
                domain && path_matches(path, &c.path) && (https || !c.secure)
            })
            .collect();
        if matching.is_empty() {
            return None;
        }
        matching.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.created.cmp(&b.created))
        });
        Some(
            matching
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

/// RFC 6265 §5.1.3.
fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
            && host.parse::<std::net::IpAddr>().is_err())
}

/// RFC 6265 §5.1.4.
fn path_matches(path: &str, cookie: &str) -> bool {
    path == cookie
        || (path.starts_with(cookie)
            && (cookie.ends_with('/') || path.as_bytes().get(cookie.len()) == Some(&b'/')))
}

/// RFC 6265 §5.1.4: the request path up to its last `/`.
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(i) if path.starts_with('/') => path[..i].to_owned(),
        _ => "/".to_owned(),
    }
}

/// RFC 6265 §5.1.1: a cookie date, as Unix seconds.
pub(super) fn parse_date(s: &str) -> Option<i64> {
    let delimiter = |c: char| {
        c == '\t'
            || (' '..='/').contains(&c)
            || (';'..='@').contains(&c)
            || ('['..='`').contains(&c)
            || ('{'..='~').contains(&c)
    };
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in s.split(delimiter).filter(|t| !t.is_empty()) {
        let digits = |t: &str, min: usize, max: usize| {
            let n = t.bytes().take_while(u8::is_ascii_digit).count();
            (min..=max)
                .contains(&n)
                .then(|| t[..n].parse::<i64>().ok())
                .flatten()
        };
        if time.is_none() {
            let mut f = token.splitn(3, ':');
            if let (Some(h), Some(m), Some(sec)) = (f.next(), f.next(), f.next())
                && let (Some(h), Some(m), Some(sec)) =
                    (digits(h, 1, 2), digits(m, 1, 2), digits(sec, 1, 2))
            {
                time = Some((h, m, sec));
                continue;
            }
        }
        if day.is_none()
            && let Some(d) = digits(token, 1, 2)
        {
            day = Some(d);
            continue;
        }
        if month.is_none() && token.len() >= 3 {
            const MONTHS: [&str; 12] = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ];
            let m = token[..3].to_ascii_lowercase();
            if let Some(i) = MONTHS.iter().position(|x| *x == m) {
                month = Some(i as i64 + 1);
                continue;
            }
        }
        if year.is_none()
            && let Some(y) = digits(token, 2, 4)
        {
            year = Some(y);
        }
    }
    let (h, m, sec) = time?;
    let (day, month, mut year) = (day?, month?, year?);
    if (70..=99).contains(&year) {
        year += 1900;
    } else if (0..=69).contains(&year) {
        year += 2000;
    }
    if !(1..=31).contains(&day) || year < 1601 || h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + sec)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn dates_parse_in_the_usual_forms() {
        assert_eq!(
            parse_date("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(
            parse_date("Wednesday, 21-Oct-15 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(parse_date("Wed Oct 21 07:28:00 2015"), Some(1_445_412_480));
        assert_eq!(parse_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_date("not a date"), None);
    }

    #[test]
    fn cookies_go_where_they_belong() {
        let now = 1_000;
        let mut j = Jar::new(true);
        let u = uri("https://www.example.com/a/b");
        assert!(j.store(&u, "a=1", now));
        assert!(j.store(&u, "b=2; Domain=.example.com; Path=/", now));
        assert!(j.store(&u, "s=3; Secure; Path=/", now));
        assert!(!j.store(&u, "c=4; Domain=other.com", now));
        assert!(!j.store(&u, "c=4; Domain=com", now));
        assert_eq!(j.header(&u, now).as_deref(), Some("a=1; b=2; s=3"));
        // Host-only, its default path /a, and Secure only over HTTPS.
        assert_eq!(
            j.header(&uri("http://shop.example.com/"), now).as_deref(),
            Some("b=2")
        );
        assert_eq!(
            j.header(&uri("https://www.example.com/x"), now).as_deref(),
            Some("b=2; s=3")
        );
        // A later Set-Cookie replaces; Max-Age=0 removes.
        assert!(j.store(&u, "b=5; Domain=example.com; Path=/", now));
        assert!(j.store(&u, "a=1; Max-Age=0", now));
        assert_eq!(j.header(&u, now).as_deref(), Some("b=5; s=3"));
        // Expiry.
        assert!(j.store(&u, "e=6; Max-Age=10; Path=/", now));
        assert!(j.header(&u, now + 5).unwrap().contains("e=6"));
        assert!(!j.header(&u, now + 11).unwrap().contains("e=6"));
    }

    #[test]
    fn longer_paths_come_first() {
        let mut j = Jar::new(true);
        let u = uri("http://h.example/a/b/c");
        assert!(j.store(&u, "x=1; Path=/", 0));
        assert!(j.store(&u, "y=2; Path=/a/b", 0));
        assert_eq!(j.header(&u, 0).as_deref(), Some("y=2; x=1"));
        assert!(path_matches("/a/b", "/a/b"));
        assert!(path_matches("/a/b/c", "/a/b"));
        assert!(!path_matches("/a/bc", "/a/b"));
        assert_eq!(default_path("/a/b/c"), "/a/b");
        assert_eq!(default_path("/a"), "/");
    }
}
