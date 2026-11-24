//! Linux readings: procfs, sysfs, hwmon and DMI. No root needed for anything shown by default; what needs root
//! (RAPL power, SMBIOS memory modules, SMART) is "—" with a reason. The parsers are pure functions with tests.
//!
//! Not run on a real Linux machine by the author (no Linux box, no Docker by rule): CI at release is the first
//! full run. Wayland is only covered by the toolkit.

use super::nvidia::Nvidia;
use super::Collector;
use crate::model::*;
use crate::specs::{self, Component, Field, Group, Specs};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn rd(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

// ───────────────────────── parsers ─────────────────────────

/// `/proc/stat` lines starting with `cpu`: (idle, total) per line; the first is the total.
pub fn parse_proc_stat(text: &str) -> Vec<(u64, u64)> {
    text.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            // idle + iowait; total = user..steal (guest is already inside user).
            (v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0), v.iter().take(8).sum())
        })
        .collect()
}

pub fn load_percent(prev: (u64, u64), cur: (u64, u64)) -> f32 {
    let dt = cur.1.saturating_sub(prev.1);
    if dt == 0 {
        return 0.0;
    }
    let idle = cur.0.saturating_sub(prev.0);
    (100.0 * (1.0 - idle as f64 / dt as f64)).clamp(0.0, 100.0) as f32
}

/// `/proc/meminfo` to (total, available, cached) in bytes.
pub fn parse_meminfo(text: &str) -> (u64, u64, u64) {
    let g = |k: &str| text.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 1024;
    (g("MemTotal:"), g("MemAvailable:"), g("Cached:"))
}

/// The CPU model name from `/proc/cpuinfo`.
pub fn parse_cpu_name(text: &str) -> Option<String> {
    text.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|s| s.trim().to_string())
}

/// One hwmon chip: its name and `(label, millidegrees)` pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub name: String,
    pub temps: Vec<(String, f32)>,
}

pub fn read_chip(dir: &Path) -> Option<Chip> {
    let name = rd(dir.join("name"))?;
    let mut temps = Vec::new();
    for i in 1..32 {
        let Some(v) = rd(dir.join(format!("temp{i}_input"))).and_then(|s| s.parse::<f32>().ok()) else { continue };
        let label = rd(dir.join(format!("temp{i}_label"))).unwrap_or_else(|| format!("temp{i}"));
        temps.push((label, v / 1000.0));
    }
    Some(Chip { name, temps })
}

/// CPU package and hottest-core temperatures from a `coretemp` (Intel) or `k10temp` (AMD) chip.
/// Intel: `Package id 0` and `Core N` labels. AMD: `Tctl` (the control temperature, with its offset on some parts),
/// `Tdie`, `Tccd1..`. The hotspot is the hottest core (or die/CCD).
pub fn cpu_temps(c: &Chip) -> (Option<f32>, Option<f32>) {
    match c.name.as_str() {
        "coretemp" => {
            let pkg = c.temps.iter().find(|(l, _)| l.starts_with("Package")).map(|x| x.1);
            let hot = c.temps.iter().filter(|(l, _)| l.starts_with("Core")).map(|x| x.1).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |x| x.max(v))));
            (pkg, hot.or(pkg))
        }
        "k10temp" | "zenpower" => {
            let pkg = c.temps.iter().find(|(l, _)| l == "Tdie").or_else(|| c.temps.iter().find(|(l, _)| l == "Tctl")).map(|x| x.1);
            let hot = c.temps.iter().filter(|(l, _)| l.starts_with("Tccd")).map(|x| x.1).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |x| x.max(v))));
            (pkg, hot.or(pkg))
        }
        _ => (None, None),
    }
}

fn hwmon_chips() -> Vec<Chip> {
    fs::read_dir("/sys/class/hwmon").map(|d| d.flatten().filter_map(|e| read_chip(&e.path())).collect()).unwrap_or_default()
}

// ───────────────────────── collector ─────────────────────────

