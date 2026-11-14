//! The parts of Python's `time` and `datetime` the recorders use.
//!
//! - [`monotonic`] is `time.monotonic()`: QueryPerformanceCounter, as CPython 3.13 uses
//!   on Windows, so it never reads near zero (the recorders treat `0.0` as "never").
//! - [`time_time`] is `time.time()`.
//! - [`DateTime`] is an aware or naive `datetime` to the microsecond, with the
//!   `isoformat()`, `strftime()` and `fromisoformat()` forms the recorders write and
//!   read back.
//! - [`DateTime::astimezone_local`] is `datetime.astimezone()` with no argument: the
//!   CRT's local time and its zone name (`"Eastern Standard Time"` on Windows, which
//!   is what Python's `tzname()` returns there), including the CRT's refusal of
//!   timestamps before 1970 or after the year 3000 (`OSError` in Python).

use crate::pyjson::PyErr;

/// `time.monotonic()`.
pub fn monotonic() -> f64 {
    imp::monotonic()
}

/// `time.time()`.
pub fn time_time() -> f64 {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    ns_to_secs(ns)
}

fn ns_to_secs(ns: i128) -> f64 {
    if ns % 1_000_000_000 == 0 {
        (ns / 1_000_000_000) as f64
    } else {
        ns as f64 / 1e9
    }
}

/// A `datetime` to the microsecond. `offset_s` is `None` for a naive datetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub micro: u32,
    pub offset_s: Option<i32>,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

impl DateTime {
    /// The UTC datetime `micros` microseconds after the Unix epoch, aware (`+00:00`).
    pub fn from_unix_micros_utc(micros: i64) -> DateTime {
        let mut dt = Self::naive_from_unix_micros(micros);
        dt.offset_s = Some(0);
        dt
    }

    fn naive_from_unix_micros(micros: i64) -> DateTime {
        let secs = micros.div_euclid(1_000_000);
        let micro = micros.rem_euclid(1_000_000) as u32;
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        DateTime {
            year: y as i32,
            month: m,
            day: d,
            hour: (sod / 3600) as u32,
            minute: (sod % 3600 / 60) as u32,
            second: (sod % 60) as u32,
            micro,
            offset_s: None,
        }
    }

    /// Microseconds since the Unix epoch of this wall time, with its offset removed
    /// (a naive datetime is read as UTC).
    pub fn to_unix_micros(&self) -> i64 {
        let days = days_from_civil(self.year as i64, self.month, self.day);
        let secs =
            days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
                - self.offset_s.unwrap_or(0) as i64;
        secs * 1_000_000 + self.micro as i64
    }

