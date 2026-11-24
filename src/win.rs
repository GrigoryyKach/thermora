//! Windows readings. Everything here works without elevation except the PawnIO part.

use crate::line;
use crate::pawnio;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::*;
use windows_sys::Win32::System::Performance::*;
use windows_sys::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, K32GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::*;
use wmi::{COMLibrary, Variant, WMIConnection};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ---------------------------------------------------------------- WMI

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
        Variant::Null | Variant::Empty => String::new(),
        _ => "?".into(),
    }
}

/// Run one WQL query, return rows of the selected fields only (so serials can never leak through by accident).
fn wmi_rows(ns: &str, class: &str, fields: &[&str], filter: &str) -> Result<Vec<Vec<String>>, String> {
    let com = COMLibrary::new().map_err(|e| e.to_string())?;
    let con = WMIConnection::with_namespace_path(ns, com).map_err(|e| e.to_string())?;
    let q = format!("SELECT {} FROM {} {}", fields.join(","), class, filter);
    let rows: Vec<HashMap<String, Variant>> = con.raw_query(&q).map_err(|e| e.to_string())?;
    Ok(rows.iter().map(|r| fields.iter().map(|f| r.get(*f).map(vs).unwrap_or_default()).collect()).collect())
}

fn cimv2(class: &str, fields: &[&str], filter: &str) -> Result<Vec<Vec<String>>, String> {
    wmi_rows("ROOT\\CIMV2", class, fields, filter)
}

// ---------------------------------------------------------------- PDH

struct Pdh {
    q: PDH_HQUERY,
    counters: Vec<(&'static str, PDH_HCOUNTER)>,
}

impl Pdh {
    fn new(paths: &[(&'static str, &str)]) -> Pdh {
        let mut q: PDH_HQUERY = std::ptr::null_mut();
        unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) };
        let mut counters = Vec::new();
        for (key, path) in paths {
            let mut c: PDH_HCOUNTER = std::ptr::null_mut();
            let st = unsafe { PdhAddEnglishCounterW(q, wide(path).as_ptr(), 0, &mut c) };
            if st == 0 {
                counters.push((*key, c));
            }
        }
        Pdh { q, counters }
    }
    fn collect(&self) {
        unsafe { PdhCollectQueryData(self.q) };
    }
    /// (instance, value) pairs of a wildcard counter, in PDH order.
    fn array(&self, key: &str) -> Vec<(String, f64)> {
        let Some((_, c)) = self.counters.iter().find(|(k, _)| *k == key) else { return vec![] };
        let (mut size, mut count) = (0u32, 0u32);
        let st = unsafe { PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut()) };
        if st != 0x800007D2 || size == 0 {
            return vec![];
        }
        let mut buf = vec![0u8; size as usize];
        let st = unsafe { PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, buf.as_mut_ptr().cast()) };
        if st != 0 {
            return vec![];
        }
        let items = buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W;
        (0..count as usize)
            .filter_map(|i| unsafe {
                let it = &*items.add(i);
                if it.FmtValue.CStatus != 0 && it.FmtValue.CStatus != 0x00000001 {
                    return None; // not PDH_CSTATUS_VALID_DATA / NEW_DATA
                }
                let mut n = 0;
                while *it.szName.add(n) != 0 {
                    n += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, n));
                Some((name, it.FmtValue.Anonymous.doubleValue))
            })
            .collect()
    }
}

// ---------------------------------------------------------------- probe

pub struct Probe {
    pdh: Pdh,
    logical: usize,
    max_mhz: u32,
    elevated: bool,
    pawn: Result<pawnio::Pawn, String>,
    module_loaded: bool,
    last_energy: Option<(f64, std::time::Instant)>,
}

fn is_elevated() -> bool {
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 0x0008, &mut tok) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret = 0u32;
        let ok = GetTokenInformation(tok, TokenElevation, (&mut e as *mut TOKEN_ELEVATION).cast(), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut ret);
        CloseHandle(tok);
        ok != 0 && e.TokenIsElevated != 0
    }
}
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION};

