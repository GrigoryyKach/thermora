//! Windows readings. Everything here works without elevation; the CPU temperature comes from the helper (`link`).
//! Sources and measurements are in `docs/SENSORS.md`.

use super::link::{self, LinkState};
use super::nvidia::Nvidia;
use super::Collector;
use crate::model::*;
use crate::specs::{self, Component, Field, Group, Specs, DASH};
use std::collections::HashMap;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::*;
use windows_sys::Win32::System::Performance::*;
use windows_sys::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::SystemInformation::*;
use wmi::{COMLibrary, Variant, WMIConnection};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ───────────────────────── WMI (specs only) ─────────────────────────

fn vs(v: &Variant) -> String {
    match v {
        Variant::String(s) => s.trim().to_string(),
        Variant::UI1(x) => x.to_string(),
        Variant::UI2(x) => x.to_string(),
        Variant::UI4(x) => x.to_string(),
        Variant::UI8(x) => x.to_string(),
        Variant::I2(x) => x.to_string(),
        Variant::I4(x) => x.to_string(),
        Variant::I8(x) => x.to_string(),
        Variant::Bool(x) => x.to_string(),
        _ => String::new(),
    }
}

/// One WQL query returning only the named fields, so a serial number can never leak through by accident.
fn cimv2(class: &str, fields: &[&str], filter: &str) -> Vec<Vec<String>> {
    let run = || -> Result<Vec<Vec<String>>, String> {
        let com = COMLibrary::new().map_err(|e| e.to_string())?;
        let con = WMIConnection::with_namespace_path("ROOT\\CIMV2", com).map_err(|e| e.to_string())?;
        let q = format!("SELECT {} FROM {} {}", fields.join(","), class, filter);
        let rows: Vec<HashMap<String, Variant>> = con.raw_query(&q).map_err(|e| e.to_string())?;
        Ok(rows.iter().map(|r| fields.iter().map(|f| r.get(*f).map(vs).unwrap_or_default()).collect()).collect())
    };
    run().unwrap_or_default()
}

// ───────────────────────── PDH ─────────────────────────

struct Pdh {
    q: PDH_HQUERY,
    counters: Vec<(&'static str, PDH_HCOUNTER)>,
}

impl Pdh {
    fn new(paths: &[(&'static str, &str)]) -> Pdh {
        let mut q: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: plain PDH calls with NUL-terminated paths.
        unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) };
        let mut counters = Vec::new();
        for (key, path) in paths {
            let mut c: PDH_HCOUNTER = std::ptr::null_mut();
            if unsafe { PdhAddEnglishCounterW(q, wide(path).as_ptr(), 0, &mut c) } == 0 {
                counters.push((*key, c));
            }
        }
        Pdh { q, counters }
    }

    fn collect(&self) {
        // SAFETY: valid query handle.
        unsafe { PdhCollectQueryData(self.q) };
    }

    /// (instance, value) pairs of a wildcard counter.
    fn array(&self, key: &str) -> Vec<(String, f64)> {
        let Some((_, c)) = self.counters.iter().find(|(k, _)| *k == key) else { return vec![] };
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: the standard two-call pattern; the buffer is sized by the first call and outlives the items.
        unsafe {
            let st = PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
            if st != 0x800007D2 || size == 0 {
                return vec![];
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            if PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, buf.as_mut_ptr().cast()) != 0 {
                return vec![];
            }
            let items = buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W;
            (0..count as usize)
                .filter_map(|i| {
                    let it = &*items.add(i);
                    if it.FmtValue.CStatus != 0 && it.FmtValue.CStatus != 1 {
                        return None;
                    }
                    let mut n = 0;
                    while *it.szName.add(n) != 0 {
                        n += 1;
                    }
                    Some((String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, n)), it.FmtValue.Anonymous.doubleValue))
                })
                .collect()
        }
    }
}

// SAFETY: PDH handles are opaque ids usable from any thread; a `Pdh` is only used by the collector thread anyway.
unsafe impl Send for Pdh {}