pub struct LinuxCollector {
    last_stat: Vec<(u64, u64)>,
    last_net: Option<(String, u64, u64, Instant)>,
    last_disk: Vec<(String, u64, u64, u64, Instant)>,
    last_energy: Option<(u64, Instant)>,
    vendor: CpuVendor,
    nvidia: Option<Nvidia>,
}

fn cpu_vendor() -> CpuVendor {
    let t = rd("/proc/cpuinfo").unwrap_or_default();
    if t.contains("GenuineIntel") {
        CpuVendor::Intel
    } else if t.contains("AuthenticAMD") {
        CpuVendor::Amd
    } else {
        CpuVendor::Other
    }
}

impl LinuxCollector {
    pub fn new() -> Self {
        Self { last_stat: vec![], last_net: None, last_disk: vec![], last_energy: None, vendor: cpu_vendor(), nvidia: Nvidia::new() }
    }

    fn cpu(&mut self) -> CpuLive {
        let cur = parse_proc_stat(&rd("/proc/stat").unwrap_or_default());
        let mut load = 0.0;
        let mut per_core = Vec::new();
        if self.last_stat.len() == cur.len() && !cur.is_empty() {
            load = load_percent(self.last_stat[0], cur[0]);
            per_core = cur.iter().zip(&self.last_stat).skip(1).map(|(c, p)| load_percent(*p, *c)).collect();
        }
        self.last_stat = cur;
        let mhz = fs::read_dir("/sys/devices/system/cpu").ok().and_then(|d| {
            d.flatten().filter_map(|e| rd(e.path().join("cpufreq/scaling_cur_freq")).and_then(|s| s.parse::<f32>().ok())).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |x| x.max(v))))
        });
        let (t, h) = hwmon_chips().iter().map(cpu_temps).find(|(p, _)| p.is_some()).unwrap_or((None, None));
        // RAPL: `energy_uj` is root-only since CVE-2020-8694.
        let power = match rd("/sys/class/powercap/intel-rapl:0/energy_uj").and_then(|s| s.parse::<u64>().ok()) {
            Some(uj) => {
                let now = Instant::now();
                let w = self.last_energy.map(|(u0, t0)| rate_per_sec(u0, uj, now.duration_since(t0).as_secs_f64()) / 1e6);
                self.last_energy = Some((uj, now));
                w.map(|w| Ok(w as f32)).unwrap_or(Err(Why::HelperStarting))
            }
            None => Err(Why::NeedsRoot),
        };
        CpuLive {
            vendor: self.vendor,
            load,
            per_core,
            clock_mhz: mhz.map(|k| k / 1000.0),
            temp: Some(t.ok_or(Why::NotReported)),
            hotspot: Some(h.ok_or(Why::NotReported)),
            power: Some(power),
        }
    }

    fn net(&mut self) -> NetLive {
        let Ok(d) = fs::read_dir("/sys/class/net") else { return NetLive::default() };
        let mut best: Option<(String, u64, u64)> = None;
        for e in d.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // `address` (the MAC) is never read.
            if n == "lo" || rd(e.path().join("operstate")).as_deref() != Some("up") || ["veth", "docker", "br-", "virbr"].iter().any(|p| n.starts_with(p)) {
                continue;
            }
            let g = |f: &str| rd(e.path().join("statistics").join(f)).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let cand = (n, g("rx_bytes"), g("tx_bytes"));
            if best.as_ref().is_none_or(|b| cand.1 + cand.2 > b.1 + b.2) {
                best = Some(cand);
            }
        }
        let Some((n, rx, tx)) = best else { return NetLive::default() };
        let now = Instant::now();
        let (down, up) = match &self.last_net {
            Some((ln, lrx, ltx, t0)) if *ln == n => {
                let dt = now.duration_since(*t0).as_secs_f64();
                (rate_per_sec(*lrx, rx, dt), rate_per_sec(*ltx, tx, dt))
            }
            _ => (0.0, 0.0),
        };
        self.last_net = Some((n.clone(), rx, tx, now));
        NetLive { adapter: n, down, up }
    }

    fn disks(&mut self) -> Vec<DiskLive> {
        let text = rd("/proc/diskstats").unwrap_or_default();
        let now = Instant::now();
        let mut out = Vec::new();
        let mut next = Vec::new();
        for l in text.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            let Some(name) = f.get(2).map(|s| s.to_string()) else { continue };
            // Whole NVMe and SATA/SCSI disks only, not partitions.
            let whole = (name.starts_with("nvme") && !name.contains('p')) || (name.starts_with("sd") && name.len() == 3);
            if !whole {
                continue;
            }
            let (Some(r), Some(w), Some(io_ms)) = (f.get(5).and_then(|s| s.parse::<u64>().ok()), f.get(9).and_then(|s| s.parse::<u64>().ok()), f.get(12).and_then(|s| s.parse::<u64>().ok())) else { continue };
            let (rb, wb) = (r * 512, w * 512);
            let (read, write, busy) = match self.last_disk.iter().find(|d| d.0 == name) {
                Some((_, lr, lw, lio, t0)) => {
                    let dt = now.duration_since(*t0).as_secs_f64();
                    // Time spent doing I/O (ms) over the interval is the "active time" Task Manager shows.
                    (rate_per_sec(*lr, rb, dt), rate_per_sec(*lw, wb, dt), (rate_per_sec(*lio, io_ms, dt) / 10.0).clamp(0.0, 100.0) as f32)
                }
                None => (0.0, 0.0, 0.0),
            };
            let model = rd(format!("/sys/block/{name}/device/model")).unwrap_or_else(|| name.clone());
            // NVMe temperature through hwmon (no root).
            let temp = hwmon_for_block(&name).map(|c| c.temps.first().map(|t| t.1)).flatten();
            out.push(DiskLive { name: model, busy, read, write, temp: Some(temp.ok_or(Why::NotReported)), volumes: mounts_for(&name) });
            next.push((name, rb, wb, io_ms, now));
        }
        self.last_disk = next;
        out
    }
}