fn pawnio_installed_version() -> Option<String> {
    // The installer registers an uninstall entry; this is what LibreHardwareMonitor checks too.
    // Reading HKLM needs no elevation. Done through `reg query` equivalent API calls.
    use windows_sys::Win32::System::Registry::*;
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO");
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ | 0x0100, &mut key) != 0 {
            return None;
        }
        let mut buf = [0u16; 64];
        let mut size = (buf.len() * 2) as u32;
        let name = wide("DisplayVersion");
        let st = RegQueryValueExW(key, name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size);
        RegCloseKey(key);
        if st != 0 {
            return Some("installed (version unknown)".into());
        }
        Some(String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
    }
}

impl Probe {
    pub fn new(module_dir: Option<String>) -> Probe {
        let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let mut max_mhz = 0;
        unsafe {
            let mut info = vec![[0u32; 6]; logical];
            if CallNtPowerInformation(ProcessorInformation, std::ptr::null(), 0, info.as_mut_ptr().cast(), (logical * 24) as u32) == 0 {
                max_mhz = info[0][1];
            }
        }
        let pdh = Pdh::new(&[
            ("cpu_load", r"\Processor Information(*)\% Processor Time"),
            ("cpu_perf", r"\Processor Information(*)\% Processor Performance"),
            ("disk_busy", r"\PhysicalDisk(*)\% Disk Time"),
            ("disk_r", r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            ("disk_w", r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            ("net_rx", r"\Network Interface(*)\Bytes Received/sec"),
            ("net_tx", r"\Network Interface(*)\Bytes Sent/sec"),
            ("net_bw", r"\Network Interface(*)\Current Bandwidth"),
            ("tz", r"\Thermal Zone Information(*)\High Precision Temperature"),
            ("gpu_eng", r"\GPU Engine(*)\Utilization Percentage"),
            ("gpu_mem", r"\GPU Adapter Memory(*)\Dedicated Usage"),
        ]);
        let elevated = is_elevated();
        let pawn = pawnio::open();
        let mut module_loaded = false;
        if let (Ok(p), Some(dir)) = (&pawn, &module_dir) {
            if let Ok(blob) = std::fs::read(std::path::Path::new(dir).join("IntelMSR.bin")) {
                module_loaded = p.load(&blob).is_ok();
            }
        }
        Probe { pdh, logical, max_mhz, elevated, pawn, module_loaded, last_energy: None }
    }

    pub fn process_cost(&self) -> (f64, f64, f64) {
        unsafe {
            let (mut c, mut e, mut k, mut u) = (std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>());
            GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
            let t = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 1e7;
            let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb);
            (t(k) + t(u), m.WorkingSetSize as f64 / 1048576.0, m.PeakWorkingSetSize as f64 / 1048576.0)
        }
    }

    // ------------------------------------------------------------ specs
    pub fn specs(&mut self) {
        println!(" access: elevated={}  PawnIO={}", self.elevated, pawnio_installed_version().unwrap_or_else(|| "not installed".into()));
        match &self.pawn {
            Ok(_) => println!(" PawnIO device opened (module IntelMSR loaded: {})", self.module_loaded),
            Err(e) => println!(" PawnIO device: {e}"),
        }
        self.cpu_specs();
        self.ram_specs();
        self.disk_specs();
        self.net_specs();
        self.board_specs();
    }

    fn cpu_specs(&self) {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::__cpuid;
            let v = __cpuid(0);
            let mut vendor = Vec::new();
            for r in [v.ebx, v.edx, v.ecx] {
                vendor.extend_from_slice(&r.to_le_bytes());
            }
            let vendor = String::from_utf8_lossy(&vendor).to_string();
            let mut brand = Vec::new();
            for leaf in 0x8000_0002u32..=0x8000_0004 {
                let r = __cpuid(leaf);
                for x in [r.eax, r.ebx, r.ecx, r.edx] {
                    brand.extend_from_slice(&x.to_le_bytes());
                }
            }
            let brand = String::from_utf8_lossy(&brand).trim_matches('\0').trim().to_string();
            let s = __cpuid(1).eax;
            let (stepping, model_lo, fam_lo) = (s & 0xF, (s >> 4) & 0xF, (s >> 8) & 0xF);
            let family = if fam_lo == 0xF { fam_lo + ((s >> 20) & 0xFF) } else { fam_lo };
            let model = if fam_lo == 0x6 || fam_lo == 0xF { model_lo | (((s >> 16) & 0xF) << 4) } else { model_lo };
            line("CPU", "name (CPUID brand)", Ok(brand.clone()));
            line("CPU", "vendor / family-model-stepping", Ok(format!("{vendor} / {family:#x}-{model:#x}-{stepping}")));
            let (code, socket) = codename(&vendor, family, model);
            line("CPU", "codename / socket (our table)", Ok(format!("{code} / {socket}")));
            line("CPU", "TDP (our table by brand)", tdp(&brand).map(|w| format!("{w} W base")).ok_or("unknown model; MSR_PKG_POWER_INFO needs the driver".into()));
        }
        line("CPU", "base clock (CallNtPowerInformation)", Ok(format!("{} MHz", self.max_mhz)));
        self.topology();
        match cimv2("Win32_Processor", &["Name", "SocketDesignation", "MaxClockSpeed", "NumberOfCores", "NumberOfLogicalProcessors"], "") {
            Ok(r) => {
                for row in r {
                    line("CPU", "WMI name / socket string", Ok(format!("{} / {}", row[0], row[1])));
                    line("CPU", "WMI MaxClockSpeed (MHz)", Ok(row[2].clone()));
                }
            }
            Err(e) => line("CPU", "WMI Win32_Processor", Err(e)),
        }
    }