    /// `datetime.now(timezone.utc)`.
    pub fn utc_now() -> DateTime {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0);
        Self::from_unix_micros_utc(micros)
    }

    /// `datetime.now()`: naive local wall time.
    pub fn local_now() -> DateTime {
        let utc = Self::utc_now();
        match imp::local_offset(utc.to_unix_micros().div_euclid(1_000_000)) {
            Some((offset, _)) => {
                Self::naive_from_unix_micros(utc.to_unix_micros() + offset as i64 * 1_000_000)
            }
            None => {
                let mut n = utc;
                n.offset_s = None;
                n
            }
        }
    }

    /// `dt + timedelta(microseconds=n)`, keeping the offset.
    pub fn plus_micros(&self, micros: i64) -> DateTime {
        let off = self.offset_s.unwrap_or(0) as i64 * 1_000_000;
        let mut dt = Self::naive_from_unix_micros(self.to_unix_micros() + off + micros);
        dt.offset_s = self.offset_s;
        dt
    }

    /// `dt.isoformat()`.
    pub fn isoformat(&self) -> String {
        let mut s = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        if self.micro != 0 {
            s.push_str(&format!(".{:06}", self.micro));
        }
        if let Some(off) = self.offset_s {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, a % 3600 / 60));
            if a % 60 != 0 {
                s.push_str(&format!(":{:02}", a % 60));
            }
        }
        s
    }

    /// `dt.strftime(fmt)` for `%Y %m %d %H %M %S %f %I %p %B %%`, in the C locale.
    pub fn strftime(&self, fmt: &str) -> String {
        let mut out = String::new();
        let mut chars = fmt.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", self.year)),
                Some('m') => out.push_str(&format!("{:02}", self.month)),
                Some('d') => out.push_str(&format!("{:02}", self.day)),
                Some('H') => out.push_str(&format!("{:02}", self.hour)),
                Some('M') => out.push_str(&format!("{:02}", self.minute)),
                Some('S') => out.push_str(&format!("{:02}", self.second)),
                Some('f') => out.push_str(&format!("{:06}", self.micro)),
                Some('I') => {
                    let h = self.hour % 12;
                    out.push_str(&format!("{:02}", if h == 0 { 12 } else { h }));
                }
                Some('p') => out.push_str(if self.hour < 12 { "AM" } else { "PM" }),
                Some('B') => out.push_str(MONTHS[(self.month as usize).saturating_sub(1) % 12]),
                Some('%') => out.push('%'),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        out
    }

    /// `datetime.fromisoformat(text)`; `None` where Python raises `ValueError`.
    ///
    /// Accepts `YYYY-MM-DD` / `YYYYMMDD`, then optionally any one separator character
    /// and `HH[:MM[:SS[.f...]]]` (or the basic `HHMM[SS[.f...]]`), then optionally `Z`
    /// or `+HH[:MM[:SS[.ffffff]]]` / `+HHMM`. Not accepted, where Python is: ISO week
    /// dates (`2026-W37-4`), a comma as the decimal mark, and `24:00`.
    pub fn fromisoformat(text: &str) -> Option<DateTime> {
        let c: Vec<char> = text.chars().collect();
        let num = |s: &[char]| -> Option<u32> {
            if s.is_empty() || !s.iter().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            s.iter().collect::<String>().parse().ok()
        };
        let (year, month, day, date_len) = if c.len() >= 10 && c[4] == '-' {
            if c[7] != '-' {
                return None;
            }
            (num(&c[0..4])?, num(&c[5..7])?, num(&c[8..10])?, 10)
        } else if c.len() >= 8 {
            (num(&c[0..4])?, num(&c[4..6])?, num(&c[6..8])?, 8)
        } else {
            return None;
        };
        let year = year as i32;
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return None;
        }
        let mut dt = DateTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            micro: 0,
            offset_s: None,
        };
        if c.len() == date_len {
            return Some(dt);
        }
        let rest = &c[date_len + 1..];
        if rest.is_empty() {
            return None;
        }
        let tz_at = rest.iter().position(|ch| matches!(ch, '+' | '-' | 'Z'));
        let (time, tz) = match tz_at {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        let (h, m, s, us) = parse_time(time)?;
        if h > 23 || m > 59 || s > 59 {
            return None;
        }
        dt.hour = h;
        dt.minute = m;
        dt.second = s;
        dt.micro = us;
        if let Some(tz) = tz {
            if tz == ['Z'] {
                dt.offset_s = Some(0);
            } else {
                let sign = if tz[0] == '-' { -1 } else { 1 };
                let (oh, om, os, ous) = parse_time(&tz[1..])?;
                if oh > 23 || om > 59 || os > 59 || ous != 0 {
                    return None;
                }
                dt.offset_s = Some(sign * (oh * 3600 + om * 60 + os) as i32);
            }
        }
        Some(dt)
    }

    /// `self.astimezone()`: the local wall time (with its offset) and the zone name
    /// `tzname()` returns. A naive datetime must be given an offset first, as the
    /// Python does with `replace(tzinfo=timezone.utc)`.
    pub fn astimezone_local(&self) -> Result<(DateTime, String), PyErr> {
        let utc_micros = self.to_unix_micros();
        let ts = utc_micros.div_euclid(1_000_000);
        // The UCRT's localtime_s accepts -12 h ..= 3000-12-31 23:59:59 + 13 h
        // (_MIN_LOCAL_TIME, _MAX__TIME64_T + _MAX_LOCAL_TIME).
        if !(-43_200..=32_535_215_999 + 46_800).contains(&ts) {
            return Err(PyErr::OSError("[Errno 22] Invalid argument".to_string()));
        }
        let (offset, name) = imp::local_offset(ts).unwrap_or((0, "UTC".to_string()));
        let mut local = Self::naive_from_unix_micros(utc_micros + offset as i64 * 1_000_000);
        if !(1..=9999).contains(&local.year) {
            return Err(PyErr::OverflowError("date value out of range".to_string()));
        }
        local.offset_s = Some(offset);
        Ok((local, name))
    }
}

