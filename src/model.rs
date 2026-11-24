//! Plain data the sensors produce and the window draws: live readings, why a reading is missing, units,
//! temperature bands and the sparkline ring buffer. No OS calls in this file, so all of it is unit-tested.

use std::collections::VecDeque;

/// Why a reading is "—". Each reason has its own tooltip, so the user knows what to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// CPU temperature and power need the one-time CPU temperature access.
    NeedsCpuAccess,
    /// The helper is set up but has not answered yet (starting) or reports a problem.
    HelperStarting,
    /// The helper cannot read this CPU (not Intel, or the driver module is missing).
    HelperUnsupported,
    /// The device or driver does not report it (a GPU without a memory sensor, no fan, and so on).
    NotReported,
    /// Needs a privilege we do not have (Linux: RAPL and SMART are root only).
    #[cfg_attr(windows, allow(dead_code))]
    NeedsRoot,
}

pub type Rd<T> = Result<T, Why>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CpuVendor {
    #[default]
    Other,
    Intel,
    Amd,
}

#[derive(Clone, Debug, Default)]
pub struct CpuLive {
    pub vendor: CpuVendor,
    pub load: f32,
    pub per_core: Vec<f32>,
    pub clock_mhz: Option<f32>,
    pub temp: Option<Rd<f32>>,
    pub hotspot: Option<Rd<f32>>,
    pub power: Option<Rd<f32>>,
}