// ───────────────────────── drives ─────────────────────────

fn open_phys(n: u32) -> Option<HANDLE> {
    let path = wide(&format!(r"\\.\PhysicalDrive{n}"));
    // Access 0 is enough for the query ioctls: no elevation needed.
    // SAFETY: NUL-terminated path.
    let h = unsafe { CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE { None } else { Some(h) }
}

/// IOCTL_STORAGE_QUERY_PROPERTY with a standard query; `extra` is the AdditionalParameters blob.
fn query_prop(h: HANDLE, id: i32, extra: &[u8]) -> Option<Vec<u8>> {
    let mut q = vec![0u8; 12.max(8 + extra.len())]; // sizeof(STORAGE_PROPERTY_QUERY) = 12
    q[0..4].copy_from_slice(&(id as u32).to_le_bytes());
    q[8..8 + extra.len()].copy_from_slice(extra);
    let mut out = vec![0u8; 4096];
    let mut ret = 0u32;
    // SAFETY: in and out buffers are valid for the sizes passed.
    let ok = unsafe { DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, q.as_ptr().cast(), q.len() as u32, out.as_mut_ptr().cast(), out.len() as u32, &mut ret, std::ptr::null_mut()) };
    (ok != 0).then(|| {
        out.truncate(ret as usize);
        out
    })
}

/// The NVMe Health log through StorageDeviceProtocolSpecificProperty: ProtocolType NVMe (3), log page (2),
/// request value 2 (health), data offset 40, length 512. The log follows 8 + 40 bytes of descriptor.
fn nvme_log(h: HANDLE) -> Option<Vec<u8>> {
    let mut extra = Vec::new();
    for v in [3u32, 2, 2, 0, 40, 512, 0, 0, 0, 0] {
        extra.extend_from_slice(&v.to_le_bytes());
    }
    let d = query_prop(h, StorageDeviceProtocolSpecificProperty, &extra)?;
    (d.len() >= 48 + 512).then(|| d[48..48 + 512].to_vec())
}

/// Fallback temperature (`StorageDeviceTemperatureProperty`), first entry at byte 24.
fn drive_temp_fallback(h: HANDLE) -> Option<i32> {
    let d = query_prop(h, StorageDeviceTemperatureProperty, &[])?;
    if d.len() < 28 || u16::from_le_bytes([d[12], d[13]]) == 0 {
        return None;
    }
    let t = i16::from_le_bytes([d[26], d[27]]) as i32;
    (0..150).contains(&t).then_some(t)
}

struct DriveInfo {
    index: u32,
    model: String,
    bus: u32,
    ssd: Option<bool>,
    capacity: u64,
    handle: HANDLE,
}

// SAFETY: the handle is only used from the collector thread that owns the `DriveInfo` list.
unsafe impl Send for DriveInfo {}

fn enumerate_drives() -> Vec<DriveInfo> {
    let mut out = Vec::new();
    for n in 0..32 {
        let Some(h) = open_phys(n) else { continue };
        let (model, bus) = query_prop(h, StorageDeviceProperty, &[]).and_then(|d| specs::parse_storage_descriptor(&d)).unwrap_or_default();
        let ssd = query_prop(h, StorageDeviceSeekPenaltyProperty, &[]).filter(|d| d.len() > 8).map(|d| d[8] == 0);
        let mut geo = [0u8; 64];
        let mut ret = 0u32;
        // SAFETY: output buffer of 64 bytes; DISK_GEOMETRY_EX is 24 + 8 bytes at least.
        unsafe { DeviceIoControl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, std::ptr::null(), 0, geo.as_mut_ptr().cast(), 64, &mut ret, std::ptr::null_mut()) };
        let capacity = i64::from_le_bytes(geo[24..32].try_into().unwrap()).max(0) as u64;
        if model.is_empty() && capacity == 0 {
            // SAFETY: handle opened above.
            unsafe { CloseHandle(h) };
            continue;
        }
        out.push(DriveInfo { index: n, model, bus, ssd, capacity, handle: h });
    }
    out
}

