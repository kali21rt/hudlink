//! System monitor: reads /proc and /sys directly (no crates, very cheap).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::proto::Buf;

const CLK_TCK: f64 = 100.0; // USER_HZ on x86_64 Linux

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

pub struct Proc {
    pub name: String,
    pub cpu: f32, // % of one core
}

#[derive(Default)]
pub struct Snapshot {
    pub cpu: u8,
    pub cores: Vec<u8>,
    pub ram_used_mb: u16,
    pub ram_total_mb: u16,
    pub swap_used_mb: u16,
    pub swap_total_mb: u16,
    pub temp_c: Option<f32>,
    pub rx_kbs: u32,
    pub tx_kbs: u32,
    pub disk_pct: u8,
    pub bat_pct: Option<u8>,
    pub bat_state: u8, // 0 discharging, 1 charging, 2 full
    pub uptime_s: u32,
    pub procs: Vec<Proc>,
}

pub struct SysMon {
    prev_cpu: Vec<CpuTimes>, // [0] = total, [1..] = cores
    prev_net: Option<(u64, u64, Instant)>,
    prev_proc: HashMap<u32, u64>,
    prev_proc_at: Instant,
    temp_path: Option<PathBuf>,
    bat_path: Option<PathBuf>,
    disk_pct: u8,
    disk_checked: Option<Instant>,
}

impl SysMon {
    pub fn new(temp_override: Option<&str>) -> Self {
        let temp_path = temp_override.map(PathBuf::from).or_else(find_temp_sensor);
        match &temp_path {
            Some(p) => eprintln!("[aksara-hud] sensor suhu: {}", p.display()),
            None => eprintln!("[aksara-hud] sensor suhu tidak ditemukan"),
        }
        let mut s = SysMon {
            prev_cpu: Vec::new(),
            prev_net: None,
            prev_proc: HashMap::new(),
            prev_proc_at: Instant::now(),
            temp_path,
            bat_path: find_battery(),
            disk_pct: 0,
            disk_checked: None,
        };
        s.prev_cpu = read_cpu_times();
        s.sample_procs(); // prime process table
        s
    }

    pub fn sample(&mut self) -> Snapshot {
        let mut snap = Snapshot::default();

        // CPU
        let now_cpu = read_cpu_times();
        let pct = |a: CpuTimes, b: CpuTimes| -> u8 {
            let dt = b.total.saturating_sub(a.total);
            let di = b.idle.saturating_sub(a.idle);
            if dt == 0 { 0 } else { (100 * (dt - di.min(dt)) / dt) as u8 }
        };
        if now_cpu.len() == self.prev_cpu.len() && !now_cpu.is_empty() {
            snap.cpu = pct(self.prev_cpu[0], now_cpu[0]);
            snap.cores = (1..now_cpu.len()).take(16).map(|i| pct(self.prev_cpu[i], now_cpu[i])).collect();
        }
        self.prev_cpu = now_cpu;

        // Memory
        let mem = read_meminfo();
        let kb = |k: &str| *mem.get(k).unwrap_or(&0);
        let mb = |v: u64| (v / 1024).min(u16::MAX as u64) as u16;
        snap.ram_total_mb = mb(kb("MemTotal"));
        snap.ram_used_mb = mb(kb("MemTotal").saturating_sub(kb("MemAvailable")));
        snap.swap_total_mb = mb(kb("SwapTotal"));
        snap.swap_used_mb = mb(kb("SwapTotal").saturating_sub(kb("SwapFree")));

        // Temperature
        snap.temp_c = self
            .temp_path
            .as_ref()
            .and_then(|p| read_trim(p))
            .and_then(|s| s.parse::<f32>().ok())
            .map(|milli| milli / 1000.0);

        // Network
        if let Some((rx, tx)) = read_net_bytes() {
            let now = Instant::now();
            if let Some((prx, ptx, at)) = self.prev_net {
                let dt = now.duration_since(at).as_secs_f64().max(0.001);
                snap.rx_kbs = (rx.saturating_sub(prx) as f64 / 1024.0 / dt) as u32;
                snap.tx_kbs = (tx.saturating_sub(ptx) as f64 / 1024.0 / dt) as u32;
            }
            self.prev_net = Some((rx, tx, now));
        }

        // Disk (df is cheap but no need to run it every second)
        if self.disk_checked.map_or(true, |t| t.elapsed().as_secs() >= 30) {
            self.disk_pct = read_disk_pct("/").unwrap_or(self.disk_pct);
            self.disk_checked = Some(Instant::now());
        }
        snap.disk_pct = self.disk_pct;

        // Battery
        if let Some(bat) = &self.bat_path {
            snap.bat_pct = read_trim(&bat.join("capacity")).and_then(|s| s.parse().ok());
            snap.bat_state = match read_trim(&bat.join("status")).as_deref() {
                Some("Charging") => 1,
                Some("Full") | Some("Not charging") => 2,
                _ => 0,
            };
        }

        // Uptime
        snap.uptime_s = read_trim(Path::new("/proc/uptime"))
            .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
            .unwrap_or(0.0) as u32;

        // Top processes
        snap.procs = self.sample_procs();
        snap
    }

