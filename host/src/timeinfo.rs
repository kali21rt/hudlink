//! Local time + Javanese pasaran, computed without any crate.

use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::proto::Buf;

/// Days from 1970-01-01 to 1945-08-17, a well known "Jumat Legi".
const PASARAN_REF_DAYS: i64 = -8903;

/// Caches the UTC offset (asks `date +%z` every 10 minutes, so DST/timezone
/// changes are picked up without restarting).
pub struct Tz {
    offset_secs: i64,
    checked: Option<Instant>,
}

impl Tz {
    pub fn new() -> Self {
        Tz { offset_secs: 0, checked: None }
    }

    fn offset(&mut self) -> i64 {
        let stale = self.checked.map_or(true, |t| t.elapsed() > Duration::from_secs(600));
        if stale {
            if let Some(off) = Command::new("date")
                .arg("+%z")
                .output()
                .ok()
                .and_then(|o| parse_offset(String::from_utf8_lossy(&o.stdout).trim()))
            {
                self.offset_secs = off;
            }
            self.checked = Some(Instant::now());
        }
        self.offset_secs
    }
}

/// "+0700" -> 25200
fn parse_offset(s: &str) -> Option<i64> {
    if s.len() != 5 {
        return None;
    }
    let sign = match &s[..1] {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let h: i64 = s[1..3].parse().ok()?;
    let m: i64 = s[3..5].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

/// Howard Hinnant's civil_from_days: days since epoch -> (year, month, day)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

pub struct LocalTime {
    pub h: u8,
    pub m: u8,
    pub s: u8,
    pub wday: u8, // 0 = Minggu
    pub day: u8,
    pub month: u8,
    pub year: u16,
    pub pasaran: u8, // 0 Legi, 1 Pahing, 2 Pon, 3 Wage, 4 Kliwon
}

pub fn from_local_epoch(local: i64) -> LocalTime {
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    LocalTime {
        h: (secs / 3600) as u8,
        m: ((secs / 60) % 60) as u8,
        s: (secs % 60) as u8,
        wday: (days + 4).rem_euclid(7) as u8, // 1970-01-01 was a Thursday
        day: d as u8,
        month: mo as u8,
        year: y as u16,
        pasaran: (days - PASARAN_REF_DAYS).rem_euclid(5) as u8,
    }
}

pub fn now(tz: &mut Tz) -> LocalTime {
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    from_local_epoch(utc + tz.offset())
}

const DAYS_EN: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DAYS_ID: [&str; 7] = ["Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu"];
const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July",
                               "August", "September", "October", "November", "December"];
const MONTHS_ID: [&str; 12] = ["Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli",
                               "Agustus", "September", "Oktober", "November", "Desember"];
const PASARAN: [&str; 5] = ["Legi", "Pahing", "Pon", "Wage", "Kliwon"];

/// strftime-like: %A %a %d %e %B %b %m %Y %y %P(pasaran) %%
pub fn format_date(fmt: &str, t: &LocalTime, lang: &str) -> String {
    let (days, months) = if lang == "id" { (DAYS_ID, MONTHS_ID) } else { (DAYS_EN, MONTHS_EN) };
    let day = days[(t.wday % 7) as usize];
    let month = months[((t.month + 11) % 12) as usize];
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('A') => out.push_str(day),
            Some('a') => out.push_str(&day[..3]),
            Some('d') => out.push_str(&format!("{:02}", t.day)),
            Some('e') => out.push_str(&t.day.to_string()),
            Some('B') => out.push_str(month),
            Some('b') => out.push_str(&month[..3]),
            Some('m') => out.push_str(&format!("{:02}", t.month)),
            Some('Y') => out.push_str(&t.year.to_string()),
            Some('y') => out.push_str(&format!("{:02}", t.year % 100)),
            Some('P') => out.push_str(PASARAN[(t.pasaran % 5) as usize]),
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

/// TIME packet: hour (already 12h-converted if needed), min, sec, ampm (0 none, 1 AM, 2 PM),
/// then the two date lines as strings.
pub fn payload(t: &LocalTime, h24: bool, line1: &str, line2: &str, lang: &str) -> Vec<u8> {
    let (hour, ampm) = if h24 {
        (t.h, 0)
    } else {
        let h12 = if t.h % 12 == 0 { 12 } else { t.h % 12 };
        (h12, if t.h < 12 { 1 } else { 2 })
    };
    let mut b = Buf::default();
    b.u8(hour).u8(t.m).u8(t.s).u8(ampm);
    b.str(&format_date(line1, t, lang), 40).str(&format_date(line2, t, lang), 40);
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(y: i64, m: u32, d: u32) -> i64 {
        // brute force reverse for tests
        let mut days = -30_000;
        loop {
            if civil_from_days(days) == (y, m, d) {
                return days * 86_400;
            }
            days += 1;
        }
    }

    #[test]
    fn proklamasi_is_jumat_legi() {
        let t = from_local_epoch(epoch(1945, 8, 17));
        assert_eq!((t.wday, t.pasaran), (5, 0));
    }

    #[test]
    fn known_dates() {
        let t = from_local_epoch(epoch(2026, 11, 23) + 21 * 3600 + 5 * 60);
        assert_eq!((t.year, t.month, t.day, t.h, t.m), (2026, 11, 23, 21, 5));
        assert_eq!(t.wday, 1); // Senin
        let t = from_local_epoch(0);
        assert_eq!((t.year, t.month, t.day, t.wday), (1970, 1, 1, 4));
    }

    #[test]
    fn formats() {
        let t = from_local_epoch(epoch(2026, 11, 23) + 21 * 3600);
        assert_eq!(format_date("%A %P, %d %B %Y", &t, "id"), "Senin Wage, 23 November 2026");
        assert_eq!(format_date("%a %e %b", &t, "en"), "Mon 23 Nov");
        let p = payload(&t, false, "%A", "", "en");
        assert_eq!(&p[..4], &[9, 0, 0, 2]); // 9 PM
    }

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("+0700"), Some(25_200));
        assert_eq!(parse_offset("-0330"), Some(-12_600));
        assert_eq!(parse_offset("WIB"), None);
    }
}
