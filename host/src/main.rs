//! hudlink — daemon that feeds an ESP32 status display over USB serial.
//!
//! Global threads : weather (Open-Meteo), media (playerctl + lyrics), control socket
//! Per session    : reader (READY / unplug), cava, and the main loop (5 ticks per second)

mod cava;
mod config;
mod control;
mod logo;
mod lyrics;
mod media;
mod osinfo;
mod proto;
mod sys;
mod theme;
mod timeinfo;
mod weather;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use config::Config;
use osinfo::OsInfo;
use weather::Weather;

type Port = Arc<Mutex<File>>;

/// State shared by every thread.
pub struct Shared {
    cfg: RwLock<Config>,
    cfg_ver: AtomicU64,
    os: OsInfo,
    weather: Mutex<Option<Weather>>,
    weather_ver: AtomicU64,
    media: Option<media::SharedHub>,
    port: Mutex<Option<Port>>,
}

impl Shared {
    fn cfg(&self) -> Config {
        self.cfg.read().unwrap().clone()
    }

    pub fn reload(&self) {
        *self.cfg.write().unwrap() = Config::load();
        self.cfg_ver.fetch_add(1, Ordering::Relaxed);
    }

    pub fn send_page(&self, id: u8, what: &str) -> String {
        match self.port.lock().unwrap().as_ref() {
            None => "the display is not connected".into(),
            Some(p) => match send(p, proto::T_PAGE, &[id]) {
                Ok(()) => format!("ok: {what}"),
                Err(e) => format!("send failed: {e}"),
            },
        }
    }
}

/// Configure the tty with `stty` (raw 8N1, no flow control, 0.5 s read timeout)
/// then open it as a normal file. Keeps the program free of serial crates.
fn open_port(path: &str, baud: u32) -> io::Result<File> {
    let status = Command::new("stty")
        .args(["-F", path, &baud.to_string(), "raw", "-echo", "-echoe", "-echok", "-hupcl", "clocal",
               "cs8", "-cstopb", "-parenb", "-ixon", "-ixoff", "-crtscts", "min", "0", "time", "5"])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("stty failed for {path}")));
    }
    OpenOptions::new().read(true).write(true).open(path)
}

fn send(port: &Mutex<File>, kind: u8, payload: &[u8]) -> io::Result<()> {
    let frame = proto::frame(kind, payload);
    port.lock().unwrap().write_all(&frame)
}

fn layout_payload(cfg: &Config, os: &OsInfo) -> Vec<u8> {
    let idx = |v: &str, list: &[&str]| list.iter().position(|x| *x == v).unwrap_or(0) as u8;
    let mut b = proto::Buf::default();
    b.u8(idx(&cfg.clock_style, &["segment", "digits", "text"]))
        .u8(cfg.clock_seconds as u8)
        .u8(idx(&cfg.home_layout, &["dashboard", "clock", "music"]))
        .u8(idx(&cfg.cava_style, &["bars", "blocks", "mirror"]));
    let ids: Vec<u8> = cfg.pages.iter().filter_map(|p| control::page_id(p)).collect();
    b.u8(ids.len() as u8);
    for id in ids {
        b.u8(id);
    }
    let title = if cfg.title == "auto" { osinfo::short_title(os) } else { cfg.title.clone() };
    b.str(&title, 20);
    b.0
}

/// Everything that depends on config/theme. Re-sent on reload or when matugen changes.
struct Look {
    palette: Option<theme::Palette>,
    logo_key: String,
    matugen_mtime: Option<SystemTime>,
    checked: Instant,
}

fn matugen_mtime(cfg: &Config) -> Option<SystemTime> {
    std::fs::metadata(config::expand(&cfg.matugen_file)).and_then(|m| m.modified()).ok()
}