fn hwmon_for_block(name: &str) -> Option<Chip> {
    let dir = PathBuf::from(format!("/sys/block/{name}/device"));
    for e in fs::read_dir(&dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("hwmon") {
            return read_chip(&e.path());
        }
        if n.starts_with("nvme") {
            if let Ok(h) = fs::read_dir(e.path()) {
                for e2 in h.flatten() {
                    if e2.file_name().to_string_lossy().starts_with("hwmon") {
                        return read_chip(&e2.path());
                    }
                }
            }
        }
    }
    None
}

/// Mounted volumes of a disk by `/proc/mounts` and `statvfs`.
fn mounts_for(disk: &str) -> Vec<VolumeLive> {
    let text = rd("/proc/mounts").unwrap_or_default();
    let mut out = Vec::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        let (Some(dev), Some(mp), Some(fs_t)) = (f.first(), f.get(1), f.get(2)) else { continue };
        let base = dev.trim_start_matches("/dev/");
        let belongs = base.starts_with(disk) && base != disk || base == disk;
        if !belongs || !dev.starts_with("/dev/") {
            continue;
        }
        if let Some((total, free)) = statvfs(mp) {
            out.push(VolumeLive { name: mp.to_string(), fs: fs_t.to_string(), total, free });
        }
    }
    out
}

fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    // SAFETY: `s` is a zeroed statvfs the call fills in; `c` is NUL-terminated.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        Some((s.f_blocks as u64 * s.f_frsize as u64, s.f_bavail as u64 * s.f_frsize as u64))
    }
}

