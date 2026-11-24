//! Lyrics: a local .lrc file next to the song (or in ~/Music/lyrics), otherwise LRCLIB
//! (https://lrclib.net, free, no API key). Synced LRC lyrics follow the playback position.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::config::home;

#[derive(Debug, Default, Clone)]
pub struct Lyrics {
    pub synced: Vec<(f64, String)>, // (seconds, line)
    pub plain: Vec<String>,
    pub instrumental: bool,
}

fn curl(url: &str) -> Option<String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "15", "-A", "hudlink (https://github.com/)", url])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a JSON string value for `key` (first occurrence). Handles escapes incl. \uXXXX.
/// Returns None for a missing key or `null`.
pub fn json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let rest = json[json.find(&needle)? + needle.len()..].trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'u' => {
                    let code: String = chars.by_ref().take(4).collect();
                    let mut cp = u32::from_str_radix(&code, 16).ok()?;
                    // surrogate pair
                    if (0xD800..0xDC00).contains(&cp) {
                        let rest: String = chars.by_ref().take(6).collect();
                        if let Some(lo) = rest.strip_prefix("\\u").and_then(|h| u32::from_str_radix(h, 16).ok()) {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        }
                    }
                    out.push(char::from_u32(cp).unwrap_or('?'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn json_bool(json: &str, key: &str) -> bool {
    let needle = format!("\"{key}\":");
    json.find(&needle).map_or(false, |i| json[i + needle.len()..].trim_start().starts_with("true"))
}

/// Parse LRC text. Lines may carry several timestamps: "[00:12.00][01:30.50]text".
pub fn parse_lrc(text: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut line = raw.trim();
        let mut stamps = Vec::new();
        while let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[..end];
            let mut parts = tag.split(':');
            let (Some(m), Some(s)) = (parts.next(), parts.next()) else { break };
            let (Ok(m), Ok(s)) = (m.trim().parse::<f64>(), s.trim().parse::<f64>()) else { break };
            stamps.push(m * 60.0 + s);
            line = &rest[end + 1..];
        }
        let text = line.trim().to_string();
        for t in stamps {
            out.push((t, text.clone()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn from_lrc_text(text: &str) -> Option<Lyrics> {
    let synced = parse_lrc(text);
    if !synced.is_empty() {
        return Some(Lyrics { synced, ..Default::default() });
    }
    let plain: Vec<String> = text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    (!plain.is_empty()).then(|| Lyrics { plain, ..Default::default() })
}

fn local_lrc(title: &str, artist: &str, url: &str) -> Option<Lyrics> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = url.strip_prefix("file://") {
        let p = PathBuf::from(url_decode(path));
        candidates.push(p.with_extension("lrc"));
    }
    let dir = home().join("Music/lyrics");
    candidates.push(dir.join(format!("{artist} - {title}.lrc")));
    candidates.push(dir.join(format!("{title}.lrc")));
    candidates.iter().find_map(|p| fs::read_to_string(p).ok()).and_then(|t| from_lrc_text(&t))
}

fn from_lrclib_json(obj: &str) -> Option<Lyrics> {
    if json_bool(obj, "instrumental") {
        return Some(Lyrics { instrumental: true, ..Default::default() });
    }
    if let Some(s) = json_str(obj, "syncedLyrics") {
        let synced = parse_lrc(&s);
        if !synced.is_empty() {
            return Some(Lyrics { synced, ..Default::default() });
        }
    }
    json_str(obj, "plainLyrics").and_then(|p| from_lrc_text(&p))
}

/// Strip "(Official Video)", "- Remastered 2011", "[Lyrics]" etc. that video sites add.
pub fn clean_title(t: &str) -> String {
    let mut s = t.to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (s.find(open), s.find(close)) {
            if b <= a {
                break;
            }
            let inner = s[a + 1..b].to_lowercase();
            if ["official", "video", "lyric", "audio", "remaster", "mv", "hd", "4k", "visualizer"]
                .iter()
                .any(|w| inner.contains(w))
            {
                s.replace_range(a..=b, "");
            } else {
                break;
            }
        }
    }
    if let Some(i) = s.to_lowercase().find(" - remaster") {
        s.truncate(i);
    }
    s.trim().to_string()
}

fn lrclib(title: &str, artist: &str, album: &str, len: f64) -> Option<Lyrics> {
    let t = clean_title(title);
    // YouTube in a browser often gives "Artist - Title" as the title and the channel as artist
    let (t, a) = match (artist.is_empty() || artist.ends_with(" - Topic"), t.split_once(" - ")) {
        (_, Some((a, tt))) if artist.is_empty() || artist.ends_with(" - Topic") => (tt.to_string(), a.to_string()),
        _ => (t.clone(), artist.trim_end_matches(" - Topic").to_string()),
    };
    if len > 0.0 {
        let url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}&album_name={}&duration={}",
            url_encode(&t), url_encode(&a), url_encode(album), len.round() as u64
        );
        if let Some(l) = curl(&url).as_deref().and_then(from_lrclib_json) {
            return Some(l);
        }
    }
    let url = format!("https://lrclib.net/api/search?track_name={}&artist_name={}", url_encode(&t), url_encode(&a));
    let json = curl(&url)?;
    // results are an array of objects; prefer the first one with synced lyrics
    let objects: Vec<&str> = json.split("},{").collect();
    objects
        .iter()
        .find(|o| json_str(o, "syncedLyrics").is_some())
        .or(objects.first())
        .and_then(|o| from_lrclib_json(o))
}

pub fn fetch(title: &str, artist: &str, album: &str, len: f64, url: &str) -> Option<Lyrics> {
    if let Some(l) = local_lrc(title, artist, url) {
        eprintln!("[hudlink] lyrics: local file for \"{title}\"");
        return Some(l);
    }
    let found = lrclib(title, artist, album, len);
    eprintln!("[hudlink] lyrics for \"{title}\" - {artist}: {}", if found.is_some() { "found (lrclib)" } else { "not found" });
    found
}

/// Current line plus `before` lines above and `after` below.
/// Returns (index of current line in the returned list, lines).
pub fn window(l: &Lyrics, pos: f64, before: usize, after: usize) -> (usize, Vec<String>) {
    let idx = l.synced.iter().rposition(|(t, _)| *t <= pos + 0.15); // tiny lead looks more natural
    let Some(idx) = idx else {
        // before the first line: show the upcoming lines, nothing highlighted yet
        let lines = l.synced.iter().take(after + 1).map(|(_, s)| s.clone());
        let mut v = vec![String::new()];
        v.extend(lines);
        return (0, v);
    };
    let start = idx.saturating_sub(before);
    let end = (idx + after + 1).min(l.synced.len());
    let lines = l.synced[start..end].iter().map(|(_, s)| if s.is_empty() { "...".into() } else { s.clone() }).collect();
    (idx - start, lines)
}

/// Unsynced lyrics: estimate the line from position / length.
pub fn window_plain(l: &Lyrics, pos: f64, len: f64, before: usize, after: usize) -> (usize, Vec<String>) {
    if l.plain.is_empty() {
        return (0, vec![]);
    }
    let frac = if len > 0.0 { (pos / len).clamp(0.0, 0.999) } else { 0.0 };
    let idx = (frac * l.plain.len() as f64) as usize;
    let start = idx.saturating_sub(before);
    let end = (idx + after + 1).min(l.plain.len());
    (idx - start, l.plain[start..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_parsing_and_window() {
        let l = Lyrics {
            synced: parse_lrc("[ar:Someone]\n[00:01.00]one\n[00:05.50]two\n[00:09.00][00:20.00]three\n[00:12.00]four\n"),
            ..Default::default()
        };
        assert_eq!(l.synced.len(), 5);
        assert_eq!(l.synced[4].1, "three");
        let (cur, lines) = window(&l, 6.0, 2, 3);
        assert_eq!(lines[cur], "two");
        assert_eq!(lines, vec!["one", "two", "three", "four", "three"]);
        let (cur, lines) = window(&l, 0.2, 2, 3);
        assert_eq!((cur, lines[0].as_str(), lines[1].as_str()), (0, "", "one"));
    }

    #[test]
    fn lrclib_json() {
        let j = r#"{"id":1,"trackName":"X","instrumental":false,"plainLyrics":"a\nb","syncedLyrics":"[00:01.00] Café \"hi\"\n[00:02.00] b"}"#;
        let l = from_lrclib_json(j).unwrap();
        assert_eq!(l.synced[0].1, "Café \"hi\"");
        assert_eq!(json_str(r#"{"syncedLyrics":null}"#, "syncedLyrics"), None);
        assert!(from_lrclib_json(r#"{"instrumental":true,"plainLyrics":null}"#).unwrap().instrumental);
    }

    #[test]
    fn title_cleanup() {
        assert_eq!(clean_title("Yellow (Official Video)"), "Yellow");
        assert_eq!(clean_title("Let It Be - Remastered 2009"), "Let It Be");
        assert_eq!(clean_title("Song (feat. X)"), "Song (feat. X)");
        assert_eq!(url_decode("a%20b%C3%A9"), "a bé");
    }
}