/// `0 C: E:` (the PDH instance of a physical disk) to (0, ["C:", "E:"]).
pub fn parse_disk_instance(name: &str) -> Option<(u32, Vec<String>)> {
    let mut it = name.split_whitespace();
    let idx = it.next()?.parse().ok()?;
    Some((idx, it.filter(|t| t.ends_with(':')).map(str::to_string).collect()))
}

fn volume(letter: &str) -> Option<VolumeLive> {
    let root = wide(&format!("{letter}\\"));
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: valid pointers to u64s and a NUL-terminated root path.
    unsafe {
        if GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) == 0 {
            return None;
        }
        let mut fs = [0u16; 32];
        GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), 32);
        let n = fs.iter().position(|c| *c == 0).unwrap_or(0);
        Some(VolumeLive { name: letter.to_string(), fs: String::from_utf16_lossy(&fs[..n]), total, free })
    }
}

// ───────────────────────── the collector ─────────────────────────

pub struct WinCollector {
    pdh: Pdh,
    gpu_pdh: Option<Pdh>,
    base_mhz: f32,
    vendor: CpuVendor,
    drives: Vec<DriveInfo>,
    nvidia: Option<Nvidia>,
    gpu_tick: u32,
}

fn cpu_vendor() -> CpuVendor {
    let v = std::arch::x86_64::__cpuid(0);
    let mut s = Vec::new();
    for r in [v.ebx, v.edx, v.ecx] {
        s.extend_from_slice(&r.to_le_bytes());
    }
    match s.as_slice() {
        b"GenuineIntel" => CpuVendor::Intel,
        b"AuthenticAMD" => CpuVendor::Amd,
        _ => CpuVendor::Other,
    }
}

fn base_clock_mhz(logical: usize) -> f32 {
    let mut info = vec![[0u32; 6]; logical.max(1)];
    // SAFETY: PROCESSOR_POWER_INFORMATION is six u32s per logical processor.
    let st = unsafe { CallNtPowerInformation(ProcessorInformation, std::ptr::null(), 0, info.as_mut_ptr().cast(), (info.len() * 24) as u32) };
    if st == 0 { info[0][1] as f32 } else { 0.0 }
}