/// amdgpu through sysfs (design note in docs/SENSORS.md; no AMD hardware was available to test).
fn amdgpu_live() -> Option<GpuLive> {
    for e in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.starts_with("card") || n.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        let Some(load) = rd(dev.join("gpu_busy_percent")).and_then(|s| s.parse::<f32>().ok()) else { continue };
        let chip = fs::read_dir(dev.join("hwmon")).ok().and_then(|d| d.flatten().find_map(|h| read_chip(&h.path())));
        let temp = |label: &str| chip.as_ref().and_then(|c| c.temps.iter().find(|t| t.0 == label).map(|t| t.1));
        let hw = |f: &str| fs::read_dir(dev.join("hwmon")).ok().and_then(|d| d.flatten().find_map(|h| rd(h.path().join(f)))).and_then(|s| s.parse::<f32>().ok());
        let nr = || Err(Why::NotReported);
        return Some(GpuLive {
            name: rd(dev.join("product_name")).unwrap_or_else(|| "AMD GPU".into()),
            load,
            temp: Some(temp("edge").ok_or(Why::NotReported)),
            hotspot: Some(temp("junction").ok_or(Why::NotReported)),
            mem_temp: Some(temp("mem").ok_or(Why::NotReported)),
            vram_used: rd(dev.join("mem_info_vram_used")).and_then(|s| s.parse().ok()).unwrap_or(0),
            vram_total: rd(dev.join("mem_info_vram_total")).and_then(|s| s.parse().ok()).unwrap_or(0),
            clock_mhz: Some(hw("freq1_input").map(|hz| hz / 1e6).ok_or(Why::NotReported)),
            power: Some(hw("power1_average").map(|uw| uw / 1e6).ok_or(Why::NotReported)),
            power_limit: Some(hw("power1_cap").map(|uw| uw / 1e6).ok_or(Why::NotReported)),
            fan: Some(nr()),
        });
    }
    None
}

impl Collector for LinuxCollector {
    fn poll(&mut self) -> Live {
        let (total, avail, cached) = parse_meminfo(&rd("/proc/meminfo").unwrap_or_default());
        let gpu = self.nvidia.as_ref().and_then(|n| n.live()).or_else(amdgpu_live);
        Live {
            cpu: self.cpu(),
            gpu,
            ram: RamLive { total, used: total.saturating_sub(avail), available: avail, cached },
            net: self.net(),
            disks: self.disks(),
            cpu_access_missing: false,
            ready: true,
        }
    }
}

// ───────────────────────── specs ─────────────────────────

