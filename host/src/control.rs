//! Control from the terminal: `aksara-hud next`, `aksara-hud rotate`, ...
//!
//! The running daemon owns the serial port, so the CLI cannot write to it.
//! Instead the daemon listens on a Unix socket ($XDG_RUNTIME_DIR/aksara-hud.sock);
//! the CLI sends one text line and prints the reply.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config;
use crate::proto;

/// The serial port of the current session (None while the HUD is unplugged).
pub type PortSlot = Arc<Mutex<Option<Arc<Mutex<File>>>>>;

/// Settings that can change while running (also saved to the config file).
pub struct Runtime {
    pub brightness: u8,
    pub rotation: u8,
}

pub const PAGE_NEXT: u8 = 0xFE;
pub const PAGE_PREV: u8 = 0xFD;

pub const USAGE: &str = "\
perintah:
  aksara-hud next | prev              halaman berikut / sebelumnya
  aksara-hud page <0|1|2>             0/beranda, 1/sistem, 2/cava
  aksara-hud rotate [1|3]             putar layar 180 derajat (disimpan)
  aksara-hud brightness <10-255>      kecerahan (disimpan)
  aksara-hud --dry-run                cek data tanpa ESP32";

pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aksara-hud.sock")
}

pub fn config_payload(rt: &Runtime, start_page: u8) -> Vec<u8> {
    vec![rt.brightness, start_page, rt.rotation]
}

fn parse_page(s: &str) -> Option<u8> {
    match s {
        "0" | "beranda" | "home" => Some(0),
        "1" | "sistem" | "system" => Some(1),
        "2" | "cava" => Some(2),
        _ => None,
    }
}

/// Turn one command line into (frame type, payload) or an error message.
fn build(line: &str, rt: &Mutex<Runtime>) -> Result<(u8, Vec<u8>, String), String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["next"] => Ok((proto::T_PAGE, vec![PAGE_NEXT], "halaman berikut".into())),
        ["prev"] => Ok((proto::T_PAGE, vec![PAGE_PREV], "halaman sebelumnya".into())),
        ["page", p] => {
            let n = parse_page(p).ok_or("halaman: 0/beranda, 1/sistem, 2/cava")?;
            Ok((proto::T_PAGE, vec![n], format!("halaman {n}")))
        }
        ["brightness", v] => {
            let v: u8 = v.parse().map_err(|_| "kecerahan harus 10-255")?;
            let v = v.max(10);
            let mut r = rt.lock().unwrap();
            r.brightness = v;
            config::set_key("brightness", &v.to_string());
            Ok((proto::T_CONFIG, config_payload(&r, 255), format!("kecerahan {v}")))
        }
        ["rotate"] | ["rotate", _] => {
            let mut r = rt.lock().unwrap();
            let new = match parts.get(1) {
                Some(&"1") => 1,
                Some(&"3") => 3,
                Some(_) => return Err("rotate hanya 1 atau 3 (landscape)".into()),
                None => if r.rotation == 1 { 3 } else { 1 },
            };
            r.rotation = new;
            config::set_key("rotation", &new.to_string());
            Ok((proto::T_CONFIG, config_payload(&r, 255), format!("rotasi {new}")))
        }
        _ => Err(USAGE.into()),
    }
}

fn handle(line: &str, slot: &PortSlot, rt: &Mutex<Runtime>) -> String {
    match build(line, rt) {
        Err(msg) => msg,
        Ok((kind, payload, done)) => match slot.lock().unwrap().as_ref() {
            None => format!("{done} disimpan, tapi HUD sedang tidak tersambung"),
            Some(port) => {
                let frame = proto::frame(kind, &payload);
                match port.lock().unwrap().write_all(&frame) {
                    Ok(()) => format!("ok: {done}"),
                    Err(e) => format!("gagal kirim: {e}"),
                }
            }
        },
    }
}

pub fn spawn_listener(slot: PortSlot, rt: Arc<Mutex<Runtime>>) {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!("[aksara-hud] daemon lain sudah memakai {}", path.display());
        return;
    }
    let _ = fs::remove_file(&path); // stale socket from a crash
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[aksara-hud] socket kontrol gagal: {e}");
            return;
        }
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_ok() {
                let reply = handle(line.trim(), &slot, &rt);
                let _ = (&stream).write_all(reply.as_bytes());
            }
        }
    });
}

/// CLI side. Returns the process exit code.
pub fn client(args: &[String]) -> i32 {
    let mut s = match UnixStream::connect(socket_path()) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("daemon aksara-hud tidak jalan. Colok HUD-nya atau: systemctl --user start aksara-hud");
            return 1;
        }
    };
    if writeln!(s, "{}", args.join(" ")).is_err() {
        return 1;
    }
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut reply = String::new();
    let _ = s.read_to_string(&mut reply);
    println!("{reply}");
    if reply.starts_with("ok") || reply.contains("disimpan") { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        let rt = Mutex::new(Runtime { brightness: 200, rotation: 1 });
        assert_eq!(build("next", &rt).unwrap().1, vec![PAGE_NEXT]);
        assert_eq!(build("page cava", &rt).unwrap().1, vec![2]);
        assert!(build("page 9", &rt).is_err());
        assert!(build("terbang", &rt).is_err());
    }
}
