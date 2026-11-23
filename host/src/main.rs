//! aksara-hud — daemon that feeds the ESP32 "Aksara HUD" over USB serial.
//!
//! Threads per session:
//!   main    : time + system stats every second, pushes new weather
//!   reader  : watches the port for "READY" lines from the ESP32
//!   cava    : runs cava, forwards each frame (~30 fps)
//! Global:
//!   weather : refreshes Open-Meteo every N minutes

mod cava;
mod config;
mod control;
mod proto;
mod sys;
mod timeinfo;
mod weather;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use config::Config;
use control::{PortSlot, Runtime};
use weather::Weather;

/// Latest weather + a version counter so sessions know when to resend.
#[derive(Default)]
struct WeatherState {
    data: Mutex<Option<Weather>>,
    version: AtomicU64,
}

/// Configure the tty with `stty` (raw 8N1, no flow control, 0.5 s read timeout)
/// then open it as a normal file. Keeps the program free of serial crates.
fn open_port(path: &str, baud: u32) -> io::Result<File> {
    let status = Command::new("stty")
        .args(["-F", path, &baud.to_string(), "raw", "-echo", "-echoe", "-echok", "-hupcl", "clocal",
               "cs8", "-cstopb", "-parenb", "-ixon", "-ixoff", "-crtscts", "min", "0", "time", "5"])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("stty gagal untuk {path}")));
    }
    OpenOptions::new().read(true).write(true).open(path)
}

fn send(port: &Mutex<File>, kind: u8, payload: &[u8]) -> io::Result<()> {
    let frame = proto::frame(kind, payload);
    let mut f = port.lock().unwrap();
    f.write_all(&frame)
}