fn send_look(port: &Port, cfg: &Config, sh: &Shared, look: &mut Look, force: bool) -> io::Result<()> {
    let (pal, src) = theme::resolve(cfg, &sh.os);
    if force || look.palette.as_ref() != Some(&pal) {
        eprintln!("[hudlink] theme: {src}");
        send(port, proto::T_THEME, &theme::payload(&pal))?;
    }
    let logo_path = logo::find(&cfg.logo, &sh.os);
    let key = format!("{:?}{:?}", logo_path, pal.bg);
    if force || key != look.logo_key {
        let (payload, info) = logo::payload(logo_path.as_deref(), pal.bg);
        eprintln!("[hudlink] logo: {info}");
        send(port, proto::T_LOGO, &payload)?;
        look.logo_key = key;
    }
    look.palette = Some(pal);
    look.matugen_mtime = matugen_mtime(cfg);
    Ok(())
}

fn run_session(sh: &Arc<Shared>) -> io::Result<()> {
    let cfg0 = sh.cfg();
    let file = open_port(&cfg0.port, cfg0.baud)?;
    eprintln!("[hudlink] connected to {} @ {}", cfg0.port, cfg0.baud);
    let port: Port = Arc::new(Mutex::new(file.try_clone()?));
    let alive = Arc::new(AtomicBool::new(true));

    // --- reader thread: wait for READY, detect unplug ---
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let reader = {
        let alive = alive.clone();
        let path = cfg0.port.clone();
        let mut rfile = file;
        thread::spawn(move || {
            let mut buf = [0u8; 256];
            let mut line = Vec::new();
            while alive.load(Ordering::Relaxed) {
                match rfile.read(&mut buf) {
                    Ok(0) => {
                        if !Path::new(&path).exists() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if b == b'\n' {
                                if String::from_utf8_lossy(&line).contains("READY") {
                                    let _ = ready_tx.send(());
                                }
                                line.clear();
                            } else if line.len() < 128 {
                                line.push(b);
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            alive.store(false, Ordering::Relaxed);
        })
    };

    // Opening the port may reset the ESP32 (DTR/RTS). Give it time to boot.
    match ready_rx.recv_timeout(Duration::from_secs(4)) {
        Ok(()) => eprintln!("[hudlink] display ready"),
        Err(_) => eprintln!("[hudlink] no READY from the display, continuing anyway"),
    }
    thread::sleep(Duration::from_millis(150));

    // --- initial state ---
    let mut cfg = sh.cfg();
    let mut cfg_ver = sh.cfg_ver.load(Ordering::Relaxed);
    send(&port, proto::T_CONFIG, &[cfg.brightness, cfg.start_page, cfg.rotation])?;
    send(&port, proto::T_LAYOUT, &layout_payload(&cfg, &sh.os))?;
    let mut look = Look { palette: None, logo_key: String::new(), matugen_mtime: None, checked: Instant::now() };
    send_look(&port, &cfg, sh, &mut look, true)?;
    *sh.port.lock().unwrap() = Some(port.clone());

    // --- cava thread: restarts itself when cava settings change ---
    let cava_thread = {
        let (port, alive, sh) = (port.clone(), alive.clone(), sh.clone());
        thread::spawn(move || {
            while alive.load(Ordering::Relaxed) {
                let c = sh.cfg();
                if !c.cava {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let n = c.cava_bars as u8;
                let mut payload = Vec::with_capacity(c.cava_bars + 1);
                let res = cava::run(&c, |bars| {
                    if !alive.load(Ordering::Relaxed) || c.cava_changed(&sh.cfg()) {
                        return false;
                    }
                    payload.clear();
                    payload.push(n);
                    payload.extend_from_slice(bars);
                    if send(&port, proto::T_CAVA, &payload).is_err() {
                        alive.store(false, Ordering::Relaxed);
                        return false;
                    }
                    true
                });
                if res.is_err() {
                    eprintln!("[hudlink] cava disabled until the config changes; everything else keeps running");
                    while alive.load(Ordering::Relaxed) && !c.cava_changed(&sh.cfg()) {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        })
    };

    // --- main loop: 5 ticks per second ---
    let mut sysmon = sys::SysMon::new(cfg.temp_sensor.as_deref());
    let mut tz = timeinfo::Tz::new();
    let mut sent_weather = 0u64;
    let mut last_sec = u64::MAX;
    let mut seconds = 0u64;
    let mut lyrics_sig = String::new();
    let mut media_sig = String::new();
    let result: io::Result<()> = (|| {
        loop {
            if !alive.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "display disconnected"));
            }

            // config reloaded?
            let ver = sh.cfg_ver.load(Ordering::Relaxed);
            if ver != cfg_ver {
                cfg_ver = ver;
                cfg = sh.cfg();
                send(&port, proto::T_CONFIG, &[cfg.brightness, 255, cfg.rotation])?;
                send(&port, proto::T_LAYOUT, &layout_payload(&cfg, &sh.os))?;
                send_look(&port, &cfg, sh, &mut look, false)?;
                sent_weather = 0; // weather text may change language
            }
            // matugen wrote new colors? (wallpaper changed)
            if look.checked.elapsed() > Duration::from_secs(2) {
                look.checked = Instant::now();
                if matugen_mtime(&cfg) != look.matugen_mtime {
                    send_look(&port, &cfg, sh, &mut look, false)?;
                }
            }

            // media + lyrics (checked every tick, sent only when changed)
            if let (Some(hub), true) = (&sh.media, cfg.media) {
                let hub = hub.lock().unwrap();
                let mp = media::payload(&hub);
                let sig = format!("{:?}", mp);
                if sig != media_sig {
                    send(&port, proto::T_MEDIA, &mp)?;
                    media_sig = sig;
                }
                if cfg.lyrics {
                    let (lp, sig) = media::lyrics_payload(&hub);
                    if sig != lyrics_sig {
                        send(&port, proto::T_LYRICS, &lp)?;
                        lyrics_sig = sig;
                    }
                }
            }

            // once per wall-clock second
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            if now.as_secs() != last_sec {
                last_sec = now.as_secs();
                seconds += 1;
                let t = timeinfo::now(&mut tz);
                send(&port, proto::T_TIME, &timeinfo::payload(&t, cfg.clock_24h, &cfg.date_line1, &cfg.date_line2, &cfg.language))?;
                send(&port, proto::T_SYS, &sys::payload(&sysmon.sample()))?;
                let wv = sh.weather_ver.load(Ordering::Relaxed);
                if wv != sent_weather {
                    if let Some(w) = sh.weather.lock().unwrap().clone() {
                        send(&port, proto::T_WEATHER, &weather::payload(&w, &cfg.language))?;
                    }
                    sent_weather = wv;
                }
                if cfg.auto_page > 0 && seconds % cfg.auto_page == 0 {
                    send(&port, proto::T_PAGE, &[control::PAGE_NEXT])?;
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
    })();

    *sh.port.lock().unwrap() = None;
    alive.store(false, Ordering::Relaxed);
    let _ = reader.join();
    let _ = cava_thread.join();
    result
}

fn spawn_weather(sh: Arc<Shared>) {
    thread::spawn(move || {
        let mut place: Option<(f64, f64, String)> = None;
        let mut place_for = String::new();
        let mut next_fetch = Instant::now();
        loop {
            let cfg = sh.cfg();
            let key = format!("{}|{:?}|{:?}", cfg.city, cfg.latitude, cfg.longitude);
            if key != place_for {
                place_for = key;
                place = match (cfg.latitude, cfg.longitude) {
                    (Some(la), Some(lo)) => Some((la, lo, if cfg.city.is_empty() { "Location".into() } else { cfg.city.clone() })),
                    _ if cfg.city.is_empty() => None,
                    _ => {
                        let p = weather::geocode(&cfg.city);
                        match &p {
                            Some((la, lo, n)) => eprintln!("[hudlink] city: {n} ({la:.3}, {lo:.3})"),
                            None => eprintln!("[hudlink] city \"{}\" not found (or offline)", cfg.city),
                        }
                        p
                    }
                };
                next_fetch = Instant::now();
            }
            if Instant::now() >= next_fetch {
                let ok = match &place {
                    Some((la, lo, name)) => match weather::fetch(*la, *lo, name) {
                        Some(w) => {
                            *sh.weather.lock().unwrap() = Some(w);
                            sh.weather_ver.fetch_add(1, Ordering::Relaxed);
                            true
                        }
                        None => false,
                    },
                    None => true, // weather off
                };
                let mins = if ok { cfg.weather_minutes } else { 2 };
                next_fetch = Instant::now() + Duration::from_secs(mins * 60);
                if place.is_none() && !cfg.city.is_empty() {
                    place_for.clear(); // retry geocoding later
                }
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

fn dry_run(cfg: &Config) {
    let os = osinfo::read();
    println!("== hudlink --dry-run ==");
    println!("distro  : {} (id {}), title {}", os.name, os.id, osinfo::short_title(&os));
    let (pal, src) = theme::resolve(cfg, &os);
    println!("theme   : {src}  accent {:02x}{:02x}{:02x}", pal.accent.0, pal.accent.1, pal.accent.2);
    match logo::find(&cfg.logo, &os) {
        Some(p) => println!("logo    : {} ({})", p.display(), logo::payload(Some(&p), pal.bg).1),
        None => println!("logo    : none found"),
    }
    let mut tz = timeinfo::Tz::new();
    let t = timeinfo::now(&mut tz);
    println!("time    : {:02}:{:02}:{:02}  {} | {}", t.h, t.m, t.s,
             timeinfo::format_date(&cfg.date_line1, &t, &cfg.language),
             timeinfo::format_date(&cfg.date_line2, &t, &cfg.language));
    let mut m = sys::SysMon::new(cfg.temp_sensor.as_deref());
    thread::sleep(Duration::from_secs(1));
    let s = m.sample();
    println!("cpu     : {}% @ {} MHz  cores={:?}  load {:.2}  {} processes", s.cpu, s.cpu_mhz, s.cores, s.load1, s.nprocs);
    println!("memory  : {} / {} MiB   swap {} / {} MiB", s.ram_used_mb, s.ram_total_mb, s.swap_used_mb, s.swap_total_mb);
    println!("sensors : temp {:?} C  fan {:?} rpm  gpu {:?} MHz", s.temp_c, s.fan_rpm, s.gpu_mhz);
    println!("io      : net rx {} tx {} KB/s   disk r {} w {} KB/s   disk {}%", s.rx_kbs, s.tx_kbs, s.disk_r_kbs, s.disk_w_kbs, s.disk_pct);
    println!("battery : {:?} (state {})  uptime {} s", s.bat_pct, s.bat_state, s.uptime_s);
    for p in &s.procs {
        println!("process : {:>7}  {:<16} {:>5.1}%  {} MiB", p.pid, p.name, p.cpu, p.mem_mb);
    }
    let hub = media::spawn(cfg.lyrics);
    thread::sleep(Duration::from_millis(700));
    match &hub.lock().unwrap().track {
        Some(t) => println!("media   : {} - {} [{:.0}/{:.0}s]", t.artist, t.title, t.pos, t.len),
        None => println!("media   : nothing playing (or playerctl missing)"),
    }
    if !cfg.city.is_empty() || cfg.latitude.is_some() {
        let place = match (cfg.latitude, cfg.longitude) {
            (Some(la), Some(lo)) => Some((la, lo, cfg.city.clone())),
            _ => weather::geocode(&cfg.city),
        };
        match place.and_then(|(la, lo, n)| weather::fetch(la, lo, &n)) {
            Some(w) => println!("weather : {} {:.1} C, {}", w.city, w.temp, weather::describe(w.code, &cfg.language)),
            None => println!("weather : failed (check internet / city name)"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first() {
        if control::is_client_command(first) {
            std::process::exit(control::client(&args));
        }
    }
    let cfg = Config::load();
    if args.iter().any(|a| a == "--dry-run") {
        dry_run(&cfg);
        return;
    }
    let os = osinfo::read();
    eprintln!("[hudlink] {} | config {}", os.name, Config::path().display());
    let media = cfg.media.then(|| media::spawn(cfg.lyrics));
    let sh = Arc::new(Shared {
        cfg: RwLock::new(cfg),
        cfg_ver: AtomicU64::new(0),
        os,
        weather: Mutex::new(None),
        weather_ver: AtomicU64::new(0),
        media,
        port: Mutex::new(None),
    });
    spawn_weather(sh.clone());
    control::spawn_listener(sh.clone());

    loop {
        if let Err(e) = run_session(&sh) {
            eprintln!("[hudlink] session ended: {e}");
        }
        thread::sleep(Duration::from_secs(2));
    }
}