    fn topology(&self) {
        unsafe {
            let mut len = 0u32;
            GetLogicalProcessorInformationEx(RelationAll, std::ptr::null_mut(), &mut len);
            let mut buf = vec![0u8; len as usize];
            if GetLogicalProcessorInformationEx(RelationAll, buf.as_mut_ptr().cast(), &mut len) == 0 {
                return line("CPU", "topology", Err("GetLogicalProcessorInformationEx failed".into()));
            }
            let (mut cores, mut smt, mut eff0, mut eff1) = (0, 0, 0, 0);
            let mut caches: std::collections::BTreeMap<(u8, u32), (u32, u32)> = Default::default();
            let mut off = 0usize;
            while off + 8 <= len as usize {
                let rel = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
                let size = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
                match rel {
                    0 => {
                        cores += 1;
                        if buf[off + 8] & 1 != 0 {
                            smt += 1;
                        }
                        if buf[off + 9] == 0 { eff0 += 1 } else { eff1 += 1 }
                    }
                    2 => {
                        let level = buf[off + 8];
                        let cache = u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap());
                        let ty = u32::from_le_bytes(buf[off + 16..off + 20].try_into().unwrap());
                        let e = caches.entry((level, ty)).or_insert((0, cache));
                        e.0 += 1;
                    }
                    _ => {}
                }
                if size == 0 {
                    break;
                }
                off += size;
            }
            line("CPU", "cores / threads", Ok(format!("{cores} / {} ({} with SMT)", self.logical, smt)));
            if eff0 > 0 && eff1 > 0 {
                line("CPU", "hybrid split (eff. class 0 / higher)", Ok(format!("{eff0} / {eff1}")));
            }
            for ((lvl, ty), (n, size)) in caches {
                let t = ["unified", "instr", "data", "trace"].get(ty as usize).copied().unwrap_or("?");
                line("CPU", &format!("L{lvl} cache ({t})"), Ok(format!("{n} x {} KB", size / 1024)));
            }
        }
    }

    fn ram_specs(&self) {
        match cimv2("Win32_PhysicalMemory", &["Manufacturer", "PartNumber", "Capacity", "Speed", "ConfiguredClockSpeed", "SMBIOSMemoryType", "DeviceLocator", "ConfiguredVoltage"], "") {
            Ok(rows) => {
                for r in rows {
                    let ty = match r[5].as_str() { "26" => "DDR4", "34" => "DDR5", "24" => "DDR3", "0" | "" => "?", o => o };
                    let cap: u64 = r[2].parse().unwrap_or(0);
                    line(
                        "RAM",
                        &format!("module {}", r[6]),
                        Ok(format!("{} {} {} GB {} rated {} MT/s, configured {} MT/s, {} mV", r[0], r[1], cap >> 30, ty, r[3], r[4], r[7])),
                    );
                }
            }
            Err(e) => line("RAM", "Win32_PhysicalMemory", Err(e)),
        }
        line("RAM", "timings (CL-tRCD-tRP-tRAS)", Err("needs SPD over SMBus: SmbusI801 PawnIO module, elevated; not implemented in T1".into()));
    }

    fn board_specs(&self) {
        // Serial-number properties are deliberately not in the field lists.
        match cimv2("Win32_BaseBoard", &["Manufacturer", "Product"], "") {
            Ok(r) => r.iter().for_each(|x| line("Board", "manufacturer / model", Ok(format!("{} / {}", x[0], x[1])))),
            Err(e) => line("Board", "Win32_BaseBoard", Err(e)),
        }
        match cimv2("Win32_BIOS", &["Manufacturer", "SMBIOSBIOSVersion", "ReleaseDate"], "") {
            Ok(r) => r.iter().for_each(|x| line("Board", "BIOS vendor / version / date", Ok(format!("{} / {} / {}", x[0], x[1], x[2].get(..8).unwrap_or(&x[2]))))),
            Err(e) => line("Board", "Win32_BIOS", Err(e)),
        }
        // The chipset has no friendly name in WMI: its LPC/eSPI bridge shows up as a generic "PCI-ISA bridge".
        // PCI device ids are bus identifiers, not serials. We map the id with our own table.
        match cimv2("Win32_PnPEntity", &["DeviceID"], "WHERE DeviceID LIKE 'PCI%'") {
            Ok(r) => {
                let id = r.iter().filter_map(|x| x[0].split("DEV_").nth(1).and_then(|d| d.get(..4)).map(|d| d.to_string())).find_map(|d| chipset_name(&d).map(|n| (d, n)));
                match id {
                    Some((d, n)) => line("Board", "chipset (PCI id via our table)", Ok(format!("{n} (DEV_{d})"))),
                    None => line("Board", "chipset", Err("LPC bridge id not in our table".into())),
                }
            }
            Err(e) => line("Board", "chipset", Err(e)),
        }
        // Endpoints carry the product names the user knows ("Microphone (fifine SC3)"); Win32_SoundDevice only has driver names.
        match cimv2("Win32_PnPEntity", &["Name"], "WHERE PNPClass='AudioEndpoint'") {
            Ok(r) => r.iter().for_each(|x| line("Audio", "endpoint", Ok(x[0].clone()))),
            Err(e) => line("Audio", "audio endpoints", Err(e)),
        }
    }

    fn net_specs(&self) {
        unsafe {
            let mut len = 32768u32;
            let mut buf = vec![0u8; len as usize];
            let st = GetAdaptersAddresses(2, 0x2 | 0x4 | 0x8, std::ptr::null(), buf.as_mut_ptr().cast(), &mut len);
            if st != 0 {
                return line("Net", "GetAdaptersAddresses", Err(format!("error {st}")));
            }
            let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
            while !p.is_null() {
                let a = &*p;
                let name = pwstr(a.FriendlyName);
                let desc = pwstr(a.Description);
                // IfType 24 is loopback, 131 tunnel. MAC (PhysicalAddress) and the IP value are NOT printed.
                if a.IfType != 24 {
                    let has_v4 = !a.FirstUnicastAddress.is_null();
                    line(
                        "Net",
                        &name,
                        Ok(format!("{desc}; {}; link {} Mbps; IPv4 {}", if a.OperStatus == 1 { "up" } else { "down" }, a.TransmitLinkSpeed / 1_000_000, if has_v4 { "present (hidden)" } else { "none" })),
                    );
                }
                p = a.Next;
            }
        }
    }

    fn disk_specs(&self) {
        for n in 0..16 {
            let Some(h) = open_phys(n) else { continue };
            unsafe {
                let desc = query_prop(h, StorageDeviceProperty, &[]);
                let (mut model, mut bus) = (String::from("?"), 0u32);
                if let Some(d) = &desc {
                    let off = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
                    let cstr = |o: usize| if o == 0 { String::new() } else { String::from_utf8_lossy(&d[o..d[o..].iter().position(|b| *b == 0).map(|e| o + e).unwrap_or(d.len())]).trim().to_string() };
                    model = format!("{} {}", cstr(off(12)), cstr(off(16))).trim().to_string(); // vendor id + product id (serial offset 24 is never read)
                    bus = off(28) as u32;
                }
                // IOCTL_DISK_GET_LENGTH_INFO needs read access; GET_DRIVE_GEOMETRY_EX is FILE_ANY_ACCESS. DiskSize is at byte 24.
                let mut geo = [0u8; 64];
                let mut ret = 0u32;
                DeviceIoControl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, std::ptr::null(), 0, geo.as_mut_ptr().cast(), 64, &mut ret, std::ptr::null_mut());
                let li = GET_LENGTH_INFORMATION { Length: i64::from_le_bytes(geo[24..32].try_into().unwrap()) };
                let bus_s = match bus { 7 => "USB", 11 => "SATA", 17 => "NVMe", 3 => "ATA", 8 => "RAID", _ => "other" };
                let ssd = query_prop(h, StorageDeviceSeekPenaltyProperty, &[]).map(|d| d[8] == 0);
                line("Storage", &format!("PhysicalDrive{n}"), Ok(format!("{model}; {} GB; {bus_s}; {}", li.Length / 1_000_000_000, match ssd { Some(true) => "no seek penalty (SSD)", Some(false) => "seek penalty (HDD)", None => "media unknown" })));
                self.nvme_health(h, n);
                CloseHandle(h);
            }
        }
        match cimv2("Win32_LogicalDisk", &["DeviceID", "FileSystem", "Size", "FreeSpace"], "WHERE DriveType=3") {
            Ok(r) => r.iter().for_each(|x| {
                let (s, f): (u64, u64) = (x[2].parse().unwrap_or(0), x[3].parse().unwrap_or(0));
                line("Storage", &format!("volume {}", x[0]), Ok(format!("{}, {} GB free of {} GB", x[1], f >> 30, s >> 30)))
            }),
            Err(e) => line("Storage", "Win32_LogicalDisk", Err(e)),
        }
    }

    /// Temperature descriptor first, SMART/Health log as the richer fallback. Both are read-only queries on a
    /// handle opened with zero access rights, which does not need elevation.
    fn nvme_health(&self, h: HANDLE, n: u32) {
        unsafe {
            match nvme_temp(h) {
                Some(t) => line("Storage", &format!("drive{n} temperature (ioctl 52)"), Ok(format!("{t} C"))),
                None => line("Storage", &format!("drive{n} temperature (ioctl 52)"), Err("not reported".into())),
            }
            match nvme_smart(h) {
                Some(s) => line("Storage", &format!("drive{n} NVMe SMART log"), Ok(s)),
                None => line("Storage", &format!("drive{n} NVMe SMART log"), Err("not an NVMe drive or access denied".into())),
            }
        }
    }

    // ------------------------------------------------------------ samples
    pub fn sample(&mut self) {
        self.pdh.collect();
        // CPU load
        let load = self.pdh.array("cpu_load");
        let perf = self.pdh.array("cpu_perf");
        if let Some((_, t)) = load.iter().find(|(n, _)| n.ends_with("_Total") && n.starts_with("_")) {
            let per: Vec<String> = load.iter().filter(|(n, _)| !n.contains("_Total")).map(|(_, v)| format!("{:.0}", v)).collect();
            line("CPU", "load total / per logical", Ok(format!("{t:.1}%  [{}]", per.join(" "))));
        }
        if let Some((_, p)) = perf.iter().find(|(n, _)| n == "_Total") {
            line("CPU", "clock (%perf x base)", Ok(format!("{:.0} MHz  (perf {:.0}% of {} MHz base)", p / 100.0 * self.max_mhz as f64, p, self.max_mhz)));
        }
        // RAM
        unsafe {
            let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..std::mem::zeroed() };
            GlobalMemoryStatusEx(&mut m);
            let mut pi: PERFORMANCE_INFORMATION = std::mem::zeroed();
            pi.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
            K32GetPerformanceInfo(&mut pi, pi.cb);
            line(
                "RAM",
                "used / total / available / cached",
                Ok(format!(
                    "{:.1} / {:.1} / {:.1} / {:.1} GB ({}%)",
                    (m.ullTotalPhys - m.ullAvailPhys) as f64 / 1e9 * 1.073741824,
                    m.ullTotalPhys as f64 / 1e9 * 1.073741824,
                    m.ullAvailPhys as f64 / 1e9 * 1.073741824,
                    (pi.SystemCache * pi.PageSize) as f64 / 1e9 * 1.073741824,
                    m.dwMemoryLoad
                )),
            );
        }
        // Disks
        let (busy, rd, wr) = (self.pdh.array("disk_busy"), self.pdh.array("disk_r"), self.pdh.array("disk_w"));
        for (name, b) in busy.iter().filter(|(n, _)| n != "_Total") {
            let get = |v: &Vec<(String, f64)>| v.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            line("Storage", &format!("disk '{name}' busy/read/write"), Ok(format!("{:.0}%  {:.1} MB/s  {:.1} MB/s", b.min(100.0), get(&rd) / 1e6, get(&wr) / 1e6)));
        }
        for n in 0..16 {
            if let Some(h) = open_phys(n) {
                unsafe {
                    // SMART composite first (works on every NVMe tested), ioctl 52 as the fallback.
                    let t = nvme_smart_temp(h).or_else(|| nvme_temp(h).map(|t| t as i32));
                    if let Some(t) = t {
                        line("Storage", &format!("drive{n} temperature"), Ok(format!("{t} C")));
                    }
                    CloseHandle(h);
                }
            }
        }
        // Network: only adapters with traffic or bandwidth are listed
        let (rx, tx, bw) = (self.pdh.array("net_rx"), self.pdh.array("net_tx"), self.pdh.array("net_bw"));
        for (name, r) in &rx {
            let t = tx.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            let b = bw.iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or(0.0);
            if b > 0.0 && !name.to_lowercase().contains("loopback") {
                line("Net", &name.chars().take(34).collect::<String>(), Ok(format!("down {:.1} KB/s  up {:.1} KB/s  ({:.0} Mbps link)", r / 1e3, t / 1e3, b / 1e6)));
            }
        }
        // ACPI thermal zone (no admin, usually motherboard zone, not the CPU)
        for (name, v) in self.pdh.array("tz") {
            line("CPU", &format!("ACPI thermal zone {}", name.rsplit('.').next().unwrap_or(&name)), Ok(format!("{:.1} C", v / 10.0 - 273.15)));
        }
        // Vendor-neutral GPU load and VRAM from PDH (what Task Manager shows). Needed for AMD/Intel on Windows.
        // Engine instance names contain a LUID and process ids; only the engine type is used, nothing is printed from them.
        let eng = self.pdh.array("gpu_eng");
        let sum = |ty: &str| eng.iter().filter(|(n, _)| n.ends_with(ty)).map(|x| x.1).sum::<f64>().min(100.0);
        line(
            "GPU",
            "PDH load (3D / Copy / VideoDecode)",
            if eng.is_empty() { Err("no GPU Engine counters".into()) } else { Ok(format!("{:.0}% / {:.0}% / {:.0}%", sum("engtype_3D"), sum("engtype_Copy"), sum("engtype_VideoDecode"))) },
        );
        let vram: f64 = self.pdh.array("gpu_mem").iter().map(|x| x.1).sum();
        line("GPU", "PDH VRAM dedicated usage", Ok(format!("{:.0} MiB", vram / 1048576.0)));
        self.cpu_msr();
    }

    fn cpu_msr(&mut self) {
        let why = match (&self.pawn, self.module_loaded) {
            (Err(e), _) => e.clone(),
            (Ok(_), false) => "driver open, but no IntelMSR.bin module loaded (pass --pawn-modules <dir>)".into(),
            _ => String::new(),
        };
        if !why.is_empty() {
            line("CPU", "package / hotspot temperature", Err(format!("needs PawnIO + elevation: {why}")));
            line("CPU", "package power (RAPL)", Err("needs PawnIO + elevation".into()));
            return;
        }
        let p = self.pawn.as_ref().unwrap();
        match pawnio::intel_temps(p, self.logical) {
            Ok(t) => {
                let max = t.cores.iter().flatten().max();
                line("CPU", "TjMax", Ok(format!("{} C", t.tjmax)));
                line("CPU", "package temperature (MSR 0x1B1)", t.package.map(|v| format!("{v} C")).ok_or("invalid reading".into()));
                line("CPU", "hotspot = hottest core (MSR 0x19C)", max.map(|v| format!("{v} C")).ok_or("invalid reading".into()));
                line("CPU", "per logical CPU", Ok(t.cores.iter().map(|c| c.map(|v| v.to_string()).unwrap_or("-".into())).collect::<Vec<_>>().join(" ")));
            }
            Err(e) => line("CPU", "temperatures", Err(e)),
        }
        if let Ok(j) = pawnio::rapl_package_joules(p) {
            let now = std::time::Instant::now();
            if let Some((j0, t0)) = self.last_energy {
                let dt = now.duration_since(t0).as_secs_f64();
                let dj = if j >= j0 { j - j0 } else { j + (u32::MAX as f64 + 1.0) / 1.0 - j0 }; // approximate wrap, fine for a probe
                line("CPU", "package power (RAPL)", Ok(format!("{:.1} W", dj / dt)));
            }
            self.last_energy = Some((j, now));
        }
    }
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

