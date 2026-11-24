//! Config file: ~/.config/hudlink/config  (simple `key = value` lines)
//!
//! Every key can also be changed live: hudlink set <key> <value>

use std::fs;
use std::path::PathBuf;

pub const DEFAULT_CONFIG: &str = "\
# hudlink config. Edit, then run `hudlink reload`, or change one key live: hudlink set <key> <value>

# --- connection ---
port = /dev/hudlink
baud = 921600

# --- identity ---
# title: auto = distro name from /etc/os-release, or any text (e.g. title = THINKPAD)
title = auto
# logo: auto = distro logo installed on this system, none, or a path to a png/svg
logo = auto

# --- theme ---
# theme: auto | matugen | distro | ocean | forest | rose | slate | mono | amber | kayon
#   auto    = matugen if its color file exists, otherwise the distro color
#   matugen = read matugen_file (needs the matugen template, see the guide)
#   distro  = accent from ANSI_COLOR in /etc/os-release
theme = auto
matugen_file = ~/.config/hudlink/colors
# accent = #7aa2f7          (optional, overrides the accent color)
# cava bar colors: theme = follow the theme, cava = gradient from ~/.config/cava/config
cava_colors = theme

# --- clock & date ---
# clock_style: segment | digits | text
clock_style = segment
clock_24h = true
clock_seconds = true
# day/month names: en | id
language = en
# %A weekday, %a short weekday, %d day, %B month, %b short month, %Y year, %P Javanese pasaran
date_line1 = %A
date_line2 = %d %B %Y

# --- layout ---
# pages shown, in order: home, system, process, cava, lyrics
pages = home, system, process, cava, lyrics
# home_layout: dashboard | clock | music
home_layout = dashboard
start_page = 0
# switch page automatically every N seconds (0 = off)
auto_page = 0
rotation = 1
brightness = 200

# --- cava ---
cava = true
cava_bars = 32
cava_fps = 30
# cava_style: bars | blocks | mirror
cava_style = blocks
# pulse | pipewire (Fedora's cava is built without pipewire)
cava_input = pulse

# --- music & lyrics ---
media = true
lyrics = true

# --- weather ---
city = Jakarta
# latitude = -6.2
# longitude = 106.8
weather_minutes = 15

# --- sensors (empty = auto-detect) ---
# temp_sensor = /sys/class/hwmon/hwmon7/temp1_input
";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub port: String,
    pub baud: u32,
    pub title: String,
    pub logo: String,
    pub theme: String,
    pub matugen_file: String,
    pub accent: Option<String>,
    pub cava_colors: String,
    pub clock_style: String,
    pub clock_24h: bool,
    pub clock_seconds: bool,
    pub language: String,
    pub date_line1: String,
    pub date_line2: String,
    pub pages: Vec<String>,
    pub home_layout: String,
    pub start_page: u8,
    pub auto_page: u64,
    pub rotation: u8,
    pub brightness: u8,
    pub cava: bool,
    pub cava_bars: usize,
    pub cava_fps: u32,
    pub cava_style: String,
    pub cava_input: String,
    pub media: bool,
    pub lyrics: bool,
    pub city: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub weather_minutes: u64,
    pub temp_sensor: Option<String>,
}

pub const PAGE_NAMES: [&str; 5] = ["home", "system", "process", "cava", "lyrics"];