fn parse_time(t: &[char]) -> Option<(u32, u32, u32, u32)> {
    let two = |s: &[char]| -> Option<u32> {
        if s.len() == 2 && s.iter().all(|c| c.is_ascii_digit()) {
            s.iter().collect::<String>().parse().ok()
        } else {
            None
        }
    };
    let (main, frac) = match t.iter().position(|c| *c == '.') {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    };
    let parts: Vec<&[char]> = if main.contains(&':') {
        main.split(|c| *c == ':').collect()
    } else {
        main.chunks(2).collect()
    };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let h = two(parts[0])?;
    let m = if parts.len() > 1 { two(parts[1])? } else { 0 };
    let s = if parts.len() > 2 { two(parts[2])? } else { 0 };
    let mut us = 0;
    if let Some(f) = frac {
        if parts.len() != 3 || f.is_empty() || !f.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let digits: String = f.iter().take(6).collect();
        us = format!("{digits:0<6}").parse().ok()?;
    }
    Some((h, m, s, us))
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows::Win32::System::Time::{
        GetTimeZoneInformation, SystemTimeToTzSpecificLocalTime, TIME_ZONE_INFORMATION,
    };

    pub fn monotonic() -> f64 {
        static FREQ: OnceLock<i64> = OnceLock::new();
        let freq = *FREQ.get_or_init(|| {
            let mut f = 0i64;
            // SAFETY: QueryPerformanceFrequency writes one i64 to a valid pointer.
            let _ = unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(1)
        });
        let mut ticks = 0i64;
        // SAFETY: QueryPerformanceCounter writes one i64 to a valid pointer.
        let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
        let ns = ticks as i128 * 1_000_000_000 / freq as i128;
        super::ns_to_secs(ns)
    }

    fn wide(name: &[u16]) -> String {
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }

    /// Offset (seconds east of UTC) and zone name of local time at Unix second `ts`.
    pub fn local_offset(ts: i64) -> Option<(i32, String)> {
        let days = ts.div_euclid(86_400);
        let sod = ts.rem_euclid(86_400);
        let (y, m, d) = super::civil_from_days(days);
        let utc = SYSTEMTIME {
            wYear: y as u16,
            wMonth: m as u16,
            wDayOfWeek: (days + 4).rem_euclid(7) as u16,
            wDay: d as u16,
            wHour: (sod / 3600) as u16,
            wMinute: (sod % 3600 / 60) as u16,
            wSecond: (sod % 60) as u16,
            wMilliseconds: 0,
        };
        let mut local = SYSTEMTIME::default();
        // SAFETY: both pointers refer to live SYSTEMTIME values; None selects the
        // active time zone.
        unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
        let mut tzi = TIME_ZONE_INFORMATION::default();
        // SAFETY: GetTimeZoneInformation fills a valid TIME_ZONE_INFORMATION.
        let _ = unsafe { GetTimeZoneInformation(&mut tzi) };
        let local_secs =
            super::days_from_civil(local.wYear as i64, local.wMonth as u32, local.wDay as u32)
                * 86_400
                + local.wHour as i64 * 3600
                + local.wMinute as i64 * 60
                + local.wSecond as i64;
        let offset = (local_secs - ts) as i32;
        let dst_offset = -(tzi.Bias + tzi.DaylightBias) * 60;
        let is_dst = tzi.DaylightDate.wMonth != 0
            && tzi.DaylightBias != tzi.StandardBias
            && offset == dst_offset;
        let name = if is_dst {
            wide(&tzi.DaylightName)
        } else {
            wide(&tzi.StandardName)
        };
        Some((offset, name))
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;

    pub fn monotonic() -> f64 {
        static START: OnceLock<Instant> = OnceLock::new();
        1000.0 + START.get_or_init(Instant::now).elapsed().as_secs_f64()
    }

    pub fn local_offset(_ts: i64) -> Option<(i32, String)> {
        Some((0, "UTC".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        let dt = DateTime::from_unix_micros_utc(1_788_480_000_123_456);
        assert_eq!(dt.isoformat(), "2026-09-04T00:00:00.123456+00:00");
        assert_eq!(DateTime::fromisoformat(&dt.isoformat()), Some(dt));
        assert_eq!(dt.strftime("%Y%m%dT%H%M%S.%fZ"), "20260904T000000.123456Z");
        assert!(DateTime::fromisoformat("2026-02-30").is_none());
        assert!(monotonic() > 0.0);
    }
}
