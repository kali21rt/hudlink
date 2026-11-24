//! "Now playing" from any MPRIS player (Spotify, Firefox, mpv, ...) via `playerctl`,
//! plus lyrics fetched in the background when the track changes.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::lyrics::{self, Lyrics};
use crate::proto::Buf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub status: u8, // 0 none, 1 playing, 2 paused
    pub title: String,
    pub artist: String,
    pub album: String,
    pub pos: f64, // seconds, at the time of polling
    pub len: f64, // seconds (0 = unknown)
    pub url: String,
}

impl Track {
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.artist, self.title)
    }
}

#[derive(Clone)]
pub enum LyricsState {
    Idle,
    Searching,
    Found(Arc<Lyrics>),
    NotFound,
}

pub struct Hub {
    pub track: Option<Track>,
    pub polled_at: Instant,
    pub lyrics_key: String,
    pub lyrics: LyricsState,
}

pub type SharedHub = Arc<Mutex<Hub>>;

impl Hub {
    /// Playback position now, interpolated between playerctl polls.
    pub fn position(&self) -> f64 {
        match &self.track {
            Some(t) if t.status == 1 => {
                let p = t.pos + self.polled_at.elapsed().as_secs_f64();
                if t.len > 0.0 { p.min(t.len) } else { p }
            }
            Some(t) => t.pos,
            None => 0.0,
        }
    }
}

const SEP: char = '\u{1f}';

fn poll() -> Option<Track> {
    let fmt = ["{{status}}", "{{title}}", "{{artist}}", "{{album}}", "{{position}}", "{{mpris:length}}", "{{xesam:url}}"]
        .join(&SEP.to_string());
    let out = Command::new("playerctl").args(["metadata", "--format", &fmt]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'))
}

pub fn parse(line: &str) -> Option<Track> {
    let f: Vec<&str> = line.split(SEP).collect();
    if f.len() < 7 {
        return None;
    }
    let status = match f[0] {
        "Playing" => 1,
        "Paused" => 2,
        _ => return None, // Stopped
    };
    let micros = |s: &str| s.trim().parse::<f64>().map(|v| v / 1_000_000.0).unwrap_or(0.0);
    Some(Track {
        status,
        title: f[1].to_string(),
        artist: f[2].to_string(),
        album: f[3].to_string(),
        pos: micros(f[4]),
        len: micros(f[5]),
        url: f[6].to_string(),
    })
}

/// Start the polling thread (2x per second). Lyrics are fetched on track change.
pub fn spawn(want_lyrics: bool) -> SharedHub {
    let hub = Arc::new(Mutex::new(Hub {
        track: None,
        polled_at: Instant::now(),
        lyrics_key: String::new(),
        lyrics: LyricsState::Idle,
    }));
    let h = hub.clone();
    thread::spawn(move || {
        let mut cache: HashMap<String, Option<Arc<Lyrics>>> = HashMap::new();
        let results: Arc<Mutex<Vec<(String, Option<Lyrics>)>>> = Arc::new(Mutex::new(Vec::new()));
        loop {
            let track = poll();
            // collect finished lyric searches
            for (key, found) in results.lock().unwrap().drain(..) {
                cache.insert(key, found.map(Arc::new));
            }
            let mut hub = h.lock().unwrap();
            if let Some(t) = &track {
                let key = t.key();
                if want_lyrics && !t.title.is_empty() {
                    match cache.get(&key) {
                        Some(Some(l)) => hub.lyrics = LyricsState::Found(l.clone()),
                        Some(None) => hub.lyrics = LyricsState::NotFound,
                        None if hub.lyrics_key != key => {
                            hub.lyrics = LyricsState::Searching;
                            let (t2, res, k2) = (t.clone(), results.clone(), key.clone());
                            thread::spawn(move || {
                                let found = lyrics::fetch(&t2.title, &t2.artist, &t2.album, t2.len, &t2.url);
                                res.lock().unwrap().push((k2, found));
                            });
                        }
                        None => {}
                    }
                    hub.lyrics_key = key;
                }
            } else {
                hub.lyrics = LyricsState::Idle;
                hub.lyrics_key.clear();
            }
            hub.track = track;
            hub.polled_at = Instant::now();
            drop(hub);
            thread::sleep(Duration::from_millis(500));
        }
    });
    hub
}

/// MEDIA packet: status, position s, length s, title, artist.
pub fn payload(hub: &Hub) -> Vec<u8> {
    let mut b = Buf::default();
    match &hub.track {
        Some(t) => {
            b.u8(t.status).u16(hub.position() as u16).u16(t.len as u16).str(&t.title, 60).str(&t.artist, 40);
        }
        None => {
            b.u8(0).u16(0).u16(0).str("", 0).str("", 0);
        }
    }
    b.0
}

/// LYRICS packet: state, index of the current line within the list, lines.
/// state: 0 no music, 1 searching, 2 synced, 3 plain (estimated), 4 not found, 5 instrumental
pub fn lyrics_payload(hub: &Hub) -> (Vec<u8>, String) {
    let mut b = Buf::default();
    let (state, cur, lines): (u8, usize, Vec<String>) = match (&hub.track, &hub.lyrics) {
        (None, _) => (0, 0, vec![]),
        (_, LyricsState::Idle) | (_, LyricsState::Searching) => (1, 0, vec![]),
        (_, LyricsState::NotFound) => (4, 0, vec![]),
        (Some(t), LyricsState::Found(l)) => {
            if l.instrumental {
                (5, 0, vec![])
            } else if !l.synced.is_empty() {
                let (cur, lines) = lyrics::window(l, hub.position(), 2, 3);
                (2, cur, lines)
            } else {
                let (cur, lines) = lyrics::window_plain(l, hub.position(), t.len, 2, 3);
                (3, cur, lines)
            }
        }
    };
    b.u8(state).u8(cur as u8).u8(lines.len() as u8);
    for l in &lines {
        b.str(l, 90);
    }
    let sig = format!("{state}|{cur}|{}", lines.join("\n"));
    (b.0, sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playerctl_line() {
        let line = ["Playing", "Bohemian Rhapsody", "Queen", "A Night at the Opera", "61500000", "354000000", "file:///music/q.flac"]
            .join("\u{1f}");
        let t = parse(&line).unwrap();
        assert_eq!(t.status, 1);
        assert_eq!(t.artist, "Queen");
        assert!((t.pos - 61.5).abs() < 1e-6);
        assert!((t.len - 354.0).abs() < 1e-6);
        assert!(parse(&["Stopped", "", "", "", "", "", ""].join("\u{1f}")).is_none());
    }
}