fn run_session(cfg: &Config, weather: &Arc<WeatherState>, slot: &PortSlot, rt: &Arc<Mutex<Runtime>>) -> io::Result<()> {
    let file = open_port(&cfg.port, cfg.baud)?;
    eprintln!("[aksara-hud] terhubung ke {} @ {}", cfg.port, cfg.baud);
    let port = Arc::new(Mutex::new(file.try_clone()?));
    let alive = Arc::new(AtomicBool::new(true));

    // --- reader thread: wait for READY, detect unplug ---
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let reader = {
        let alive = alive.clone();
        let path = cfg.port.clone();
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
        Ok(()) => eprintln!("[aksara-hud] ESP32 siap"),
        Err(_) => eprintln!("[aksara-hud] tidak ada READY, lanjut saja"),
    }
    thread::sleep(Duration::from_millis(150));

    let config = control::config_payload(&rt.lock().unwrap(), cfg.start_page);
    send(&port, proto::T_CONFIG, &config)?;
    *slot.lock().unwrap() = Some(port.clone()); // terminal commands can reach the HUD now

    // --- cava thread ---
    let cava_thread = cfg.cava.then(|| {
        let (port, alive, cfg) = (port.clone(), alive.clone(), cfg.clone());
        thread::spawn(move || {
            let n = cfg.cava_bars as u8;
            let mut payload = Vec::with_capacity(cfg.cava_bars + 1);
            let res = cava::run(&cfg, |bars| {
                if !alive.load(Ordering::Relaxed) {
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
            if res.is_err() && alive.load(Ordering::Relaxed) {
                eprintln!("[aksara-hud] cava dimatikan untuk sesi ini; jam/sistem/cuaca tetap jalan");
            }
        })
    });

    // --- main loop: 1 Hz ---
    let mut sysmon = sys::SysMon::new(cfg.temp_sensor.as_deref());
    let mut tz = timeinfo::Tz::new();
    let mut sent_weather = 0u64;
    let mut seconds = 0u64;
    let result = loop {
        if !alive.load(Ordering::Relaxed) {
            break Err(io::Error::new(io::ErrorKind::BrokenPipe, "perangkat terputus"));
        }
        let t = timeinfo::now(&mut tz);
        if let Err(e) = send(&port, proto::T_TIME, &timeinfo::payload(&t)) {
            break Err(e);
        }
        let snap = sysmon.sample();
        if let Err(e) = send(&port, proto::T_SYS, &sys::payload(&snap)) {
            break Err(e);
        }
        let ver = weather.version.load(Ordering::Relaxed);
        if ver != sent_weather {
            if let Some(w) = weather.data.lock().unwrap().clone() {
                if let Err(e) = send(&port, proto::T_WEATHER, &weather::payload(&w)) {
                    break Err(e);
                }
            }
            sent_weather = ver;
        }
        seconds += 1;
        if cfg.auto_page > 0 && seconds % cfg.auto_page == 0 {
            if let Err(e) = send(&port, proto::T_PAGE, &[control::PAGE_NEXT]) {
                break Err(e);
            }
        }
        // sleep until the next whole second so the clock ticks on time
        let ms_into_sec = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_millis())
            .unwrap_or(0)) as u64;
        thread::sleep(Duration::from_millis(1000 - ms_into_sec.min(999) + 5));
    };

    *slot.lock().unwrap() = None;
    alive.store(false, Ordering::Relaxed);
    let _ = reader.join();
    if let Some(h) = cava_thread {
        let _ = h.join();
    }
    result
}

fn spawn_weather(cfg: &Config, state: Arc<WeatherState>) {
    if cfg.city.is_empty() && cfg.latitude.is_none() {
        eprintln!("[aksara-hud] cuaca dimatikan (city kosong)");
        return;
    }
    let cfg = cfg.clone();
    thread::spawn(move || {
        let mut place: Option<(f64, f64, String)> = match (cfg.latitude, cfg.longitude) {
            (Some(la), Some(lo)) => Some((la, lo, if cfg.city.is_empty() { "Lokasi".into() } else { cfg.city.clone() })),
            _ => None,
        };
        loop {
            if place.is_none() {
                place = weather::geocode(&cfg.city);
                match &place {
                    Some((la, lo, n)) => eprintln!("[aksara-hud] kota: {n} ({la:.3}, {lo:.3})"),
                    None => eprintln!("[aksara-hud] kota \"{}\" tidak ditemukan / offline", cfg.city),
                }
            }
            let ok = match &place {
                Some((la, lo, name)) => match weather::fetch(*la, *lo, name) {
                    Some(w) => {
                        *state.data.lock().unwrap() = Some(w);
                        state.version.fetch_add(1, Ordering::Relaxed);
                        true
                    }
                    None => false,
                },
                None => false,
            };
            // retry sooner when offline
            let mins = if ok { cfg.weather_minutes } else { 2 };
            thread::sleep(Duration::from_secs(mins * 60));
        }
    });
}

fn dry_run(cfg: &Config) {
    println!("== aksara-hud --dry-run ==");
    let mut tz = timeinfo::Tz::new();
    let t = timeinfo::now(&mut tz);
    const PASARAN: [&str; 5] = ["Legi", "Pahing", "Pon", "Wage", "Kliwon"];
    println!("waktu   : {:02}:{:02}:{:02}  {}-{:02}-{:02}  wday={} pasaran={}",
             t.h, t.m, t.s, t.year, t.month, t.day, t.wday, PASARAN[t.pasaran as usize]);
    let mut m = sys::SysMon::new(cfg.temp_sensor.as_deref());
    thread::sleep(Duration::from_secs(1));
    let s = m.sample();
    println!("cpu     : {}%  cores={:?}", s.cpu, s.cores);
    println!("ram     : {} / {} MiB   swap {} / {} MiB", s.ram_used_mb, s.ram_total_mb, s.swap_used_mb, s.swap_total_mb);
    println!("suhu    : {:?} C   disk {}%   baterai {:?} (state {})", s.temp_c, s.disk_pct, s.bat_pct, s.bat_state);
    println!("net     : rx {} KB/s  tx {} KB/s   uptime {} s", s.rx_kbs, s.tx_kbs, s.uptime_s);
    for p in &s.procs {
        println!("proses  : {:<16} {:.1}%", p.name, p.cpu);
    }
    if !cfg.city.is_empty() || cfg.latitude.is_some() {
        let place = match (cfg.latitude, cfg.longitude) {
            (Some(la), Some(lo)) => Some((la, lo, cfg.city.clone())),
            _ => weather::geocode(&cfg.city),
        };
        match place.and_then(|(la, lo, n)| weather::fetch(la, lo, &n)) {
            Some(w) => println!("cuaca   : {:?}", w),
            None => println!("cuaca   : gagal (cek internet / nama kota)"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first() {
        if matches!(first.as_str(), "next" | "prev" | "page" | "rotate" | "brightness") {
            std::process::exit(control::client(&args));
        }
    }
    let cfg = Config::load();
    if std::env::args().any(|a| a == "--dry-run") {
        dry_run(&cfg);
        return;
    }
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        println!("{}\nconfig: {}/config", control::USAGE, config::config_dir().display());
        return;
    }
    let weather = Arc::new(WeatherState::default());
    spawn_weather(&cfg, weather.clone());
    let slot: PortSlot = Arc::new(Mutex::new(None));
    let rt = Arc::new(Mutex::new(Runtime { brightness: cfg.brightness, rotation: cfg.rotation }));
    control::spawn_listener(slot.clone(), rt.clone());

    loop {
        if let Err(e) = run_session(&cfg, &weather, &slot, &rt) {
            eprintln!("[aksara-hud] sesi selesai: {e}");
        }
        thread::sleep(Duration::from_secs(2));
    }
}