fn open_phys(n: u32) -> Option<HANDLE> {
    let path = wide(&format!(r"\\.\PhysicalDrive{n}"));
    // Access 0: enough for query ioctls, so no elevation is needed.
    let h = unsafe { CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE { None } else { Some(h) }
}

/// IOCTL_STORAGE_QUERY_PROPERTY with PropertyStandardQuery; `extra` is the AdditionalParameters blob.
unsafe fn query_prop(h: HANDLE, id: i32, extra: &[u8]) -> Option<Vec<u8>> {
    let mut q = vec![0u8; 12.max(8 + extra.len())]; // sizeof(STORAGE_PROPERTY_QUERY) = 12
    q[0..4].copy_from_slice(&(id as u32).to_le_bytes());
    q[8..8 + extra.len()].copy_from_slice(extra);
    let mut out = vec![0u8; 4096];
    let mut ret = 0u32;
    let ok = DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, q.as_ptr().cast(), q.len() as u32, out.as_mut_ptr().cast(), out.len() as u32, &mut ret, std::ptr::null_mut());
    if ok == 0 { None } else { out.truncate(ret as usize); Some(out) }
}

/// StorageDeviceTemperatureProperty (52). Layout: Version, Size, Critical i16, Warning i16, InfoCount u16,
/// reserved(2), reserved(8), then TemperatureInfo[] {Index u16, Temperature i16, ...}. First entry at byte 24.
unsafe fn nvme_temp(h: HANDLE) -> Option<i16> {
    let d = query_prop(h, StorageDeviceTemperatureProperty, &[])?;
    if d.len() < 28 || u16::from_le_bytes([d[12], d[13]]) == 0 {
        return None;
    }
    let t = i16::from_le_bytes([d[26], d[27]]);
    if (0..150).contains(&t) { Some(t) } else { None }
}

