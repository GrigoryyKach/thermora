//! `--demo`: plausible fake readings and a fake hardware list for screenshots. Nothing here touches the machine.

use super::Collector;
use crate::model::*;
use crate::specs::*;

pub struct DemoCollector {
    t: f32,
}

impl DemoCollector {
    pub fn new() -> Self {
        Self { t: 0.0 }
    }
}

fn wave(t: f32, period: f32, lo: f32, hi: f32, phase: f32) -> f32 {
    let s = ((t / period + phase) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    lo + (hi - lo) * s
}

impl Collector for DemoCollector {
    fn poll(&mut self) -> Live {
        let t = self.t;
        self.t += 1.0;
        let load = (wave(t, 23.0, 14.0, 71.0, 0.0) + wave(t, 5.0, 0.0, 9.0, 0.3)).clamp(0.0, 100.0);
        let per_core: Vec<f32> = (0..12).map(|i| wave(t, 11.0 + i as f32, 4.0, 96.0, i as f32 * 0.17).clamp(0.0, 100.0)).collect();
        let cpu_t = 41.0 + load * 0.34;
        let gpu_load = wave(t, 31.0, 6.0, 64.0, 0.4);
        // THERMORA_DEMO_NOACCESS=1 (test folder only): the state before the CPU temperature access is enabled.
        let no_access = crate::config::is_test_instance() && std::env::var("THERMORA_DEMO_NOACCESS").is_ok();
        let missing: Option<Rd<f32>> = Some(Err(Why::NeedsCpuAccess));
        let mut live = Live {
            cpu: CpuLive {
                vendor: CpuVendor::Intel,
                load,
                per_core,
                clock_mhz: Some(3600.0 + load * 8.0),
                temp: if no_access { missing } else { Some(Ok(cpu_t)) },
                hotspot: if no_access { missing } else { Some(Ok(cpu_t + 5.0)) },
                power: if no_access { missing } else { Some(Ok(18.0 + load * 0.62)) },
            },
            gpu: Some(GpuLive {
                name: "GeForce RTX 4060".into(),
                load: gpu_load,
                temp: Some(Ok(38.0 + gpu_load * 0.36)),
                hotspot: Some(Ok(47.0 + gpu_load * 0.4)),
                mem_temp: Some(Err(Why::NotReported)),
                vram_used: (1.9e9 + gpu_load as f64 * 3.1e7) as u64,
                vram_total: 8 * 1024 * 1024 * 1024,
                clock_mhz: Some(Ok(210.0 + gpu_load * 24.0)),
                power: Some(Ok(11.0 + gpu_load * 0.95)),
                power_limit: Some(Ok(115.0)),
                fan: Some(Ok(if gpu_load > 25.0 { 38.0 + gpu_load * 0.4 } else { 0.0 })),
            }),
            ram: RamLive { total: 32 * 1024 * 1024 * 1024, used: (14.2e9 + wave(t, 40.0, 0.0, 1.2e9, 0.0) as f64) as u64, available: 18_000_000_000, cached: 6_400_000_000 },
            net: NetLive { adapter: "Ethernet".into(), down: wave(t, 17.0, 0.2e6, 14.0e6, 0.0) as f64, up: wave(t, 13.0, 0.03e6, 1.4e6, 0.5) as f64 },
            disks: vec![
                DiskLive {
                    name: "Samsung SSD 980 PRO 1TB".into(),
                    busy: wave(t, 9.0, 0.0, 22.0, 0.0),
                    read: wave(t, 9.0, 0.0, 180e6, 0.1) as f64,
                    write: wave(t, 7.0, 0.0, 60e6, 0.6) as f64,
                    temp: Some(Ok(41.0 + wave(t, 30.0, 0.0, 6.0, 0.0))),
                    volumes: vec![VolumeLive { name: "C:".into(), fs: "NTFS".into(), total: 931 * 1024 * 1024 * 1024, free: 412 * 1024 * 1024 * 1024 }],
                },
                DiskLive {
                    name: "WD Blue SN580 2TB".into(),
                    busy: 1.0,
                    read: 0.0,
                    write: 0.2e6,
                    temp: Some(Ok(33.0)),
                    volumes: vec![VolumeLive { name: "D:".into(), fs: "NTFS".into(), total: 1863 * 1024 * 1024 * 1024, free: 1210 * 1024 * 1024 * 1024 }],
                },
            ],
            cpu_access_missing: no_access,
            ready: true,
        };
        if crate::config::is_test_instance() && std::env::var("THERMORA_DEMO_EXTREME").is_ok() {
            extremes(&mut live);
        }
        live
    }
}

/// THERMORA_DEMO_EXTREME=1 (test folder only): the longest numbers and names, to check that no card changes width.
fn extremes(l: &mut Live) {
    l.cpu.clock_mhz = Some(9990.0);
    l.cpu.temp = Some(Ok(100.0));
    l.cpu.hotspot = Some(Ok(100.0));
    l.cpu.power = Some(Ok(999.9));
    if let Some(g) = &mut l.gpu {
        g.name = "NVIDIA GeForce RTX 4090 Founders Edition Collector".into();
        g.temp = Some(Ok(100.0));
        g.hotspot = Some(Ok(100.0));
        g.mem_temp = Some(Ok(100.0));
        g.clock_mhz = Some(Ok(9990.0));
        g.power = Some(Ok(999.0));
        g.power_limit = Some(Ok(999.0));
        g.fan = Some(Ok(100.0));
        g.vram_used = 99 * 1024 * 1024 * 1024;
        g.vram_total = 99 * 1024 * 1024 * 1024;
    }
    l.ram = RamLive { total: 999 * 1024 * 1024 * 1024, used: 999 * 1024 * 1024 * 1024, available: 999 * 1024 * 1024 * 1024, cached: 999 * 1024 * 1024 * 1024 };
    l.net = NetLive { adapter: "Intel(R) Ethernet Controller I225-V Extremely Long Adapter Name Number 12345".into(), down: 999.9e6, up: 10.0e9 };
    for (i, d) in l.disks.iter_mut().enumerate() {
        d.name = format!("Very Long Manufacturer Name Enterprise NVMe PCIe Gen5 x4 SSD 16 TB Edition {i}");
        d.busy = 100.0;
        d.read = if i == 0 { 65.5e3 } else { 999.9e6 };
        d.write = if i == 0 { 10.1e6 } else { 10.0e9 };
        d.temp = Some(Ok(100.0));
        d.volumes = ["C:", "D:", "E:", "F:"].iter().map(|n| VolumeLive { name: n.to_string(), fs: "NTFS".into(), total: 999 * 1024 * 1024 * 1024, free: 999 * 1024 * 1024 * 1024 }).collect();
    }
}

/// A fake PC: nothing here is real hardware of the person running the demo.
pub fn demo_specs() -> Specs {
    let g = |s: &str| s.to_string();
    Specs {
        comps: vec![
            Component::new(Group::Cpu, "Intel Core i5-12400F")
                .sub("6 cores, 12 threads")
                .row(Field::Manufacturer, "Intel")
                .row(Field::Codename, "Alder Lake-S")
                .row(Field::Socket, "LGA1700")
                .row(Field::Cores, "6")
                .row(Field::Threads, "12")
                .row(Field::BaseClock, "2.50 GHz")
                .row(Field::L1, "6 x 32 KB + 6 x 48 KB")
                .row(Field::L2, "6 x 1.25 MB")
                .row(Field::L3, "18 MB")
                .row(Field::Tdp, "65 W"),
            Component::new(Group::Gpu, "NVIDIA GeForce RTX 4060")
                .sub("8 GB")
                .row(Field::Manufacturer, "NVIDIA")
                .row(Field::Model, "GeForce RTX 4060")
                .row(Field::Vram, "8.0 GB")
                .row(Field::Driver, "566.36")
                .row(Field::MaxCoreClock, "3105 MHz")
                .row(Field::MaxMemClock, "8501 MHz")
                .row(Field::PowerLimit, "115 W")
                .row(Field::PcieLink, "Gen4 x8 (max Gen4 x8)"),
            Component::new(Group::Board, "ASUS PRIME B660-PLUS D4")
                .sub("BIOS 3210")
                .row(Field::Manufacturer, "ASUSTeK COMPUTER INC.")
                .row(Field::Model, "PRIME B660-PLUS D4")
                .row(Field::Chipset, "Intel B660")
                .row(Field::BiosVendor, "American Megatrends Inc.")
                .row(Field::BiosVersion, "3210")
                .row(Field::BiosDate, "2024-04-19"),
            memory_module(&[g("Kingston"), g("KF3600C18D4/16GX"), g("17179869184"), g("3600"), g("3600"), g("26"), g("DIMM_A1"), g("1200")]),
            memory_module(&[g("Kingston"), g("KF3600C18D4/16GX"), g("17179869184"), g("3600"), g("3600"), g("26"), g("DIMM_B1"), g("1200")]),
            Component::new(Group::Storage, "Samsung SSD 980 PRO 1TB")
                .sub("NVMe, 1 TB")
                .row(Field::Capacity, "1 TB")
                .row(Field::Bus, "NVMe")
                .row(Field::Media, "SSD")
                .row(Field::Health, "OK, 2% used")
                .row(Field::Temperature, "41 °C")
                .row(Field::Written, "19.1 TB")
                .row(Field::PowerOn, "7070 h")
                .row(Field::Volumes, "C: NTFS, 412 GB free of 931 GB"),
            Component::new(Group::Storage, "WD Blue SN580 2TB")
                .sub("NVMe, 2 TB")
                .row(Field::Capacity, "2 TB")
                .row(Field::Bus, "NVMe")
                .row(Field::Media, "SSD")
                .row(Field::Health, "OK, 1% used")
                .row(Field::Temperature, "33 °C")
                .row(Field::Written, "6.4 TB")
                .row(Field::PowerOn, "2210 h")
                .row(Field::Volumes, "D: NTFS, 1.18 TB free of 1.82 TB"),
            Component::new(Group::Audio, "Studio Microphone USB").sub("Microphone, Speakers").row(Field::Model, "Studio Microphone USB").row(Field::Status, "Microphone, Speakers"),
            Component::new(Group::Audio, "NVIDIA High Definition Audio").sub("Digital output").row(Field::Model, "NVIDIA High Definition Audio").row(Field::Status, "Digital output"),
            Component::new(Group::Network, "Realtek Gaming 2.5GbE Family Controller").sub("1 Gbps").row(Field::Model, "Realtek Gaming 2.5GbE Family Controller").row(Field::LinkSpeed, "1 Gbps").row(Field::Address, "192.0.2.10"),
        ],
    }
}