    /// Diff utime+stime of every process against the previous call.
    fn sample_procs(&mut self) -> Vec<Proc> {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_proc_at).as_secs_f64().max(0.001);
        let mut current = HashMap::with_capacity(self.prev_proc.len() + 16);
        let mut list = Vec::new();
        if let Ok(dir) = fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
                let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else { continue };
                let Some((name, ticks)) = parse_proc_stat(&stat) else { continue };
                if let Some(prev) = self.prev_proc.get(&pid) {
                    let cpu = (ticks.saturating_sub(*prev) as f64 / CLK_TCK / dt * 100.0) as f32;
                    if cpu > 0.05 {
                        list.push(Proc { name, cpu });
                    }
                }
                current.insert(pid, ticks);
            }
        }
        self.prev_proc = current;
        self.prev_proc_at = now;
        list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        list.truncate(3);
        list
    }
}

pub fn payload(s: &Snapshot) -> Vec<u8> {
    let mut b = Buf::default();
    b.u8(s.cpu).u8(s.cores.len() as u8);
    for c in &s.cores {
        b.u8(*c);
    }
    b.u16(s.ram_used_mb).u16(s.ram_total_mb).u16(s.swap_used_mb).u16(s.swap_total_mb);
    b.i16(s.temp_c.map_or(i16::MIN, |t| (t * 10.0).round() as i16));
    b.u32(s.rx_kbs).u32(s.tx_kbs);
    b.u8(s.disk_pct).u8(s.bat_pct.unwrap_or(255)).u8(s.bat_state).u32(s.uptime_s);
    b.u8(s.procs.len() as u8);
    for p in &s.procs {
        b.u16((p.cpu * 10.0).round().min(u16::MAX as f32) as u16).str(&p.name, 15);
    }
    b.0
}

// ---------------- /proc parsers ----------------

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_cpu_times() -> Vec<CpuTimes> {
    let Ok(stat) = fs::read_to_string("/proc/stat") else { return Vec::new() };
    stat.lines()
        .take_while(|l| l.starts_with("cpu"))
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            // user nice system idle iowait irq softirq steal
            let total = v.iter().take(8).sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            CpuTimes { total, idle }
        })
        .collect()
}

fn read_meminfo() -> HashMap<String, u64> {
    fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.to_string(), v.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}

/// Sum of rx/tx bytes on all interfaces except loopback and virtual bridges.
fn read_net_bytes() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/net/dev").ok()?;
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in text.lines().skip(2) {
        let (iface, rest) = line.split_once(':')?;
        let iface = iface.trim();
        if iface == "lo" || iface.starts_with("virbr") || iface.starts_with("veth") || iface.starts_with("podman") {
            continue;
        }
        let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        rx += v.first().copied().unwrap_or(0);
        tx += v.get(8).copied().unwrap_or(0);
    }
    Some((rx, tx))
}

fn read_disk_pct(mount: &str) -> Option<u8> {
    let out = Command::new("df").args(["-P", mount]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    line.split_whitespace().nth(4)?.trim_end_matches('%').parse().ok()
}

/// `/proc/<pid>/stat` -> (comm, utime + stime)
fn parse_proc_stat(stat: &str) -> Option<(String, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // after ")": [0]=state ... [11]=utime [12]=stime
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

fn find_temp_sensor() -> Option<PathBuf> {
    let wanted = ["coretemp", "k10temp", "zenpower", "thinkpad", "acpitz"];
    let hwmons: Vec<PathBuf> = fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    for want in wanted {
        for h in &hwmons {
            if read_trim(&h.join("name")).as_deref() == Some(want) {
                let p = h.join("temp1_input");
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    let zone = PathBuf::from("/sys/class/thermal/thermal_zone0/temp");
    zone.exists().then_some(zone)
}

fn find_battery() -> Option<PathBuf> {
    fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| read_trim(&p.join("type")).as_deref() == Some("Battery"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_with_spaces_in_name() {
        let s = "1234 (Web Content) S 1 2 3 4 5 6 7 8 9 10 150 50 0 0 20 0";
        assert_eq!(parse_proc_stat(s), Some(("Web Content".into(), 200)));
    }

    #[test]
    fn live_sample_works() {
        let mut m = SysMon::new(None);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = m.sample();
        assert!(s.ram_total_mb > 0);
        assert!(!payload(&s).is_empty());
    }
}
