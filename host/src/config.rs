//! Config file: ~/.config/aksara-hud/config  (simple `key = value` lines)

use std::fs;
use std::path::PathBuf;

const DEFAULT_CONFIG: &str = "\
# Aksara HUD config
# Baris yang diawali # adalah komentar.

# Port serial (symlink dari aturan udev)
port = /dev/aksarahud
baud = 921600

# Cuaca: ketik nama kotamu. Kosongkan untuk mematikan cuaca.
city = Yogyakarta
# Atau isi koordinat langsung (lebih akurat, melewati pencarian kota):
# latitude = -7.7956
# longitude = 110.3695
weather_minutes = 15

# Layar
brightness = 200
# Halaman awal: 0 = beranda, 1 = sistem, 2 = cava
start_page = 0
# Putar layar: 1 atau 3 (kebalik 180 derajat). Bisa juga: aksara-hud rotate
rotation = 1
# Ganti halaman otomatis tiap N detik (0 = mati)
auto_page = 0

# Cava
cava = true
cava_bars = 32
cava_fps = 30
# pipewire atau pulse
cava_input = pipewire

# Sensor suhu (kosong = deteksi otomatis)
# temp_sensor = /sys/class/hwmon/hwmon5/temp1_input
";

#[derive(Debug, Clone)]
pub struct Config {
    pub port: String,
    pub baud: u32,
    pub city: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub weather_minutes: u64,
    pub brightness: u8,
    pub start_page: u8,
    pub rotation: u8,
    pub auto_page: u64,
    pub cava: bool,
    pub cava_bars: usize,
    pub cava_fps: u32,
    pub cava_input: String,
    pub temp_sensor: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: "/dev/aksarahud".into(),
            baud: 921_600,
            city: String::new(),
            latitude: None,
            longitude: None,
            weather_minutes: 15,
            brightness: 200,
            start_page: 0,
            rotation: 1,
            auto_page: 0,
            cava: true,
            cava_bars: 32,
            cava_fps: 30,
            cava_input: "pipewire".into(),
            temp_sensor: None,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"));
    base.join("aksara-hud")
}

impl Config {
    /// Load the config, creating a commented default file on first run.
    pub fn load() -> Config {
        let dir = config_dir();
        let path = dir.join("config");
        if !path.exists() {
            let _ = fs::create_dir_all(&dir);
            if fs::write(&path, DEFAULT_CONFIG).is_ok() {
                eprintln!("[aksara-hud] config baru dibuat: {}", path.display());
            }
        }
        let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim().trim_matches('"'));
            match key {
                "port" => c.port = value.into(),
                "baud" => c.baud = value.parse().unwrap_or(c.baud),
                "city" => c.city = value.into(),
                "latitude" => c.latitude = value.parse().ok(),
                "longitude" => c.longitude = value.parse().ok(),
                "weather_minutes" => c.weather_minutes = value.parse().unwrap_or(15).max(5),
                "brightness" => c.brightness = value.parse().unwrap_or(200),
                "start_page" => c.start_page = value.parse().unwrap_or(0),
                "rotation" => c.rotation = if value == "3" { 3 } else { 1 },
                "auto_page" => c.auto_page = value.parse().unwrap_or(0),
                "cava" => c.cava = matches!(value, "true" | "yes" | "1" | "on"),
                "cava_bars" => c.cava_bars = value.parse().unwrap_or(32).clamp(8, 64),
                "cava_fps" => c.cava_fps = value.parse().unwrap_or(30).clamp(10, 60),
                "cava_input" => c.cava_input = value.into(),
                "temp_sensor" if !value.is_empty() => c.temp_sensor = Some(value.into()),
                _ => eprintln!("[aksara-hud] kunci config tidak dikenal: {key}"),
            }
        }
        c
    }
}

/// Change (or add) one `key = value` line in the config file, keeping comments.
pub fn set_key(key: &str, value: &str) {
    let path = config_dir().join("config");
    let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let is_key = l.split_once('=').map_or(false, |(k, _)| k.trim() == key) && !l.trim_start().starts_with('#');
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
        assert_eq!(c.port, "/dev/aksarahud");
        assert_eq!(c.city, "Yogyakarta");
        assert_eq!(c.cava_bars, 32);
        assert!(c.latitude.is_none());
    }

    #[test]
    fn overrides_and_clamps() {
        let c = Config::parse("cava_bars = 200\nlatitude = -6.2\ncava = off\n");
        assert_eq!(c.cava_bars, 64);
        assert_eq!(c.latitude, Some(-6.2));
        assert!(!c.cava);
    }
}