impl WinCollector {
    pub fn new() -> Self {
        let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let pdh = Pdh::new(&[
            ("cpu_load", r"\Processor Information(*)\% Processor Time"),
            ("cpu_perf", r"\Processor Information(*)\% Processor Performance"),
            ("disk_busy", r"\PhysicalDisk(*)\% Disk Time"),
            ("disk_r", r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            ("disk_w", r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            ("net_rx", r"\Network Interface(*)\Bytes Received/sec"),
            ("net_tx", r"\Network Interface(*)\Bytes Sent/sec"),
            ("net_bw", r"\Network Interface(*)\Current Bandwidth"),
        ]);
        pdh.collect();
        link::start();
        let nvidia = Nvidia::new();
        // The expensive GPU Engine counter is only needed when there is no NVML to ask.
        let gpu_pdh = nvidia.is_none().then(|| {
            let p = Pdh::new(&[("eng", r"\GPU Engine(*)\Utilization Percentage"), ("mem", r"\GPU Adapter Memory(*)\Dedicated Usage")]);
            p.collect();
            p
        });
        Self { pdh, gpu_pdh, base_mhz: base_clock_mhz(logical), vendor: cpu_vendor(), drives: enumerate_drives(), nvidia, gpu_tick: 0 }
    }

    fn cpu(&self) -> CpuLive {
        let load = self.pdh.array("cpu_load");
        let perf = self.pdh.array("cpu_perf");
        let total = load.iter().find(|(n, _)| n == "_Total").map(|x| x.1).unwrap_or(0.0) as f32;
        let mut per_core: Vec<(String, f32)> = load.iter().filter(|(n, _)| n.contains(',') && !n.contains("_Total")).map(|(n, v)| (n.clone(), *v as f32)).collect();
        per_core.sort_by_key(|(n, _)| n.split(',').filter_map(|p| p.parse::<u32>().ok()).fold((0, 0), |a, x| (a.1, x)));
        let clock = perf.iter().find(|(n, _)| n == "_Total").map(|x| x.1 as f32 / 100.0 * self.base_mhz).filter(|c| *c > 0.0);
        let mut cpu = CpuLive { vendor: self.vendor, load: total.clamp(0.0, 100.0), per_core: per_core.into_iter().map(|x| x.1.clamp(0.0, 100.0)).collect(), clock_mhz: clock, ..Default::default() };
        let (t, h, p) = match link::state() {
            LinkState::NotSetUp => (Err(Why::NeedsCpuAccess), Err(Why::NeedsCpuAccess), Err(Why::NeedsCpuAccess)),
            LinkState::Starting => (Err(Why::HelperStarting), Err(Why::HelperStarting), Err(Why::HelperStarting)),
            LinkState::Unsupported(_) => (Err(Why::HelperUnsupported), Err(Why::HelperUnsupported), Err(Why::HelperUnsupported)),
            LinkState::Ok(s) => (
                s.pkg.map(|v| v as f32).ok_or(Why::NotReported),
                s.hot.map(|v| v as f32).ok_or(Why::NotReported),
                s.watts.map(|v| v as f32).ok_or(Why::HelperStarting),
            ),
        };
        cpu.temp = Some(t);
        cpu.hotspot = Some(h);
        cpu.power = Some(p);
        cpu
    }

    fn ram(&self) -> RamLive {
        // SAFETY: structs are sized and zero-initialised before the calls.
        unsafe {
            let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..std::mem::zeroed() };
            GlobalMemoryStatusEx(&mut m);
            let mut pi: PERFORMANCE_INFORMATION = std::mem::zeroed();
            pi.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
            K32GetPerformanceInfo(&mut pi, pi.cb);
            RamLive { total: m.ullTotalPhys, used: m.ullTotalPhys - m.ullAvailPhys, available: m.ullAvailPhys, cached: (pi.SystemCache * pi.PageSize) as u64 }
        }
    }

    fn disks(&self) -> Vec<DiskLive> {
        let (busy, rd, wr) = (self.pdh.array("disk_busy"), self.pdh.array("disk_r"), self.pdh.array("disk_w"));
        let mut out = Vec::new();
        for d in &self.drives {
            let inst = busy.iter().find(|(n, _)| parse_disk_instance(n).is_some_and(|(i, _)| i == d.index));
            let Some((name, b)) = inst else { continue };
            let letters = parse_disk_instance(name).map(|x| x.1).unwrap_or_default();
            let get = |v: &Vec<(String, f64)>| v.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            // SMART composite first (worked on every NVMe tested), the plain temperature ioctl as fallback.
            let temp = nvme_log(d.handle).and_then(|l| specs::parse_nvme_health(&l)).map(|h| h.temp_c).or_else(|| drive_temp_fallback(d.handle));
            let title = if d.model.is_empty() { format!("Disk {}", d.index) } else { d.model.clone() };
            out.push(DiskLive {
                name: title,
                busy: (*b as f32).clamp(0.0, 100.0),
                read: get(&rd),
                write: get(&wr),
                temp: Some(temp.map(|t| t as f32).ok_or(Why::NotReported)),
                volumes: letters.iter().filter_map(|l| volume(l)).collect(),
            });
        }
        out
    }

    fn net(&self) -> NetLive {
        let (rx, tx, bw) = (self.pdh.array("net_rx"), self.pdh.array("net_tx"), self.pdh.array("net_bw"));
        let mut best: Option<NetLive> = None;
        let mut best_score = -1.0;
        for (name, r) in &rx {
            let t = tx.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            let b = bw.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            if b <= 0.0 || !specs::is_real_adapter(name, 6, true) {
                continue;
            }
            // Traffic decides; bandwidth breaks ties so an idle PC still names its main adapter.
            let score = (r + t) + b / 1e12;
            if score > best_score {
                best_score = score;
                best = Some(NetLive { adapter: name.replace('[', "(").replace(']', ")"), down: *r, up: t });
            }
        }
        best.unwrap_or_default()
    }

    fn gpu(&mut self) -> Option<GpuLive> {
        if let Some(n) = &self.nvidia {
            return n.live();
        }
        // Any vendor: what Task Manager shows. Temperature, power and fan are "not reported" without a vendor API.
        let p = self.gpu_pdh.as_ref()?;
        self.gpu_tick += 1;
        p.collect();
        let eng = p.array("eng");
        if eng.is_empty() {
            return None;
        }
        let load = eng.iter().filter(|(n, _)| n.ends_with("engtype_3D")).map(|x| x.1).sum::<f64>().min(100.0) as f32;
        let vram: f64 = p.array("mem").iter().map(|x| x.1).sum();
        let nr = Some(Err(Why::NotReported));
        Some(GpuLive { name: String::new(), load, temp: nr.clone(), hotspot: nr.clone(), mem_temp: nr.clone(), vram_used: vram as u64, vram_total: 0, clock_mhz: nr.clone(), power: nr.clone(), power_limit: nr.clone(), fan: nr })
    }
}

impl Collector for WinCollector {
    fn poll(&mut self) -> Live {
        self.pdh.collect();
        let cpu = self.cpu();
        let access_missing = matches!(link::state(), LinkState::NotSetUp);
        Live { cpu, gpu: self.gpu(), ram: self.ram(), net: self.net(), disks: self.disks(), cpu_access_missing: access_missing && self.vendor != CpuVendor::Other, ready: true }
    }
}

impl Drop for WinCollector {
    fn drop(&mut self) {
        for d in &self.drives {
            // SAFETY: handles opened by `enumerate_drives`, closed once.
            unsafe { CloseHandle(d.handle) };
        }
    }
}

// ───────────────────────── specs ─────────────────────────

fn cpu_specs() -> Component {
    let v = std::arch::x86_64::__cpuid(0);
    let mut vb = Vec::new();
    for r in [v.ebx, v.edx, v.ecx] {
        vb.extend_from_slice(&r.to_le_bytes());
    }
    let vendor = String::from_utf8_lossy(&vb).to_string();
    let mut brand = Vec::new();
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        let r = std::arch::x86_64::__cpuid(leaf);
        for x in [r.eax, r.ebx, r.ecx, r.edx] {
            brand.extend_from_slice(&x.to_le_bytes());
        }
    }
    let brand = String::from_utf8_lossy(&brand).trim_matches('\0').trim().to_string();
    let s = std::arch::x86_64::__cpuid(1).eax;
    let (model_lo, fam_lo) = ((s >> 4) & 0xF, (s >> 8) & 0xF);
    let family = if fam_lo == 0xF { fam_lo + ((s >> 20) & 0xFF) } else { fam_lo };
    let model = if fam_lo == 0x6 || fam_lo == 0xF { model_lo | (((s >> 16) & 0xF) << 4) } else { model_lo };
    let (code, socket) = specs::codename(&vendor, family, model).map(|(c, s)| (c.to_string(), s.to_string())).unwrap_or_default();
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let (cores, caches) = topology();
    let cache = |lvl: u8, ty: &[u32]| -> String {
        let parts: Vec<String> = caches.iter().filter(|((l, t), _)| *l == lvl && ty.contains(t)).map(|(_, (n, size))| format!("{n} x {}", kb(*size))).collect();
        parts.join(" + ")
    };
    let base = base_clock_mhz(logical);
    let mfr = match vendor.as_str() {
        "GenuineIntel" => "Intel",
        "AuthenticAMD" => "AMD",
        o => o,
    };
    let clean = brand.replace("(R)", "").replace("(TM)", "").replace("  ", " ");
    Component::new(Group::Cpu, clean.clone())
        .sub(format!("{cores} cores, {logical} threads"))
        .row(Field::Manufacturer, mfr)
        .row(Field::Model, clean)
        .row(Field::Codename, code)
        .row(Field::Socket, socket)
        .row(Field::Cores, cores.to_string())
        .row(Field::Threads, logical.to_string())
        .row(Field::BaseClock, if base > 0.0 { fmt_mhz(base) } else { String::new() })
        .row(Field::L1, [cache(1, &[2]), cache(1, &[1])].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" + "))
        .row(Field::L2, cache(2, &[0]))
        .row(Field::L3, cache(3, &[0]))
        .row(Field::Tdp, specs::tdp_watts(&brand).map(|w| format!("{w} W")).unwrap_or_default())
}

