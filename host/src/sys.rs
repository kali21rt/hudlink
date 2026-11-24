//! System monitor: reads /proc and /sys directly (no crates, very cheap).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::proto::Buf;

const CLK_TCK: f64 = 100.0; // USER_HZ on x86_64 Linux
const PAGE_KB: u64 = 4;

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,   // % of one core
    pub mem_mb: u32, // resident memory
}

#[derive(Default)]
pub struct Snapshot {
    pub cpu: u8,
    pub cores: Vec<u8>,
    pub cpu_mhz: u16,
    pub ram_used_mb: u16,
    pub ram_total_mb: u16,
    pub swap_used_mb: u16,
    pub swap_total_mb: u16,
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u16>,
    pub gpu_mhz: Option<u16>,
    pub rx_kbs: u32,
    pub tx_kbs: u32,
    pub disk_r_kbs: u32,
    pub disk_w_kbs: u32,
    pub disk_pct: u8,
    pub bat_pct: Option<u8>,
    pub bat_state: u8, // 0 discharging, 1 charging, 2 full
    pub uptime_s: u32,
    pub load1: f32,
    pub nprocs: u16,
    pub procs: Vec<Proc>,
}

pub struct SysMon {
    prev_cpu: Vec<CpuTimes>,
    prev_net: Option<(u64, u64, Instant)>,
    prev_disk: Option<(u64, u64, Instant)>,
    prev_proc: HashMap<u32, u64>,
    prev_proc_at: Instant,
    temp_path: Option<PathBuf>,
    fan_path: Option<PathBuf>,
    gpu_path: Option<PathBuf>,
    bat_path: Option<PathBuf>,
    disk_pct: u8,
    disk_checked: Option<Instant>,
}

impl SysMon {
    pub fn new(temp_override: Option<&str>) -> Self {
        let temp_path = temp_override.map(PathBuf::from).or_else(find_temp_sensor);
        match &temp_path {
            Some(p) => eprintln!("[hudlink] temperature sensor: {}", p.display()),
            None => eprintln!("[hudlink] no temperature sensor found"),
        }
        let mut s = SysMon {
            prev_cpu: read_cpu_times(),
            prev_net: None,
            prev_disk: None,
            prev_proc: HashMap::new(),
            prev_proc_at: Instant::now(),
            temp_path,
            fan_path: find_fan(),
            gpu_path: find_gpu_freq(),
            bat_path: find_battery(),
            disk_pct: 0,
            disk_checked: None,
        };
        s.sample_procs(); // prime the process table
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
        snap.cpu_mhz = read_cpu_mhz();

        // Memory
        let mem = read_meminfo();
        let kb = |k: &str| *mem.get(k).unwrap_or(&0);
        let mb = |v: u64| (v / 1024).min(u16::MAX as u64) as u16;
        snap.ram_total_mb = mb(kb("MemTotal"));
        snap.ram_used_mb = mb(kb("MemTotal").saturating_sub(kb("MemAvailable")));
        snap.swap_total_mb = mb(kb("SwapTotal"));
        snap.swap_used_mb = mb(kb("SwapTotal").saturating_sub(kb("SwapFree")));

        // Sensors
        let read_num = |p: &Option<PathBuf>| p.as_ref().and_then(|p| read_trim(p)).and_then(|s| s.parse::<f64>().ok());
        snap.temp_c = read_num(&self.temp_path).map(|m| (m / 1000.0) as f32);
        snap.fan_rpm = read_num(&self.fan_path).map(|v| v.min(65_000.0) as u16);
        snap.gpu_mhz = self.gpu_path.as_ref().and_then(|p| read_gpu_mhz(p));

        // Network & disk throughput
        let now = Instant::now();
        if let Some((rx, tx)) = read_net_bytes() {
            if let Some((prx, ptx, at)) = self.prev_net {
                let dt = now.duration_since(at).as_secs_f64().max(0.001);
                snap.rx_kbs = (rx.saturating_sub(prx) as f64 / 1024.0 / dt) as u32;
                snap.tx_kbs = (tx.saturating_sub(ptx) as f64 / 1024.0 / dt) as u32;
            }
            self.prev_net = Some((rx, tx, now));
        }
        if let Some((r, w)) = read_disk_sectors() {
            if let Some((pr, pw, at)) = self.prev_disk {
                let dt = now.duration_since(at).as_secs_f64().max(0.001);
                snap.disk_r_kbs = (r.saturating_sub(pr) as f64 * 0.5 / dt) as u32; // 512-byte sectors
                snap.disk_w_kbs = (w.saturating_sub(pw) as f64 * 0.5 / dt) as u32;
            }
            self.prev_disk = Some((r, w, now));
        }
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

        // Uptime & load
        snap.uptime_s = read_trim(Path::new("/proc/uptime"))
            .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
            .unwrap_or(0.0) as u32;
        snap.load1 = read_trim(Path::new("/proc/loadavg"))
            .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
            .unwrap_or(0.0);

        let (procs, count) = self.sample_procs();
        snap.procs = procs;
        snap.nprocs = count.min(u16::MAX as usize) as u16;
        snap
    }

