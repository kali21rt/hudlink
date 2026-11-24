//! Terminal control. The daemon owns the serial port, so the CLI talks to it through a
//! Unix socket ($XDG_RUNTIME_DIR/hudlink.sock). Config edits are written to the config
//! file by the CLI itself, then the daemon is told to reload.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::thread;

use crate::config::{self, Config, PAGE_NAMES};
use crate::theme::PRESETS;
use crate::Shared;

pub const PAGE_NEXT: u8 = 0xFE;
pub const PAGE_PREV: u8 = 0xFD;

pub const USAGE: &str = "\
hudlink - ESP32 status display for Linux

usage:
  hudlink                         run the daemon (normally started by systemd)
  hudlink next | prev             next / previous page
  hudlink page <name>             home, system, process, cava, lyrics
  hudlink set <key> <value>       change a setting live (saved to the config)
  hudlink get [key]               show settings
  hudlink themes                  list color themes
  hudlink edit                    open the config in $EDITOR, then reload
  hudlink reload                  re-read the config file
  hudlink rotate                  flip the screen 180 degrees
  hudlink brightness <10-255>     backlight level
  hudlink --dry-run               print all data once, no ESP32 needed

examples:
  hudlink set theme forest
  hudlink set clock_style digits
  hudlink set cava_bars 48
  hudlink set pages home, cava, lyrics";

pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("hudlink.sock")
}

pub fn page_id(name: &str) -> Option<u8> {
    if let Ok(n) = name.parse::<u8>() {
        return (n < PAGE_NAMES.len() as u8).then_some(n);
    }
    PAGE_NAMES.iter().position(|p| *p == name).map(|i| i as u8)
}

// ---------------- daemon side ----------------

fn handle(line: &str, shared: &Shared) -> String {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["next"] => shared.send_page(PAGE_NEXT, "next page"),
        ["prev"] => shared.send_page(PAGE_PREV, "previous page"),
        ["page", p] => match page_id(p) {
            Some(id) => shared.send_page(id, &format!("page {}", PAGE_NAMES[id as usize])),
            None => format!("unknown page \"{p}\" (use: {})", PAGE_NAMES.join(", ")),
        },
        ["reload"] => {
            shared.reload();
            "ok: config reloaded".into()
        }
        _ => format!("unknown command\n{USAGE}"),
    }
}

pub fn spawn_listener(shared: Arc<Shared>) {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!("[hudlink] another daemon is already using {}", path.display());
        return;
    }
    let _ = fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[hudlink] control socket failed: {e}");
            return;
        }
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_ok() {
                let reply = handle(line.trim(), &shared);
                let _ = (&stream).write_all(reply.as_bytes());
            }
        }
    });
}

// ---------------- CLI side ----------------

fn ask_daemon(line: &str) -> Option<String> {
    let mut s = UnixStream::connect(socket_path()).ok()?;
    writeln!(s, "{line}").ok()?;
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut reply = String::new();
    s.read_to_string(&mut reply).ok()?;
    Some(reply)
}

const NOT_RUNNING: &str = "the hudlink daemon is not running (plug the display in, or: systemctl --user start hudlink)";

/// Validate, save, and ask the daemon to reload.
fn set(key: &str, value: &str) -> i32 {
    let mut probe = Config::load();
    if !probe.apply(key, value) {
        eprintln!("unknown key \"{key}\". See: hudlink get");
        return 1;
    }
    if key == "theme" && !["auto", "matugen", "distro"].contains(&value) && !PRESETS.contains(&value) {
        eprintln!("unknown theme \"{value}\". See: hudlink themes");
        return 1;
    }
    config::set_key(key, value);
    match ask_daemon("reload") {
        Some(_) => println!("ok: {key} = {value}"),
        None => println!("saved {key} = {value} (daemon not running, applies on next start)"),
    }
    0
}

pub fn client(args: &[String]) -> i32 {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match a.as_slice() {
        ["set", key, rest @ ..] if !rest.is_empty() => set(key, &rest.join(" ")),
        ["rotate"] => {
            let cur = Config::load().rotation;
            set("rotation", if cur == 3 { "1" } else { "3" })
        }
        ["rotate", v] => set("rotation", v),
        ["brightness", v] => set("brightness", v),
        ["get"] => {
            let text = fs::read_to_string(Config::path()).unwrap_or_default();
            for l in text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')) {
                println!("{l}");
            }
            0
        }
        ["get", key] => {
            let text = fs::read_to_string(Config::path()).unwrap_or_default();
            match text.lines().find(|l| l.split_once('=').map_or(false, |(k, _)| k.trim() == *key) && !l.starts_with('#')) {
                Some(l) => println!("{l}"),
                None => println!("{key} is not set (default)"),
            }
            0
        }
        ["themes"] => {
            println!("auto      matugen file if present, otherwise distro color");
            println!("matugen   colors from your wallpaper (needs the matugen template)");
            println!("distro    accent from /etc/os-release");
            for p in PRESETS {
                println!("{p}");
            }
            0
        }
        ["edit"] => {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
            let _ = Config::load(); // make sure the file exists
            let ok = Command::new(&editor).arg(Config::path()).status().map_or(false, |s| s.success());
            if ok {
                println!("{}", ask_daemon("reload").unwrap_or_else(|| NOT_RUNNING.into()));
            }
            0
        }
        [cmd, ..] if ["next", "prev", "page", "reload"].contains(cmd) => match ask_daemon(&a.join(" ")) {
            Some(r) => {
                println!("{r}");
                if r.starts_with("ok") { 0 } else { 1 }
            }
            None => {
                eprintln!("{NOT_RUNNING}");
                1
            }
        },
        _ => {
            println!("{USAGE}");
            1
        }
    }
}

pub fn is_client_command(first: &str) -> bool {
    ["next", "prev", "page", "set", "get", "themes", "edit", "reload", "rotate", "brightness", "help", "-h", "--help"]
        .contains(&first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names() {
        assert_eq!(page_id("lyrics"), Some(4));
        assert_eq!(page_id("2"), Some(2));
        assert_eq!(page_id("9"), None);
        assert_eq!(page_id("nope"), None);
    }
}