fn kb(bytes: u32) -> String {
    if bytes >= 1024 * 1024 { format!("{} MB", bytes / 1024 / 1024) } else { format!("{} KB", bytes / 1024) }
}

/// (physical cores, caches as ((level, type) -> (count, size))). Type: 0 unified, 1 instruction, 2 data.
fn topology() -> (u32, std::collections::BTreeMap<(u8, u32), (u32, u32)>) {
    let mut caches: std::collections::BTreeMap<(u8, u32), (u32, u32)> = Default::default();
    let mut cores = 0;
    // SAFETY: standard two-call pattern; records are parsed with bounds checks.
    unsafe {
        let mut len = 0u32;
        GetLogicalProcessorInformationEx(RelationAll, std::ptr::null_mut(), &mut len);
        let mut buf = vec![0u8; len as usize];
        if len == 0 || GetLogicalProcessorInformationEx(RelationAll, buf.as_mut_ptr().cast(), &mut len) == 0 {
            return (0, caches);
        }
        let mut off = 0usize;
        while off + 20 <= len as usize {
            let rel = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            let size = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
            if rel == 0 {
                cores += 1;
            } else if rel == 2 {
                let level = buf[off + 8];
                let cache = u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap());
                let ty = u32::from_le_bytes(buf[off + 16..off + 20].try_into().unwrap());
                caches.entry((level, ty)).or_insert((0, cache)).0 += 1;
            }
            if size == 0 {
                break;
            }
            off += size;
        }
    }
    (cores, caches)
}