/// NVMe Health/SMART log page (id 2) through StorageDeviceProtocolSpecificProperty (50).
/// Request: STORAGE_PROTOCOL_SPECIFIC_DATA (40 bytes): ProtocolType=3 (NVMe), DataType=2 (log page),
/// RequestValue=2 (health), SubValue=0, DataOffset=40, DataLength=512, then 4 reserved/sub-value dwords.
/// The log is returned after the 8-byte descriptor header plus the same 40-byte struct (offset 48).
unsafe fn nvme_smart_temp(h: HANDLE) -> Option<i32> {
    nvme_log(h).map(|l| u16::from_le_bytes([l[1], l[2]]) as i32 - 273)
}

unsafe fn nvme_log(h: HANDLE) -> Option<Vec<u8>> {
    let mut extra = Vec::new();
    for v in [3u32, 2, 2, 0, 40, 512, 0, 0, 0, 0] {
        extra.extend_from_slice(&v.to_le_bytes());
    }
    let d = query_prop(h, StorageDeviceProtocolSpecificProperty, &extra)?;
    if d.len() < 48 + 512 {
        return None;
    }
    Some(d[48..48 + 512].to_vec())
}

unsafe fn nvme_smart(h: HANDLE) -> Option<String> {
    let l = nvme_log(h)?;
    let u128_at = |o: usize| u128::from_le_bytes(l[o..o + 16].try_into().unwrap());
    let comp = u16::from_le_bytes([l[1], l[2]]) as i32 - 273;
    let written_tb = u128_at(48) as f64 * 512_000.0 / 1e12;
    Some(format!(
        "composite {comp} C, spare {}%, used {}%, critical warning {:#x}, power-on {} h, written {:.1} TB, media errors {}",
        l[3], l[5], l[0], u128_at(128), written_tb, u128_at(160)
    ))
}

