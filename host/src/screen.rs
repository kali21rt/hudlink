//! Screen follows the laptop: backlight off while the session is locked,
//! dimmer at night.

use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::timeinfo::LocalTime;

fn loginctl(args: &[&str]) -> Option<String> {
    let out = Command::new("loginctl").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Watches logind's LockedHint for the user's graphical session.
/// GNOME, KDE and lockers that talk to logind set it; some Wayland lockers don't.
pub struct LockWatch {
    session: Option<String>,
    looked_up: Option<Instant>,
}

impl LockWatch {
    pub fn new() -> Self {
        LockWatch { session: None, looked_up: None }
    }

    fn session(&mut self) -> Option<String> {
        let stale = self.looked_up.map_or(true, |t| t.elapsed() > Duration::from_secs(30));
        if self.session.is_none() && stale {
            self.looked_up = Some(Instant::now());
            self.session = std::env::var("XDG_SESSION_ID").ok().filter(|s| !s.is_empty()).or_else(|| {
                let user = std::env::var("USER").ok()?;
                loginctl(&["show-user", &user, "-p", "Display", "--value"]).filter(|s| !s.is_empty())
            });
        }
        self.session.clone()
    }

    pub fn locked(&mut self) -> bool {
        let Some(sid) = self.session() else { return false };
        match loginctl(&["show-session", &sid, "-p", "LockedHint", "--value"]) {
            Some(v) => v == "yes",
            None => {
                self.session = None; // session ended, look it up again later
                false
            }
        }
    }
}

fn minutes(hm: &str) -> Option<u16> {
    let (h, m) = hm.split_once(':')?;
    Some(h.trim().parse::<u16>().ok()? * 60 + m.trim().parse::<u16>().ok()?)
}

pub fn is_night(cfg: &Config, t: &LocalTime) -> bool {
    let (Some(start), Some(end)) = (minutes(&cfg.night_start), minutes(&cfg.night_end)) else { return false };
    let now = t.h as u16 * 60 + t.m as u16;
    if start == end {
        false
    } else if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end // crosses midnight, e.g. 22:00-06:00
    }
}

/// Backlight level to send right now.
pub fn brightness(cfg: &Config, t: &LocalTime, locked: bool) -> u8 {
    if cfg.lock_off && locked {
        0
    } else if is_night(cfg, t) {
        cfg.night_brightness.min(cfg.brightness)
    } else {
        cfg.brightness
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeinfo::from_local_epoch;

    fn at(h: i64, m: i64) -> LocalTime {
        from_local_epoch(h * 3600 + m * 60)
    }

    #[test]
    fn night_window_over_midnight() {
        let c = Config::parse("night_start = 22:00\nnight_end = 06:00\nnight_brightness = 60\nbrightness = 200\n");
        assert!(is_night(&c, &at(23, 0)));
        assert!(is_night(&c, &at(2, 30)));
        assert!(!is_night(&c, &at(6, 0)));
        assert!(!is_night(&c, &at(12, 0)));
        assert_eq!(brightness(&c, &at(23, 0), false), 60);
        assert_eq!(brightness(&c, &at(12, 0), false), 200);
        assert_eq!(brightness(&c, &at(12, 0), true), 0);
    }

    #[test]
    fn night_off_when_empty() {
        let c = Config::parse("night_start =\n");
        assert!(!is_night(&c, &at(23, 0)));
    }
}