fn ipv4_of(a: &IP_ADAPTER_ADDRESSES_LH) -> String {
    // The first IPv4 unicast address (the query asked for AF_INET only). Local address, shown on the specs page only.
    // SAFETY: the list and sockaddr pointers come from GetAdaptersAddresses and live as long as the buffer.
    unsafe {
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let sa = (*u).Address.lpSockaddr;
            if !sa.is_null() && (*sa).sa_family == 2 {
                let b = (*sa).sa_data;
                return format!("{}.{}.{}.{}", b[2] as u8, b[3] as u8, b[4] as u8, b[5] as u8);
            }
            u = (*u).Next;
        }
    }
    String::new()
}

unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn network_specs() -> Vec<Component> {
    let mut out = Vec::new();
    let mut len = 32768u32;
    let mut buf = vec![0u64; len as usize / 8];
    // SAFETY: the buffer is 8-byte aligned and its size is passed; the list is only read while `buf` lives.
    unsafe {
        if GetAdaptersAddresses(2, 0x2 | 0x4 | 0x8, std::ptr::null(), buf.as_mut_ptr().cast(), &mut len) != 0 {
            return out;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            let desc = pwstr(a.Description);
            if specs::is_real_adapter(&desc, a.IfType, a.OperStatus == 1) {
                let mbps = a.TransmitLinkSpeed / 1_000_000;
                let speed = if mbps >= 1000 { format!("{} Gbps", mbps as f64 / 1000.0) } else { format!("{mbps} Mbps") };
                // The MAC address (a.PhysicalAddress) is never read.
                out.push(Component::new(Group::Network, desc.clone()).sub(speed.clone()).row(Field::Model, desc).row(Field::LinkSpeed, speed).row(Field::Address, ipv4_of(a)));
            }
            p = a.Next;
        }
    }
    out
}