#[derive(Clone, Debug, Default)]
pub struct GpuLive {
    pub name: String,
    pub load: f32,
    pub temp: Option<Rd<f32>>,
    pub hotspot: Option<Rd<f32>>,
    pub mem_temp: Option<Rd<f32>>,
    pub vram_used: u64,
    pub vram_total: u64,
    pub clock_mhz: Option<Rd<f32>>,
    pub power: Option<Rd<f32>>,
    pub power_limit: Option<Rd<f32>>,
    pub fan: Option<Rd<f32>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RamLive {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub cached: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NetLive {
    pub adapter: String,
    pub down: f64,
    pub up: f64,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeLive {
    pub name: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DiskLive {
    pub name: String,
    pub busy: f32,
    pub read: f64,
    pub write: f64,
    pub temp: Option<Rd<f32>>,
    pub volumes: Vec<VolumeLive>,
}

/// Everything the Monitor page shows, once a second.
#[derive(Clone, Debug, Default)]
pub struct Live {
    pub cpu: CpuLive,
    pub gpu: Option<GpuLive>,
    pub ram: RamLive,
    pub net: NetLive,
    pub disks: Vec<DiskLive>,
    /// CPU temperature access: `true` when the helper is needed on this system and not set up.
    pub cpu_access_missing: bool,
    /// The first reading has arrived.
    pub ready: bool,
}

// ───────────────────────── units ─────────────────────────

/// Bytes per second to a value and a unit with sensible digits: `1.2 MB/s`, `340 KB/s`, `0 B/s`.
pub fn fmt_rate(bytes_per_s: f64) -> String {
    let b = bytes_per_s.max(0.0);
    const U: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = b;
    let mut i = 0;
    while v >= 1000.0 && i < 3 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{:.0} {}", v, U[i])
    } else if v >= 100.0 {
        format!("{:.0} {}", v, U[i])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, U[i])
    } else {
        format!("{:.2} {}", v, U[i])
    }
}

/// Binary sizes as Windows shows them (GB means GiB): `31.9 GB`, `512 MB`, `1.8 TB`.
pub fn fmt_bytes(b: u64) -> String {
    let g = 1024.0f64 * 1024.0 * 1024.0;
    let v = b as f64;
    if v >= g * 1024.0 {
        format!("{:.2} TB", v / (g * 1024.0))
    } else if v >= g {
        format!("{:.1} GB", v / g)
    } else if v >= 1024.0 * 1024.0 {
        format!("{:.0} MB", v / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", v / 1024.0)
    }
}

/// Drive sizes as the box says (decimal): `1 TB`, `2 TB`, `512 GB`.
pub fn fmt_capacity(b: u64) -> String {
    let v = b as f64;
    if v >= 0.95e12 {
        format!("{} TB", (v / 1e12 * 10.0).round() / 10.0)
    } else {
        format!("{} GB", (v / 1e9).round())
    }
}

pub fn fmt_mhz(mhz: f32) -> String {
    if mhz >= 1000.0 {
        format!("{:.2} GHz", mhz / 1000.0)
    } else {
        format!("{:.0} MHz", mhz)
    }
}

/// The rate over an interval from two counter readings. A counter that went back (wrap or reset) gives 0, not a spike.
#[cfg_attr(windows, allow(dead_code))]
pub fn rate_per_sec(prev: u64, cur: u64, dt_secs: f64) -> f64 {
    if dt_secs <= 0.0 || cur < prev {
        return 0.0;
    }
    (cur - prev) as f64 / dt_secs
}

// ───────────────────────── temperature bands ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Normal,
    Warm,
    Hot,
}

/// Thresholds in degrees C: (warm from, hot from). These are comfort bands, not the chip limits.
///
/// - Intel Core: throttling starts at TjMax (100 C for current desktop parts); sustained 70+ is warm, 85+ is hot.
/// - AMD Ryzen: Tctl runs hotter by design (limit 90-95 C); warm from 75, hot from 90.
/// - NVIDIA/AMD GPU core: the driver slows down near 83-95 C; warm from 70, hot from 83.
/// - NVMe: controllers throttle around 70-80 C (composite warning is usually 75); warm from 55, hot from 70.
pub fn thresholds(kind: SensorKind) -> (f32, f32) {
    match kind {
        SensorKind::CpuIntel | SensorKind::CpuOther => (70.0, 85.0),
        SensorKind::CpuAmd => (75.0, 90.0),
        SensorKind::Gpu => (70.0, 83.0),
        SensorKind::Nvme => (55.0, 70.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    CpuIntel,
    CpuAmd,
    CpuOther,
    Gpu,
    Nvme,
}

impl From<CpuVendor> for SensorKind {
    fn from(v: CpuVendor) -> Self {
        match v {
            CpuVendor::Intel => SensorKind::CpuIntel,
            CpuVendor::Amd => SensorKind::CpuAmd,
            CpuVendor::Other => SensorKind::CpuOther,
        }
    }
}

pub fn band(kind: SensorKind, temp_c: f32) -> Band {
    let (warm, hot) = thresholds(kind);
    if temp_c >= hot {
        Band::Hot
    } else if temp_c >= warm {
        Band::Warm
    } else {
        Band::Normal
    }
}

// ───────────────────────── sparkline buffer ─────────────────────────

/// A fixed-length history (60 points = 60 s at 1 s).
#[derive(Clone, Debug)]
pub struct Ring {
    cap: usize,
    v: VecDeque<f32>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), v: VecDeque::with_capacity(cap.max(1)) }
    }
    pub fn push(&mut self, x: f32) {
        if self.v.len() == self.cap {
            self.v.pop_front();
        }
        self.v.push_back(if x.is_finite() { x } else { 0.0 });
    }
    pub fn len(&self) -> usize {
        self.v.len()
    }
    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        self.v.iter().copied()
    }
    pub fn max(&self) -> f32 {
        self.v.iter().copied().fold(0.0, f32::max)
    }
}

/// Histories the Monitor cards draw.
#[derive(Clone)]
pub struct History {
    pub cpu_load: Ring,
    pub cpu_temp: Ring,
    pub gpu_load: Ring,
    pub gpu_temp: Ring,
    pub ram: Ring,
    pub down: Ring,
    pub up: Ring,
}

impl History {
    pub const POINTS: usize = 60;
    pub fn new() -> Self {
        let r = || Ring::new(Self::POINTS);
        Self { cpu_load: r(), cpu_temp: r(), gpu_load: r(), gpu_temp: r(), ram: r(), down: r(), up: r() }
    }