    /// Diff utime+stime of every process against the previous call.
    /// Returns the top 8 by CPU (then memory) and the total process count.
    fn sample_procs(&mut self) -> (Vec<Proc>, usize) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_proc_at).as_secs_f64().max(0.001);
        let mut current = HashMap::with_capacity(self.prev_proc.len() + 16);
        let mut list = Vec::new();
        if let Ok(dir) = fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
                let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else { continue };
                let Some(ps) = parse_proc_stat(&stat) else { continue };
                let cpu = self
                    .prev_proc
                    .get(&pid)
                    .map(|prev| (ps.ticks.saturating_sub(*prev) as f64 / CLK_TCK / dt * 100.0) as f32)
                    .unwrap_or(0.0);
                if ps.rss_pages > 0 {
                    list.push(Proc { pid, name: ps.name, cpu, mem_mb: (ps.rss_pages * PAGE_KB / 1024) as u32 });
                }
                current.insert(pid, ps.ticks);
            }
        }
        let count = current.len();
        self.prev_proc = current;
        self.prev_proc_at = now;
        list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.mem_mb.cmp(&a.mem_mb)));
        list.truncate(8);
        (list, count)
    }
}

pub fn payload(s: &Snapshot) -> Vec<u8> {
    let mut b = Buf::default();
    b.u8(s.cpu).u8(s.cores.len() as u8);
    for c in &s.cores {
        b.u8(*c);
    }
    b.u16(s.cpu_mhz);
    b.u16(s.ram_used_mb).u16(s.ram_total_mb).u16(s.swap_used_mb).u16(s.swap_total_mb);
    b.i16(s.temp_c.map_or(i16::MIN, |t| (t * 10.0).round() as i16));
    b.u16(s.fan_rpm.unwrap_or(u16::MAX)).u16(s.gpu_mhz.unwrap_or(0));
    b.u32(s.rx_kbs).u32(s.tx_kbs).u32(s.disk_r_kbs).u32(s.disk_w_kbs).u8(s.disk_pct);
    b.u8(s.bat_pct.unwrap_or(255)).u8(s.bat_state).u32(s.uptime_s);
    b.u16((s.load1 * 100.0).round().min(65_535.0) as u16).u16(s.nprocs);
    b.u8(s.procs.len() as u8);
    for p in &s.procs {
        b.u32(p.pid)
            .u16((p.cpu * 10.0).round().min(65_535.0) as u16)
            .u16(p.mem_mb.min(65_535) as u16)
            .str(&p.name, 15);
    }
    b.0
}

// ---------------- /proc and /sys parsers ----------------

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_cpu_times() -> Vec<CpuTimes> {
    let Ok(stat) = fs::read_to_string("/proc/stat") else { return Vec::new() };
    stat.lines()
        .take_while(|l| l.starts_with("cpu"))
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            let total = v.iter().take(8).sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            CpuTimes { total, idle }
        })
        .collect()
}

/// Average current frequency over all cores (cpufreq), MHz.
fn read_cpu_mhz() -> u16 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for i in 0..64 {
        let p = format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq");
        match read_trim(Path::new(&p)).and_then(|s| s.parse::<u64>().ok()) {
            Some(khz) => {
                sum += khz;
                n += 1;
            }
            None if i > 0 => break,
            None => {}
        }
    }
    if n == 0 { 0 } else { (sum / n / 1000).min(u16::MAX as u64) as u16 }
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