fn storage_specs() -> Vec<Component> {
    let vols: Vec<_> = cimv2("Win32_LogicalDisk", &["DeviceID", "FileSystem", "Size", "FreeSpace"], "WHERE DriveType=3");
    let mut out = Vec::new();
    // The PDH instance names map drive letters to physical drives (`0 C: E:`).
    let pdh = Pdh::new(&[("busy", r"\PhysicalDisk(*)\% Disk Time")]);
    pdh.collect();
    let inst: Vec<(u32, Vec<String>)> = pdh.array("busy").iter().filter_map(|(n, _)| parse_disk_instance(n)).collect();
    for d in enumerate_drives() {
        let health = nvme_log(d.handle).and_then(|l| specs::parse_nvme_health(&l));
        let letters = inst.iter().find(|(i, _)| *i == d.index).map(|x| x.1.clone()).unwrap_or_default();
        let volumes: Vec<String> = letters
            .iter()
            .filter_map(|l| vols.iter().find(|v| v[0].eq_ignore_ascii_case(l)))
            .map(|v| {
                let (s, f): (u64, u64) = (v[2].parse().unwrap_or(0), v[3].parse().unwrap_or(0));
                format!("{} {}, {} free of {}", v[0], v[1], fmt_bytes(f), fmt_bytes(s))
            })
            .collect();
        let bus = specs::bus_name(d.bus);
        let media = match d.ssd {
            Some(true) => "SSD",
            Some(false) => "HDD",
            None => "",
        };
        let temp = health.as_ref().map(|h| h.temp_c).or_else(|| drive_temp_fallback(d.handle));
        let title = if d.model.is_empty() { format!("Disk {}", d.index) } else { d.model.clone() };
        let mfr = d.model.split_whitespace().next().unwrap_or("").to_string();
        out.push(
            Component::new(Group::Storage, title.clone())
                .sub(format!("{bus} {}", fmt_capacity(d.capacity)).trim().to_string())
                .row(Field::Manufacturer, mfr)
                .row(Field::Model, title)
                .row(Field::Capacity, fmt_capacity(d.capacity))
                .row(Field::Bus, bus)
                .row(Field::Media, media)
                .row(Field::Health, health.as_ref().map(|h| h.summary()).unwrap_or_default())
                .row(Field::Temperature, temp.map(|t| format!("{t} °C")).unwrap_or_default())
                .row(Field::Written, health.as_ref().map(|h| format!("{:.1} TB", h.written_tb)).unwrap_or_default())
                .row(Field::PowerOn, health.as_ref().map(|h| format!("{} h", h.power_on_hours)).unwrap_or_default())
                .row(Field::Volumes, volumes.join("\n")),
        );
        // SAFETY: handle from `enumerate_drives`, closed once.
        unsafe { CloseHandle(d.handle) };
    }
    out
}

