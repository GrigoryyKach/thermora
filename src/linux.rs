//! Linux readings: sysfs, procfs and DMI only. No root needed for anything printed here;
//! what needs root is reported as "not available". NOT run in T1 (no Linux machine; CI at release).

use crate::line;
use std::fs;
use std::path::Path;

fn rd(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub struct Probe {
    last_stat: Vec<(u64, u64)>, // (idle, total) per cpu line
    last_net: Vec<(String, u64, u64)>,
    last_disk: Vec<(String, u64, u64)>,
}

impl Probe {
    pub fn new(_module_dir: Option<String>) -> Probe {
        Probe { last_stat: vec![], last_net: vec![], last_disk: vec![] }
    }

    pub fn process_cost(&self) -> (f64, f64, f64) {
        // /proc/self/stat utime+stime (fields 14,15) in clock ticks (100 Hz on every mainstream kernel build).
        let cpu = rd("/proc/self/stat")
            .and_then(|s| {
                let rest = s.rsplit(')').next()?.split_whitespace().map(str::to_string).collect::<Vec<_>>();
                Some((rest.get(11)?.parse::<f64>().ok()? + rest.get(12)?.parse::<f64>().ok()?) / 100.0)
            })
            .unwrap_or(0.0);
        let kb = |key: &str| rd("/proc/self/status").and_then(|s| s.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())).unwrap_or(0.0) / 1024.0;
        (cpu, kb("VmRSS:"), kb("VmHWM:"))
    }

    pub fn specs(&mut self) {
        // CPU
        if let Some(c) = rd("/proc/cpuinfo") {
            if let Some(m) = c.lines().find(|l| l.starts_with("model name")) {
                line("CPU", "name", Ok(m.split(':').nth(1).unwrap_or("").trim().to_string()));
            }
            let n = c.lines().filter(|l| l.starts_with("processor")).count();
            line("CPU", "logical CPUs", Ok(n.to_string()));
        }
        // DMI without root: serials/UUID files are root-only and are never read.
        for (k, f) in [("board vendor", "board_vendor"), ("board model", "board_name"), ("BIOS vendor", "bios_vendor"), ("BIOS version", "bios_version"), ("BIOS date", "bios_date")] {
            line("Board", k, rd(format!("/sys/class/dmi/id/{f}")).ok_or("no DMI".into()));
        }
        line("RAM", "module part numbers / timings", Err("needs root (dmidecode -t 17 / SMBIOS table); timings need SPD".into()));
        // Storage
        if let Ok(rd_dir) = fs::read_dir("/sys/block") {
            for e in rd_dir.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("loop") || n.starts_with("ram") || n.starts_with("zram") {
                    continue;
                }
                let model = rd(e.path().join("device/model")).unwrap_or_default(); // device/serial never read
                let sectors: u64 = rd(e.path().join("size")).and_then(|s| s.parse().ok()).unwrap_or(0);
                let rot = rd(e.path().join("queue/rotational")).map(|v| if v == "1" { "HDD" } else { "SSD/NVMe" }).unwrap_or("?");
                line("Storage", &n, Ok(format!("{model}; {} GB; {rot}", sectors * 512 / 1_000_000_000)));
            }
        }
        // Network: name, speed, state. address/ file (MAC) is never read.
        if let Ok(d) = fs::read_dir("/sys/class/net") {
            for e in d.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n == "lo" {
                    continue;
                }
                line("Net", &n, Ok(format!("{}; speed {} Mbps", rd(e.path().join("operstate")).unwrap_or_default(), rd(e.path().join("speed")).unwrap_or("?".into()))));
            }
        }
    }

    pub fn sample(&mut self) {
        // CPU load from /proc/stat
        if let Some(s) = rd("/proc/stat") {
            let cur: Vec<(u64, u64)> = s
                .lines()
                .filter(|l| l.starts_with("cpu"))
                .map(|l| {
                    let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
                    (v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0), v.iter().sum())
                })
                .collect();
            if self.last_stat.len() == cur.len() {
                let pct: Vec<String> = cur.iter().zip(&self.last_stat).map(|(c, l)| format!("{:.0}", 100.0 * (1.0 - (c.0 - l.0) as f64 / (c.1 - l.1).max(1) as f64))).collect();
                line("CPU", "load total / per logical", Ok(pct.join(" ")));
            }
            self.last_stat = cur;
        }
        // clock
        if let Ok(d) = fs::read_dir("/sys/devices/system/cpu") {
            let mut m = 0u64;
            for e in d.flatten() {
                if let Some(k) = rd(e.path().join("cpufreq/scaling_cur_freq")).and_then(|s| s.parse::<u64>().ok()) {
                    m = m.max(k);
                }
            }
            line("CPU", "clock (fastest core)", if m > 0 { Ok(format!("{} MHz", m / 1000)) } else { Err("no cpufreq".into()) });
        }
        // RAM
        if let Some(m) = rd("/proc/meminfo") {
            let g = |k: &str| m.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()).unwrap_or(0.0) / 1048576.0;
            line("RAM", "used / total / available / cached", Ok(format!("{:.1} / {:.1} / {:.1} / {:.1} GB", g("MemTotal:") - g("MemAvailable:"), g("MemTotal:"), g("MemAvailable:"), g("Cached:"))));
        }
        // hwmon temperatures: coretemp, k10temp, nvme, amdgpu
        if let Ok(d) = fs::read_dir("/sys/class/hwmon") {
            for e in d.flatten() {
                let chip = rd(e.path().join("name")).unwrap_or_default();
                if !["coretemp", "k10temp", "nvme", "amdgpu", "zenpower"].contains(&chip.as_str()) {
                    continue;
                }
                for i in 1..40 {
                    let Some(v) = rd(e.path().join(format!("temp{i}_input"))).and_then(|s| s.parse::<f64>().ok()) else { continue };
                    let label = rd(e.path().join(format!("temp{i}_label"))).unwrap_or(format!("temp{i}"));
                    line("Temp", &format!("{chip} / {label}"), Ok(format!("{:.1} C", v / 1000.0)));
                }
            }
        }
        // Network & disk rates
        if let Ok(d) = fs::read_dir("/sys/class/net") {
            let mut cur = vec![];
            for e in d.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let g = |f: &str| rd(e.path().join("statistics").join(f)).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                cur.push((n, g("rx_bytes"), g("tx_bytes")));
            }
            for c in &cur {
                if let Some(l) = self.last_net.iter().find(|l| l.0 == c.0) {
                    if c.0 != "lo" {
                        line("Net", &c.0, Ok(format!("down {:.1} KB/s up {:.1} KB/s", (c.1 - l.1) as f64 / 1e3, (c.2 - l.2) as f64 / 1e3)));
                    }
                }
            }
            self.last_net = cur;
        }
        if let Some(s) = rd("/proc/diskstats") {
            let cur: Vec<(String, u64, u64)> = s
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    let name = f.get(2)?.to_string();
                    if !(name.starts_with("nvme") && !name.contains('p') || name.starts_with("sd") && name.len() == 3) {
                        return None;
                    }
                    Some((name, f.get(5)?.parse::<u64>().ok()? * 512, f.get(9)?.parse::<u64>().ok()? * 512))
                })
                .collect();
            for c in &cur {
                if let Some(l) = self.last_disk.iter().find(|l| l.0 == c.0) {
                    line("Storage", &c.0, Ok(format!("read {:.1} MB/s write {:.1} MB/s", (c.1 - l.1) as f64 / 1e6, (c.2 - l.2) as f64 / 1e6)));
                }
            }
            self.last_disk = cur;
        }
        line("CPU", "package power (RAPL)", match rd("/sys/class/powercap/intel-rapl:0/energy_uj") {
            Some(_) => Ok("readable (delta of energy_uj / dt)".into()),
            None => Err("energy_uj is root-only since CVE-2020-8694".into()),
        });
    }
}