fn read_net_bytes() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/net/dev").ok()?;
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in text.lines().skip(2) {
        let (iface, rest) = line.split_once(':')?;
        let iface = iface.trim();
        if iface == "lo" || ["virbr", "veth", "podman", "docker", "br-"].iter().any(|p| iface.starts_with(p)) {
            continue;
        }
        let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        rx += v.first().copied().unwrap_or(0);
        tx += v.get(8).copied().unwrap_or(0);
    }
    Some((rx, tx))
}

fn is_whole_disk(name: &str) -> bool {
    let digits_after = |prefix: &str| name.strip_prefix(prefix).map_or(false, |r| !r.is_empty());
    (name.starts_with("nvme") && name.contains('n') && !name.contains('p'))
        || (name.starts_with("sd") && name.len() == 3)
        || (name.starts_with("vd") && name.len() == 3)
        || (digits_after("mmcblk") && !name.contains('p'))
}

/// Total sectors read / written on whole disks.
fn read_disk_sectors() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/diskstats").ok()?;
    let (mut r, mut w) = (0u64, 0u64);
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !is_whole_disk(f[2]) {
            continue;
        }
        r += f[5].parse::<u64>().unwrap_or(0);
        w += f[9].parse::<u64>().unwrap_or(0);
    }
    Some((r, w))
}

fn read_disk_pct(mount: &str) -> Option<u8> {
    let out = Command::new("df").args(["-P", mount]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().nth(1)?.split_whitespace().nth(4)?.trim_end_matches('%').parse().ok()
}

struct ProcStat {
    name: String,
    ticks: u64,
    rss_pages: u64,
}

/// `/proc/<pid>/stat` -> name, utime+stime, rss
fn parse_proc_stat(stat: &str) -> Option<ProcStat> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let f: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // after ")": [0]=state ... [11]=utime [12]=stime ... [21]=rss
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let rss: i64 = f.get(21)?.parse().ok()?;
    Some(ProcStat { name, ticks: utime + stime, rss_pages: rss.max(0) as u64 })
}

fn hwmons() -> Vec<PathBuf> {
    fs::read_dir("/sys/class/hwmon").map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

fn find_temp_sensor() -> Option<PathBuf> {
    let all = hwmons();
    for want in ["coretemp", "k10temp", "zenpower", "thinkpad", "acpitz"] {
        for h in &all {
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

fn find_fan() -> Option<PathBuf> {
    hwmons().into_iter().map(|h| h.join("fan1_input")).find(|p| p.exists())
}

/// Intel: gt_cur_freq_mhz. AMD: pp_dpm_sclk (line marked with '*').
fn find_gpu_freq() -> Option<PathBuf> {
    let cards: Vec<PathBuf> = fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map_or(false, |n| n.starts_with("card") && !n.contains('-')))
        .collect();
    for c in &cards {
        for p in [c.join("gt_cur_freq_mhz"), c.join("device/pp_dpm_sclk")] {
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn read_gpu_mhz(p: &Path) -> Option<u16> {
    let text = fs::read_to_string(p).ok()?;
    let line = if p.ends_with("pp_dpm_sclk") { text.lines().find(|l| l.contains('*'))? } else { text.trim() };
    let digits: String = line
        .split_whitespace()
        .find(|w| w.chars().next().map_or(false, |c| c.is_ascii_digit()) && !w.ends_with(':'))?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
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
        let s = "1234 (Web Content) S 1 2 3 4 5 6 7 8 9 10 150 50 0 0 20 0 30 0 100 2000000 5000 18446744073709551615";
        let p = parse_proc_stat(s).unwrap();
        assert_eq!((p.name.as_str(), p.ticks, p.rss_pages), ("Web Content", 200, 5000));
    }

    #[test]
    fn disk_names() {
        for n in ["nvme0n1", "sda", "vdb", "mmcblk0"] {
            assert!(is_whole_disk(n), "{n}");
        }
        for n in ["nvme0n1p2", "sda1", "mmcblk0p1", "loop0", "zram0", "dm-0"] {
            assert!(!is_whole_disk(n), "{n}");
        }
    }

    #[test]
    fn live_sample_works() {
        let mut m = SysMon::new(None);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = m.sample();
        assert!(s.ram_total_mb > 0);
        assert!(s.nprocs > 0);
        assert!(!payload(&s).is_empty());
    }
}