pub fn collect_specs() -> Specs {
    let mut comps = Vec::new();
    let cpuinfo = rd("/proc/cpuinfo").unwrap_or_default();
    let name = parse_cpu_name(&cpuinfo).unwrap_or_else(|| "CPU".into());
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    comps.push(Component::new(Group::Cpu, name.clone()).sub(format!("{threads} threads")).row(Field::Model, name).row(Field::Threads, threads.to_string()));
    match Nvidia::new() {
        Some(n) => comps.extend(n.specs()),
        None => {
            if let Some(g) = amdgpu_live() {
                comps.push(Component::new(Group::Gpu, g.name.clone()).row(Field::Model, g.name).row(Field::Vram, fmt_bytes(g.vram_total)));
            }
        }
    }
    // DMI without root; the serial and UUID files are root-only and are never read.
    let dmi = |f: &str| rd(format!("/sys/class/dmi/id/{f}")).unwrap_or_default();
    let (bv, bn) = (dmi("board_vendor"), dmi("board_name"));
    if !bn.is_empty() {
        comps.push(
            Component::new(Group::Board, format!("{bv} {bn}").trim().to_string())
                .row(Field::Manufacturer, bv)
                .row(Field::Model, bn)
                .row(Field::BiosVendor, dmi("bios_vendor"))
                .row(Field::BiosVersion, dmi("bios_version"))
                .row(Field::BiosDate, dmi("bios_date")),
        );
    }
    // Memory modules: SMBIOS type 17 (root only).
    match fs::read("/sys/firmware/dmi/tables/DMI") {
        Ok(t) => comps.extend(specs::parse_smbios_memory(&t)),
        Err(_) => {
            let (total, _, _) = parse_meminfo(&rd("/proc/meminfo").unwrap_or_default());
            comps.push(Component::new(Group::Ram, "System memory").sub(fmt_bytes(total)).row(Field::Capacity, fmt_bytes(total)).row(Field::PartNumber, specs::DASH).row(Field::Timings, specs::DASH));
        }
    }
    if let Ok(d) = fs::read_dir("/sys/block") {
        let mut names: Vec<String> = d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("nvme") || n.starts_with("sd")).collect();
        names.sort();
        for n in names {
            let model = rd(format!("/sys/block/{n}/device/model")).unwrap_or_else(|| n.clone());
            let sectors: u64 = rd(format!("/sys/block/{n}/size")).and_then(|s| s.parse().ok()).unwrap_or(0);
            let rot = rd(format!("/sys/block/{n}/queue/rotational")).map(|v| if v == "1" { "HDD" } else { "SSD" }).unwrap_or("");
            let temp = hwmon_for_block(&n).and_then(|c| c.temps.first().map(|t| format!("{:.0} °C", t.1))).unwrap_or_default();
            let vols: Vec<String> = mounts_for(&n).iter().map(|v| format!("{} {}, {} free of {}", v.name, v.fs, fmt_bytes(v.free), fmt_bytes(v.total))).collect();
            comps.push(
                Component::new(Group::Storage, model.clone())
                    .sub(fmt_capacity(sectors * 512))
                    .row(Field::Model, model)
                    .row(Field::Capacity, fmt_capacity(sectors * 512))
                    .row(Field::Bus, if n.starts_with("nvme") { "NVMe" } else { "SATA" })
                    .row(Field::Media, rot)
                    .row(Field::Temperature, temp)
                    .row(Field::Health, specs::DASH)
                    .row(Field::Volumes, vols.join("\n")),
            );
        }
    }
    if let Some(cards) = rd("/proc/asound/cards") {
        for l in cards.lines().filter(|l| l.contains(" - ")) {
            let dev = l.split(" - ").nth(1).unwrap_or("").trim().to_string();
            comps.push(Component::new(Group::Audio, dev.clone()).row(Field::Model, dev));
        }
    }
    if let Ok(d) = fs::read_dir("/sys/class/net") {
        for e in d.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == "lo" || rd(e.path().join("operstate")).as_deref() != Some("up") {
                continue;
            }
            let mbps = rd(e.path().join("speed")).and_then(|s| s.parse::<i64>().ok()).filter(|s| *s > 0);
            let speed = mbps.map(|m| format!("{m} Mbps")).unwrap_or_default();
            comps.push(Component::new(Group::Network, n.clone()).sub(speed.clone()).row(Field::Model, n).row(Field::LinkSpeed, speed));
        }
    }
    Specs { comps }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\nintr 12345\n";

    #[test]
    fn proc_stat_gives_idle_and_total() {
        let v = parse_proc_stat(STAT);
        assert_eq!(v, vec![(850, 1000), (425, 500)]);
        // 150 more busy ticks out of 200: 75 %.
        assert_eq!(load_percent((850, 1000), (900, 1200)), 75.0);
        assert_eq!(load_percent((1, 1), (1, 1)), 0.0);
    }

    #[test]
    fn meminfo_is_in_bytes() {
        let (t, a, c) = parse_meminfo("MemTotal:       32768 kB\nMemFree: 1 kB\nMemAvailable:   16384 kB\nCached:          2048 kB\n");
        assert_eq!((t, a, c), (32768 * 1024, 16384 * 1024, 2048 * 1024));
        assert_eq!(parse_meminfo(""), (0, 0, 0));
    }

    #[test]
    fn coretemp_and_k10temp_are_mapped() {
        let intel = Chip { name: "coretemp".into(), temps: vec![("Package id 0".into(), 55.0), ("Core 0".into(), 48.0), ("Core 1".into(), 52.0)] };
        assert_eq!(cpu_temps(&intel), (Some(55.0), Some(52.0)));
        let amd = Chip { name: "k10temp".into(), temps: vec![("Tctl".into(), 61.0), ("Tccd1".into(), 58.0)] };
        assert_eq!(cpu_temps(&amd), (Some(61.0), Some(58.0)));
        let nvme = Chip { name: "nvme".into(), temps: vec![("Composite".into(), 40.0)] };
        assert_eq!(cpu_temps(&nvme), (None, None));
    }

    #[test]
    fn cpu_name_from_cpuinfo() {
        assert_eq!(parse_cpu_name("processor\t: 0\nmodel name\t: AMD Ryzen 7 5800X3D 8-Core Processor\n").as_deref(), Some("AMD Ryzen 7 5800X3D 8-Core Processor"));
        assert_eq!(parse_cpu_name("nothing"), None);
    }
}
