//! Weather from Open-Meteo (free, no API key). HTTP via the system `curl`.

use std::process::Command;

use crate::proto::Buf;

#[derive(Clone, Debug)]
pub struct Weather {
    pub temp: f64,
    pub feels: f64,
    pub tmax: f64,
    pub tmin: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: u8,
    pub is_day: u8,
    pub city: String,
}

fn curl(url: &str) -> Option<String> {
    let out = Command::new("curl").args(["-fsSL", "--max-time", "15", url]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Find the first number after `"key":` inside the block that starts at `section`.
/// Handles both `"key":12.3` and `"key":[12.3, ...]`. Tiny, but enough for Open-Meteo.
fn num_in(json: &str, section: &str, key: &str) -> Option<f64> {
    let block = &json[json.find(section)?..];
    let needle = format!("\"{key}\":");
    let rest = block[block.find(&needle)? + needle.len()..].trim_start().trim_start_matches('[').trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn str_in(json: &str, section: &str, key: &str) -> Option<String> {
    let block = &json[json.find(section)?..];
    let needle = format!("\"{key}\":\"");
    let rest = &block[block.find(&needle)? + needle.len()..];
    Some(rest[..rest.find('"')?].to_string())
}

/// City name -> (lat, lon, display name)
pub fn geocode(city: &str) -> Option<(f64, f64, String)> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=id&format=json",
        url_encode(city)
    );
    let json = curl(&url)?;
    let lat = num_in(&json, "\"results\":", "latitude")?;
    let lon = num_in(&json, "\"results\":", "longitude")?;
    let name = str_in(&json, "\"results\":", "name").unwrap_or_else(|| city.to_string());
    Some((lat, lon, name))
}

pub fn fetch(lat: f64, lon: f64, city: &str) -> Option<Weather> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &daily=temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=1"
    );
    parse_forecast(&curl(&url)?, city)
}

pub fn parse_forecast(json: &str, city: &str) -> Option<Weather> {
    let cur = "\"current\":{";
    let day = "\"daily\":{";
    Some(Weather {
        temp: num_in(json, cur, "temperature_2m")?,
        feels: num_in(json, cur, "apparent_temperature").unwrap_or(f64::NAN),
        humidity: num_in(json, cur, "relative_humidity_2m").unwrap_or(0.0),
        code: num_in(json, cur, "weather_code").unwrap_or(0.0) as u8,
        wind: num_in(json, cur, "wind_speed_10m").unwrap_or(0.0),
        is_day: num_in(json, cur, "is_day").unwrap_or(1.0) as u8,
        tmax: num_in(json, day, "temperature_2m_max").unwrap_or(f64::NAN),
        tmin: num_in(json, day, "temperature_2m_min").unwrap_or(f64::NAN),
        city: city.to_string(),
    })
}

pub fn payload(w: &Weather) -> Vec<u8> {
    let x10 = |v: f64| if v.is_finite() { (v * 10.0).round() as i16 } else { 0 };
    let mut b = Buf::default();
    b.i16(x10(w.temp)).i16(x10(w.feels)).i16(x10(w.tmax)).i16(x10(w.tmin));
    b.u8(w.humidity.round().clamp(0.0, 100.0) as u8);
    b.u16((w.wind * 10.0).round().clamp(0.0, 65535.0) as u16);
    b.u8(w.code).u8(w.is_day).str(&w.city, 30);
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"latitude":-7.8,"longitude":110.375,"current_units":{"time":"iso8601","temperature_2m":"°C"},
    "current":{"time":"2026-11-23T21:00","interval":900,"temperature_2m":26.4,"apparent_temperature":29.9,
    "relative_humidity_2m":88,"weather_code":61,"wind_speed_10m":5.2,"is_day":0},
    "daily_units":{"temperature_2m_max":"°C"},
    "daily":{"time":["2026-11-23"],"temperature_2m_max":[31.6],"temperature_2m_min":[23.1]}}"#;

    #[test]
    fn parses_open_meteo() {
        let w = parse_forecast(SAMPLE, "Yogyakarta").unwrap();
        assert_eq!(w.temp, 26.4);
        assert_eq!(w.feels, 29.9);
        assert_eq!(w.code, 61);
        assert_eq!(w.is_day, 0);
        assert_eq!(w.tmax, 31.6);
        assert_eq!(w.tmin, 23.1);
        assert_eq!(w.humidity, 88.0);
    }

    #[test]
    fn parses_geocoding_block() {
        let j = r#"{"results":[{"id":1,"name":"Yogyakarta","latitude":-7.80139,"longitude":110.36472,"elevation":120.0}]}"#;
        assert_eq!(num_in(j, "\"results\":", "latitude"), Some(-7.80139));
        assert_eq!(str_in(j, "\"results\":", "name").as_deref(), Some("Yogyakarta"));
    }

    #[test]
    fn encodes_spaces() {
        assert_eq!(url_encode("Kota Malang"), "Kota%20Malang");
    }
}