/// Gathers everything once. Takes a second or two (WMI), so it runs on a background thread.
pub fn collect_specs() -> Specs {
    let mut comps = vec![cpu_specs()];
    match Nvidia::new() {
        Some(n) => comps.extend(n.specs()),
        None => {
            for r in cimv2("Win32_VideoController", &["Name", "DriverVersion"], "") {
                if r[0].is_empty() || r[0].contains("Basic") || r[0].contains("Remote") {
                    continue;
                }
                comps.push(Component::new(Group::Gpu, r[0].clone()).row(Field::Model, r[0].clone()).row(Field::Driver, r[1].clone()));
            }
        }
    }
    if let Some(b) = cimv2("Win32_BaseBoard", &["Manufacturer", "Product"], "").into_iter().next() {
        let bios = cimv2("Win32_BIOS", &["Manufacturer", "SMBIOSBIOSVersion", "ReleaseDate"], "").into_iter().next().unwrap_or_default();
        let g = |i: usize| bios.get(i).cloned().unwrap_or_default();
        let date = g(2);
        let date = if date.len() >= 8 { format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..8]) } else { date };
        // The chipset has no friendly name in WMI: its LPC/eSPI bridge shows up as a generic PCI-ISA bridge.
        let chipset = cimv2("Win32_PnPEntity", &["DeviceID"], "WHERE DeviceID LIKE 'PCI%'").iter().filter_map(|r| specs::pci_dev_id(&r[0])).find_map(|d| specs::chipset_name(&d)).unwrap_or("");
        let title = format!("{} {}", b[0].replace("ASUSTeK COMPUTER INC.", "ASUS"), b[1]).trim().to_string();
        comps.push(
            Component::new(Group::Board, title)
                .sub(format!("BIOS {}", g(1)))
                .row(Field::Manufacturer, b[0].clone())
                .row(Field::Model, b[1].clone())
                .row(Field::Chipset, chipset)
                .row(Field::BiosVendor, g(0))
                .row(Field::BiosVersion, g(1))
                .row(Field::BiosDate, date),
        );
    }
    for r in cimv2("Win32_PhysicalMemory", &["Manufacturer", "PartNumber", "Capacity", "Speed", "ConfiguredClockSpeed", "SMBIOSMemoryType", "DeviceLocator", "ConfiguredVoltage"], "") {
        comps.push(specs::memory_module(&r));
    }
    comps.extend(storage_specs());
    // Endpoints carry the product names people know; Win32_SoundDevice only has driver names.
    let eps: Vec<String> = cimv2("Win32_PnPEntity", &["Name"], "WHERE PNPClass='AudioEndpoint'").into_iter().map(|r| r[0].clone()).filter(|n| !n.is_empty()).collect();
    for (dev, kinds) in specs::group_audio(&eps) {
        comps.push(Component::new(Group::Audio, dev.clone()).sub(kinds.join(", ")).row(Field::Model, dev).row(Field::Status, kinds.join(", ")));
    }
    comps.extend(network_specs());
    let _ = DASH;
    Specs { comps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdh_disk_instances_map_to_drive_letters() {
        assert_eq!(parse_disk_instance("0 C: E:"), Some((0, vec!["C:".into(), "E:".into()])));
        assert_eq!(parse_disk_instance("1 D:"), Some((1, vec!["D:".into()])));
        assert_eq!(parse_disk_instance("2"), Some((2, vec![])));
        assert_eq!(parse_disk_instance("_Total"), None);
    }

    #[test]
    fn the_collector_gives_real_numbers_on_this_machine() {
        let mut c = WinCollector::new();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let l = c.poll();
        assert!(l.ram.total > 1 << 30 && l.ram.used <= l.ram.total);
        assert!(l.cpu.load >= 0.0 && l.cpu.load <= 100.0);
        assert!(!l.cpu.per_core.is_empty());
        // Without the helper the CPU temperature is a reason, never a number.
        if !link::enabled() {
            assert_eq!(l.cpu.temp, Some(Err(Why::NeedsCpuAccess)));
        }
    }

    #[test]
    fn specs_are_collected_and_contain_nothing_private() {
        let s = collect_specs();
        assert!(s.comps.iter().any(|c| c.group == Group::Cpu));
        assert!(s.comps.iter().any(|c| c.group == Group::Ram));
        let text = specs::copy_text(&s, crate::i18n::Lang::En).to_lowercase();
        assert!(!text.contains("serial"));
        // No MAC-address-looking text anywhere.
        let mac = text.split_whitespace().any(|w| w.len() == 17 && w.matches(|c| c == '-' || c == ':').count() == 5);
        assert!(!mac);
    }
}