impl Default for Config {
    fn default() -> Self {
        Config::parse(DEFAULT_CONFIG)
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("hudlink")
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// "~/x" -> "/home/user/x"
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

fn boolean(v: &str) -> bool {
    matches!(v, "true" | "yes" | "1" | "on")
}

fn one_of(v: &str, allowed: &[&str], fallback: &str) -> String {
    if allowed.contains(&v) { v.to_string() } else { fallback.to_string() }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config")
    }

    /// Load the config, creating a commented default file on first run.
    pub fn load() -> Config {
        let path = Config::path();
        if !path.exists() {
            let _ = fs::create_dir_all(config_dir());
            if fs::write(&path, DEFAULT_CONFIG).is_ok() {
                eprintln!("[hudlink] created default config: {}", path.display());
            }
        }
        let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Config {
        // start from hard defaults so a missing key never breaks anything
        let mut c = Config {
            port: "/dev/hudlink".into(),
            baud: 921_600,
            title: "auto".into(),
            logo: "auto".into(),
            theme: "auto".into(),
            matugen_file: "~/.config/hudlink/colors".into(),
            accent: None,
            cava_colors: "theme".into(),
            clock_style: "segment".into(),
            clock_24h: true,
            clock_seconds: true,
            language: "en".into(),
            date_line1: "%A".into(),
            date_line2: "%d %B %Y".into(),
            pages: PAGE_NAMES.iter().map(|s| s.to_string()).collect(),
            home_layout: "dashboard".into(),
            start_page: 0,
            auto_page: 0,
            rotation: 1,
            brightness: 200,
            cava: true,
            cava_bars: 32,
            cava_fps: 30,
            cava_style: "blocks".into(),
            cava_input: "pulse".into(),
            media: true,
            lyrics: true,
            city: String::new(),
            latitude: None,
            longitude: None,
            weather_minutes: 15,
            temp_sensor: None,
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            c.apply(key.trim(), value.trim().trim_matches('"'));
        }
        c
    }

    /// Apply one key. Returns false for an unknown key.
    pub fn apply(&mut self, key: &str, v: &str) -> bool {
        match key {
            "port" => self.port = v.into(),
            "baud" => self.baud = v.parse().unwrap_or(self.baud),
            "title" => self.title = if v.is_empty() { "auto".into() } else { v.into() },
            "logo" => self.logo = if v.is_empty() { "auto".into() } else { v.into() },
            "theme" => self.theme = v.to_lowercase(),
            "matugen_file" => self.matugen_file = v.into(),
            "accent" => self.accent = (!v.is_empty() && v != "none").then(|| v.to_string()),
            "cava_colors" => self.cava_colors = one_of(v, &["theme", "cava"], "theme"),
            "clock_style" => self.clock_style = one_of(v, &["segment", "digits", "text"], "segment"),
            "clock_24h" => self.clock_24h = boolean(v),
            "clock_seconds" => self.clock_seconds = boolean(v),
            "language" => self.language = one_of(v, &["en", "id"], "en"),
            "date_line1" => self.date_line1 = v.into(),
            "date_line2" => self.date_line2 = v.into(),
            "pages" => {
                let list: Vec<String> = v
                    .split(|c| c == ',' || c == ' ')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| PAGE_NAMES.contains(&s.as_str()))
                    .collect();
                if !list.is_empty() {
                    self.pages = list;
                }
            }
            "home_layout" => self.home_layout = one_of(v, &["dashboard", "clock", "music"], "dashboard"),
            "start_page" => self.start_page = v.parse().unwrap_or(0),
            "auto_page" => self.auto_page = v.parse().unwrap_or(0),
            "rotation" => self.rotation = if v == "3" { 3 } else { 1 },
            "brightness" => self.brightness = v.parse::<u8>().unwrap_or(200).max(10),
            "cava" => self.cava = boolean(v),
            "cava_bars" => self.cava_bars = v.parse().unwrap_or(32).clamp(8, 64),
            "cava_fps" => self.cava_fps = v.parse().unwrap_or(30).clamp(10, 60),
            "cava_style" => self.cava_style = one_of(v, &["bars", "blocks", "mirror"], "blocks"),
            "cava_input" => self.cava_input = v.into(),
            "media" => self.media = boolean(v),
            "lyrics" => self.lyrics = boolean(v),
            "city" => self.city = v.into(),
            "latitude" => self.latitude = v.parse().ok(),
            "longitude" => self.longitude = v.parse().ok(),
            "weather_minutes" => self.weather_minutes = v.parse().unwrap_or(15).max(5),
            "temp_sensor" => self.temp_sensor = (!v.is_empty()).then(|| v.to_string()),
            _ => return false,
        }
        true
    }

    /// Does a change between `self` and `new` require restarting cava?
    pub fn cava_changed(&self, new: &Config) -> bool {
        self.cava != new.cava
            || self.cava_bars != new.cava_bars
            || self.cava_fps != new.cava_fps
            || self.cava_input != new.cava_input
    }
}

/// Change (or add) one `key = value` line in the config file, keeping comments.
pub fn set_key(key: &str, value: &str) {
    let path = Config::path();
    let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let is_key = !l.trim_start().starts_with('#')
                && l.split_once('=').map_or(false, |(k, _)| k.trim() == key);
            if is_key && !found {
                found = true;
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(format!("{key} = {value}"));
    }
    let _ = fs::create_dir_all(config_dir());
    let _ = fs::write(&path, out.join("\n") + "\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses() {
        let c = Config::parse(DEFAULT_CONFIG);
        assert_eq!(c.port, "/dev/hudlink");
        assert_eq!(c.city, "Jakarta");
        assert_eq!(c.pages.len(), 5);
        assert_eq!(c.cava_style, "blocks");
        assert_eq!(c.date_line2, "%d %B %Y");
    }

    #[test]
    fn validation() {
        let c = Config::parse("pages = cava, lyrics, nope\ncava_bars = 200\nclock_style = fancy\n");
        assert_eq!(c.pages, vec!["cava", "lyrics"]);
        assert_eq!(c.cava_bars, 64);
        assert_eq!(c.clock_style, "segment");
        let mut d = c.clone();
        assert!(d.apply("cava_bars", "16"));
        assert!(c.cava_changed(&d));
        assert!(!d.apply("terbang", "1"));
    }
}
