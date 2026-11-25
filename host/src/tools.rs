//! `hudlink doctor` (checks the whole setup) and `hudlink setup` (first-run wizard).

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::Command;

use crate::config::{self, Config, PAGE_NAMES};
use crate::control::ask_daemon;
use crate::theme::PRESETS;
use crate::{logo, osinfo, sys, theme, weather, FIRMWARE};

// ---------------- doctor ----------------

struct Report {
    problems: u32,
}

impl Report {
    fn ok(&self, what: &str) {
        println!("  ok  {what}");
    }
    fn warn(&mut self, what: &str, hint: &str) {
        self.problems += 1;
        println!("  !!  {what}");
        if !hint.is_empty() {
            println!("      -> {hint}");
        }
    }
    fn info(&self, what: &str) {
        println!("  --  {what}");
    }
}

pub fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

fn run_ok(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).output().map_or(false, |o| o.status.success())
}

fn run_out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn doctor(online: bool) -> i32 {
    let mut r = Report { problems: 0 };
    let os = osinfo::read();
    println!("hudlink doctor  ({}, daemon {})\n", os.name, env!("CARGO_PKG_VERSION"));

    println!("config");
    let path = Config::path();
    if path.exists() {
        r.ok(&format!("{}", path.display()));
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut probe = Config::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if let Some((k, v)) = line.split_once('=') {
                if !probe.apply(k.trim(), v.trim()) {
                    r.warn(&format!("unknown key \"{}\"", k.trim()), "remove it, or see `hudlink get` for valid keys");
                }
            }
        }
    } else {
        r.info("no config yet (created on first run, or run `hudlink setup`)");
    }
    let cfg = Config::load();

    println!("\nprograms");
    for (cmd, why) in [("stty", "serial port setup"), ("curl", "weather and lyrics"), ("df", "disk usage")] {
        if have(cmd) { r.ok(cmd) } else { r.warn(&format!("{cmd} missing ({why})"), "install it with your package manager") }
    }
    for (cmd, why, needed) in [("cava", "visualizer", cfg.cava), ("playerctl", "now playing + lyrics", cfg.media)] {
        if !needed {
            r.info(&format!("{cmd}: disabled in config"));
        } else if have(cmd) {
            r.ok(cmd);
        } else {
            r.warn(&format!("{cmd} missing ({why})"), "run scripts/install.sh, or install it with your package manager");
        }
    }
    if have("magick") || have("convert") { r.ok("ImageMagick") } else { r.warn("ImageMagick missing (distro logo)", "install ImageMagick, or set logo = none") }

    println!("\nhardware");
    let chip = ["1a86:7523", "10c4:ea60", "1a86:55d4"].iter().find(|id| run_ok("lsusb", &["-d", id]));
    match chip {
        Some(id) => r.ok(&format!("USB serial chip {id} found")),
        None if have("lsusb") => r.warn("no CH340/CP2102 USB chip found", "plug the ESP32 in (use a data cable, not a charge-only one)"),
        None => r.info("lsusb not installed, skipped USB check"),
    }
    let rule = Path::new("/etc/udev/rules.d/70-hudlink.rules");
    match fs::read_to_string(rule) {
        Ok(t) if t.contains("__VID__") => r.warn("udev rule has no USB id filled in", "run scripts/install.sh again with the ESP32 plugged in"),
        Ok(_) => r.ok("udev rule installed"),
        Err(_) => r.warn("udev rule missing", "run scripts/install.sh (creates /dev/hudlink)"),
    }
    if Path::new(&cfg.port).exists() {
        if run_ok("test", &["-r", &cfg.port, "-a", "-w", &cfg.port]) {
            r.ok(&format!("{} is accessible", cfg.port));
        } else {
            r.warn(&format!("{} exists but you cannot open it", cfg.port), "log out and back in (uaccess), or add yourself to the dialout group");
        }
    } else {
        r.warn(&format!("{} not present", cfg.port), "plug the display in; if it stays missing, check the udev rule");
    }

    println!("\ndaemon");
    if have("systemctl") && run_ok("systemctl", &["--user", "--version"]) {
        let enabled = run_out("systemctl", &["--user", "is-enabled", "hudlink.service"]);
        let active = run_out("systemctl", &["--user", "is-active", "hudlink.service"]);
        match enabled.as_str() {
            "enabled" => r.ok("service enabled"),
            "" | "not-found" => r.warn("service not installed", "run scripts/install.sh"),
            other => r.warn(&format!("service is {other}"), "systemctl --user enable hudlink.service"),
        }
        if !active.is_empty() {
            r.info(&format!("service is {active}"));
        }
    } else if config::home().join(".config/autostart/hudlink.desktop").exists() {
        r.ok("autostart entry installed (no systemd)");
    } else {
        r.warn("no service and no autostart entry", "run scripts/install.sh, or start `hudlink` from your WM's autostart");
    }
    match ask_daemon("status") {
        Some(status) => {
            r.ok("daemon answers on the control socket");
            let fw = status.lines().find_map(|l| l.strip_prefix("firmware")).map(str::trim).unwrap_or("unknown");
            if fw == FIRMWARE {
                r.ok(&format!("firmware {fw}"));
            } else if fw == "unknown" {
                r.info("firmware version unknown (display not connected?)");
            } else {
                r.warn(&format!("firmware {fw}, this daemon expects {FIRMWARE}"), "run scripts/flash.sh");
            }
        }
        None => r.warn("daemon not running", "plug the display in, or: systemctl --user start hudlink"),
    }

    println!("\nlook");
    let (_, src) = theme::resolve(&cfg, &os);
    if cfg.theme == "matugen" && !config::expand(&cfg.matugen_file).exists() {
        r.warn(&format!("theme matugen but {} does not exist", cfg.matugen_file), "add the hudlink template to matugen (see README)");
    } else {
        r.ok(&format!("theme: {src}"));
    }
    match logo::find(&cfg.logo, &os) {
        Some(p) => r.ok(&format!("logo: {}", p.display())),
        None if cfg.logo == "none" => r.info("logo disabled"),
        None => r.warn("no logo found", "set logo = /path/to/logo.png"),
    }

    println!("\ndata");
    match sys::find_temp_sensor() {
        Some(p) => r.ok(&format!("temperature sensor {}", p.display())),
        None => r.warn("no temperature sensor", "set temp_sensor = /sys/class/hwmon/hwmonN/temp1_input"),
    }
    if cfg.media && have("playerctl") {
        let players = run_out("playerctl", &["-l"]);
        if players.is_empty() { r.info("no media player running right now") } else { r.ok(&format!("players: {}", players.replace('\n', ", "))) }
    }
    if cfg.city.is_empty() && cfg.latitude.is_none() {
        r.info("weather off (no city)");
    } else if online {
        let place = match (cfg.latitude, cfg.longitude) {
            (Some(la), Some(lo)) => Some((la, lo, cfg.city.clone())),
            _ => weather::geocode(&cfg.city),
        };
        match place.and_then(|(la, lo, n)| weather::fetch(la, lo, &n)) {
            Some(w) => r.ok(&format!("weather: {} {:.1} C", w.city, w.temp)),
            None => r.warn("weather request failed", "check the internet connection and the city name"),
        }
    } else {
        r.info(&format!("weather city: {} (add --online to test the request)", if cfg.city.is_empty() { "coordinates" } else { &cfg.city }));
    }

    println!();
    if r.problems == 0 {
        println!("everything looks good");
        0
    } else {
        println!("{} thing(s) to fix", r.problems);
        1
    }
}