    pub fn push(&mut self, l: &Live) {
        self.cpu_load.push(l.cpu.load);
        if let Some(Ok(t)) = l.cpu.temp {
            self.cpu_temp.push(t);
        }
        if let Some(g) = &l.gpu {
            self.gpu_load.push(g.load);
            if let Some(Ok(t)) = g.temp {
                self.gpu_temp.push(t);
            }
        }
        self.ram.push(if l.ram.total > 0 { l.ram.used as f32 / l.ram.total as f32 * 100.0 } else { 0.0 });
        self.down.push(l.net.down as f32);
        self.up.push(l.net.up as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_use_sensible_units() {
        assert_eq!(fmt_rate(0.0), "0 B/s");
        assert_eq!(fmt_rate(512.0), "512 B/s");
        assert_eq!(fmt_rate(1_500.0), "1.50 KB/s");
        assert_eq!(fmt_rate(34_000.0), "34.0 KB/s");
        assert_eq!(fmt_rate(120_000_000.0), "120 MB/s");
        assert_eq!(fmt_rate(2.5e9), "2.50 GB/s");
        assert_eq!(fmt_rate(-5.0), "0 B/s");
    }

    #[test]
    fn sizes_and_capacities() {
        assert_eq!(fmt_bytes(34 * 1024 * 1024 * 1024), "34.0 GB");
        assert_eq!(fmt_bytes(2 * 1024u64.pow(4)), "2.00 TB");
        assert_eq!(fmt_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(fmt_capacity(1_024_209_543_168), "1 TB");
        assert_eq!(fmt_capacity(512_110_190_592), "512 GB");
        assert_eq!(fmt_capacity(2_000_398_934_016), "2 TB");
        assert_eq!(fmt_mhz(4157.0), "4.16 GHz");
        assert_eq!(fmt_mhz(800.0), "800 MHz");
    }

    #[test]
    fn rate_maths_bytes_to_per_second() {
        assert_eq!(rate_per_sec(1000, 3000, 2.0), 1000.0);
        assert_eq!(rate_per_sec(1000, 1000, 1.0), 0.0);
        // A counter that went backwards (reset or wrap) must not give a huge spike.
        assert_eq!(rate_per_sec(5000, 100, 1.0), 0.0);
        assert_eq!(rate_per_sec(0, 100, 0.0), 0.0);
    }

    #[test]
    fn bands_follow_the_vendor_tables() {
        assert_eq!(band(SensorKind::CpuIntel, 60.0), Band::Normal);
        assert_eq!(band(SensorKind::CpuIntel, 70.0), Band::Warm);
        assert_eq!(band(SensorKind::CpuIntel, 85.0), Band::Hot);
        // The same 80 C is only warm on a Ryzen.
        assert_eq!(band(SensorKind::CpuAmd, 80.0), Band::Warm);
        assert_eq!(band(SensorKind::CpuIntel, 88.0), Band::Hot);
        assert_eq!(band(SensorKind::Gpu, 83.0), Band::Hot);
        assert_eq!(band(SensorKind::Nvme, 56.0), Band::Warm);
    }

    #[test]
    fn the_ring_keeps_the_last_points() {
        let mut r = Ring::new(3);
        assert!(r.is_empty());
        for i in 0..5 {
            r.push(i as f32);
        }
        assert_eq!(r.iter().collect::<Vec<_>>(), vec![2.0, 3.0, 4.0]);
        assert_eq!(r.len(), 3);
        assert_eq!(r.max(), 4.0);
        r.push(f32::NAN);
        assert_eq!(r.iter().last(), Some(0.0), "NaN never reaches the chart");
    }

    #[test]
    fn history_skips_missing_temperatures() {
        let mut h = History::new();
        let mut l = Live::default();
        l.cpu.load = 12.0;
        l.cpu.temp = Some(Err(Why::NeedsCpuAccess));
        h.push(&l);
        assert_eq!(h.cpu_load.len(), 1);
        assert_eq!(h.cpu_temp.len(), 0, "no fake zero for a missing sensor");
        l.cpu.temp = Some(Ok(44.0));
        h.push(&l);
        assert_eq!(h.cpu_temp.len(), 1);
    }
}