/// Maintained table: CPUID (vendor, family, model) -> codename and socket. Extend as hardware appears.
fn codename(vendor: &str, family: u32, model: u32) -> (&'static str, &'static str) {
    match (vendor, family, model) {
        ("GenuineIntel", 6, 0x97) => ("Alder Lake-S", "LGA1700"),
        ("GenuineIntel", 6, 0x9A) => ("Alder Lake-P/H", "BGA1744"),
        ("GenuineIntel", 6, 0xB7) | ("GenuineIntel", 6, 0xBF) => ("Raptor Lake-S", "LGA1700"),
        ("GenuineIntel", 6, 0xBA) | ("GenuineIntel", 6, 0xBE) => ("Raptor Lake-P", "BGA1700"),
        ("GenuineIntel", 6, 0xC6) => ("Arrow Lake-S", "LGA1851"),
        ("GenuineIntel", 6, 0xA7) => ("Rocket Lake-S", "LGA1200"),
        ("GenuineIntel", 6, 0xA5) => ("Comet Lake-S", "LGA1200"),
        ("AuthenticAMD", 0x19, 0x21) | ("AuthenticAMD", 0x19, 0x61) => ("Zen 3 / Zen 4", "AM4 / AM5"),
        ("AuthenticAMD", 0x1A, _) => ("Zen 5", "AM5"),
        _ => ("unknown (not in table)", "unknown"),
    }
}

/// Base power from the marketing name. Fallback only; with the driver MSR_PKG_POWER_INFO is exact.
fn tdp(brand: &str) -> Option<u32> {
    let b = brand.to_uppercase();
    if b.contains("12400F") || b.contains("12400") { Some(65) } else { None }
}

/// Intel PCH LPC/eSPI bridge ids. Only ids confirmed on real hardware or on pci-ids are listed; extend from
/// https://pci-ids.ucw.cz when new boards show up. 7A86 verified on the dev PC (ASUS PRIME B660-PLUS D4).
fn chipset_name(dev: &str) -> Option<&'static str> {
    Some(match dev.to_uppercase().as_str() {
        "7A86" => "Intel B660",
        _ => return None,
    })
}