// ---------------- setup wizard ----------------

fn ask(question: &str, current: &str) -> String {
    print!("{question} [{current}]: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return current.to_string();
    }
    let v = line.trim();
    if v.is_empty() { current.to_string() } else { v.to_string() }
}

/// Ask until the answer passes config::check.
fn ask_valid(key: &str, question: &str, current: &str) -> String {
    loop {
        let v = ask(question, current);
        match config::check(key, &v) {
            Ok(()) => return v,
            Err(e) => println!("  {e}"),
        }
    }
}

fn yes(question: &str, default: bool) -> bool {
    let v = ask(question, if default { "Y/n" } else { "y/N" }).to_lowercase();
    match v.as_str() {
        "y" | "yes" | "ya" => true,
        "n" | "no" | "tidak" => false,
        _ => default,
    }
}

pub fn setup() -> i32 {
    let cfg = Config::load();
    println!("hudlink setup  (press Enter to keep the value in brackets)\n");
    let mut changes: Vec<(String, String)> = Vec::new();
    let mut set = |k: &str, v: String| changes.push((k.to_string(), v));

    let city = ask("City for the weather (empty = off)", if cfg.city.is_empty() { "-" } else { &cfg.city });
    if city == "-" || city.is_empty() {
        set("city", String::new());
        set("latitude", String::new());
        set("longitude", String::new());
    } else if city != cfg.city || cfg.latitude.is_none() {
        print!("looking up \"{city}\"... ");
        let _ = io::stdout().flush();
        match weather::geocode(&city) {
            Some((la, lo, name)) => {
                println!("found {name} ({la:.4}, {lo:.4})");
                set("city", city.clone());
                if yes("Save these coordinates (more precise)?", true) {
                    set("latitude", format!("{la:.4}"));
                    set("longitude", format!("{lo:.4}"));
                }
            }
            None => {
                println!("not found (or offline). Saving the name; you can set latitude/longitude later.");
                set("city", city.clone());
            }
        }
    }

    set("language", ask_valid("language", "Language for day and month names (en/id)", &cfg.language));
    set("clock_24h", yes("24-hour clock?", cfg.clock_24h).to_string());
    set("clock_seconds", yes("Show seconds?", cfg.clock_seconds).to_string());
    set("clock_style", ask_valid("clock_style", "Clock style (segment/digits/text)", &cfg.clock_style));

    println!("themes: auto, matugen, distro, {}", PRESETS.join(", "));
    set("theme", ask_valid("theme", "Theme", &cfg.theme));
    set("home_layout", ask_valid("home_layout", "Home layout (dashboard/clock/music)", &cfg.home_layout));
    println!("pages: {}", PAGE_NAMES.join(", "));
    set("pages", ask_valid("pages", "Pages to show, in order", &cfg.pages.join(", ")));
    set("rotation", if yes("Is the screen upside down (rotate 180)?", cfg.rotation == 3) { "3".into() } else { "1".into() });

    let mut saved = 0;
    for (k, v) in &changes {
        if config::check(k, v).is_ok() {
            config::set_key(k, v);
            saved += 1;
        }
    }
    println!("\nsaved {saved} settings to {}", Config::path().display());
    match ask_daemon("reload") {
        Some(_) => println!("the display is updated"),
        None => println!("the daemon is not running; the settings apply when the display is plugged in"),
    }
    0
}
