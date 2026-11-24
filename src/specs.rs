//! The "System specs" data: components, their fields and the parsers that turn raw OS output into them.
//! Collection itself lives in `sensors::win` and `sensors::linux`; everything that can be tested without the OS
//! (parsers, name tables, the copy-to-clipboard text) is here.
//!
//! Privacy: no serial number, UUID, MAC address or IP address is ever a field. The collectors never request them.

use crate::i18n::Lang;

pub const DASH: &str = "—";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Cpu,
    Gpu,
    Board,
    Ram,
    Storage,
    Audio,
    Network,
}

impl Group {
    pub const ALL: [Group; 7] = [Group::Cpu, Group::Gpu, Group::Board, Group::Ram, Group::Storage, Group::Audio, Group::Network];
}

/// A label of a detail row. The words live in `strings.rs`, so every field is translated in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Manufacturer,
    Model,
    Codename,
    Socket,
    Cores,
    Threads,
    BaseClock,
    L1,
    L2,
    L3,
    Tdp,
    Vram,
    Driver,
    MaxCoreClock,
    MaxMemClock,
    PowerLimit,
    PcieLink,
    Chipset,
    BiosVendor,
    BiosVersion,
    BiosDate,
    PartNumber,
    Capacity,
    MemType,
    RatedSpeed,
    ConfiguredSpeed,
    Voltage,
    Slot,
    Timings,
    Bus,
    Media,
    Health,
    Temperature,
    Written,
    PowerOn,
    Volumes,
    Status,
    LinkSpeed,
    Address,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub group: Group,
    /// The exact model name, shown in the list.
    pub title: String,
    /// One short line under it (capacity, speed, and so on).
    pub sub: String,
    pub rows: Vec<(Field, String)>,
}

impl Component {
    pub fn new(group: Group, title: impl Into<String>) -> Self {
        Self { group, title: title.into(), sub: String::new(), rows: Vec::new() }
    }
    pub fn sub(mut self, s: impl Into<String>) -> Self {
        self.sub = s.into();
        self
    }
    /// Adds a row; an empty value becomes "—".
    pub fn row(mut self, f: Field, v: impl Into<String>) -> Self {
        let v = v.into();
        self.rows.push((f, if v.trim().is_empty() { DASH.to_string() } else { v }));
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Specs {
    pub comps: Vec<Component>,
}

impl Specs {
    pub fn of(&self, g: Group) -> impl Iterator<Item = (usize, &Component)> {
        self.comps.iter().enumerate().filter(move |(_, c)| c.group == g)
    }
}

/// The plain-text summary behind "Copy specs": group, model, then the fields. Labels come from the caller so the
/// text is in the interface language. The local address is left out of it, so the text is safe to paste anywhere.
pub fn copy_text(specs: &Specs, lang: Lang) -> String {
    let t = crate::strings::TABLE.get(lang);
    let mut out = String::new();
    for g in Group::ALL {
        let items: Vec<_> = specs.of(g).collect();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("{}\n", t.group_name(g)));
        for (_, c) in items {
            out.push_str(&format!("  {}\n", c.title));
            for (f, v) in c.rows.iter().filter(|(f, _)| *f != Field::Address) {
                out.push_str(&format!("    {}: {}\n", t.field_name(*f), v));
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

// ───────────────────────── name tables (maintained by hand) ─────────────────────────

/// CPUID (vendor, family, model) to codename and socket. Extend it when new hardware shows up.
pub fn codename(vendor: &str, family: u32, model: u32) -> Option<(&'static str, &'static str)> {
    Some(match (vendor, family, model) {
        ("GenuineIntel", 6, 0x97) => ("Alder Lake-S", "LGA1700"),
        ("GenuineIntel", 6, 0x9A) => ("Alder Lake-P/H", "BGA1744"),
        ("GenuineIntel", 6, 0xB7) | ("GenuineIntel", 6, 0xBF) => ("Raptor Lake-S", "LGA1700"),
        ("GenuineIntel", 6, 0xBA) | ("GenuineIntel", 6, 0xBE) => ("Raptor Lake-P", "BGA1700"),
        ("GenuineIntel", 6, 0xC6) => ("Arrow Lake-S", "LGA1851"),
        ("GenuineIntel", 6, 0xA7) => ("Rocket Lake-S", "LGA1200"),
        ("GenuineIntel", 6, 0xA5) => ("Comet Lake-S", "LGA1200"),
        ("AuthenticAMD", 0x19, 0x21) | ("AuthenticAMD", 0x19, 0x50) => ("Zen 3 (Vermeer / Cezanne)", "AM4"),
        ("AuthenticAMD", 0x19, 0x61) => ("Zen 4 (Raphael)", "AM5"),
        ("AuthenticAMD", 0x1A, _) => ("Zen 5", "AM5"),
        _ => return None,
    })
}

/// Base power from the marketing name, only for the parts we know. The exact value needs the driver.
pub fn tdp_watts(brand: &str) -> Option<u32> {
    let b = brand.to_uppercase();
    [("12400F", 65), ("12400", 65), ("12600K", 125), ("12700K", 125), ("13600K", 125), ("14600K", 125), ("5600X", 65), ("5800X3D", 105), ("7800X3D", 120), ("7600X", 105)]
        .iter()
        .find(|(m, _)| b.contains(m))
        .map(|(_, w)| *w)
}

/// Intel PCH LPC/eSPI bridge PCI id to chipset name. `7A86` was verified on a real ASUS PRIME B660-PLUS D4;
/// add more from https://pci-ids.ucw.cz when they are confirmed.
pub fn chipset_name(dev_id: &str) -> Option<&'static str> {
    Some(match dev_id.to_uppercase().as_str() {
        "7A86" => "Intel B660",
        _ => return None,
    })
}

/// Pulls the 4-hex-digit device id out of a PnP id like `PCI\VEN_8086&DEV_7A86&SUBSYS_...`.
pub fn pci_dev_id(pnp: &str) -> Option<String> {
    let rest = pnp.split("DEV_").nth(1)?;
    let id = rest.get(..4)?;
    id.chars().all(|c| c.is_ascii_hexdigit()).then(|| id.to_string())
}

// ───────────────────────── parsers ─────────────────────────

/// DDR generation from the SMBIOS memory type code (type 17, offset 0x12; WMI `SMBIOSMemoryType`).
pub fn memory_type_name(code: u32) -> &'static str {
    match code {
        0x18 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        _ => "",
    }
}

/// One `Win32_PhysicalMemory` row in this order: Manufacturer, PartNumber, Capacity, Speed, ConfiguredClockSpeed,
/// SMBIOSMemoryType, DeviceLocator, ConfiguredVoltage. The serial number is never part of the query.
pub fn memory_module(row: &[String]) -> Component {
    let g = |i: usize| row.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    let cap: u64 = g(2).parse().unwrap_or(0);
    let ty = memory_type_name(g(5).parse().unwrap_or(0));
    let speed = g(3).parse::<u32>().ok().filter(|s| *s > 0);
    let conf = g(4).parse::<u32>().ok().filter(|s| *s > 0);
    let mv = g(7).parse::<u32>().ok().filter(|v| *v > 0);
    let part = g(1);
    let title = if part.is_empty() { format!("{} {}", g(0), crate::model::fmt_bytes(cap)).trim().to_string() } else { part.clone() };
    let mut sub = format!("{} GB", cap >> 30);
    if !ty.is_empty() {
        sub = format!("{sub} {ty}");
    }
    if let Some(c) = conf.or(speed) {
        sub = format!("{sub}-{c}");
    }
    Component::new(Group::Ram, title)
        .sub(sub)
        .row(Field::Manufacturer, g(0))
        .row(Field::PartNumber, part)
        .row(Field::Capacity, format!("{} GB", cap >> 30))
        .row(Field::MemType, ty)
        .row(Field::RatedSpeed, speed.map(|s| format!("{s} MT/s")).unwrap_or_default())
        .row(Field::ConfiguredSpeed, conf.map(|s| format!("{s} MT/s")).unwrap_or_default())
        .row(Field::Voltage, mv.map(|v| format!("{:.2} V", v as f64 / 1000.0)).unwrap_or_default())
        .row(Field::Slot, g(6))
        .row(Field::Timings, DASH)
}

/// The NVMe Health/SMART log page (512 bytes), only the figures we show.
#[derive(Clone, Debug, PartialEq)]
pub struct NvmeHealth {
    pub temp_c: i32,
    pub spare_pct: u8,
    pub used_pct: u8,
    pub critical_warning: u8,
    pub power_on_hours: u64,
    pub written_tb: f64,
    pub media_errors: u64,
}

pub fn parse_nvme_health(l: &[u8]) -> Option<NvmeHealth> {
    if l.len() < 176 {
        return None;
    }
    let u128_at = |o: usize| u128::from_le_bytes(l[o..o + 16].try_into().unwrap());
    let kelvin = u16::from_le_bytes([l[1], l[2]]) as i32;
    if kelvin == 0 {
        return None;
    }
    Some(NvmeHealth {
        temp_c: kelvin - 273,
        spare_pct: l[3],
        used_pct: l[5],
        critical_warning: l[0],
        power_on_hours: u128_at(128).min(u64::MAX as u128) as u64,
        // Data Units Written: thousands of 512-byte units.
        written_tb: u128_at(48) as f64 * 512_000.0 / 1e12,
        media_errors: u128_at(160).min(u64::MAX as u128) as u64,
    })
}

impl NvmeHealth {
    pub fn summary(&self) -> String {
        if self.critical_warning != 0 {
            format!("Warning {:#x}", self.critical_warning)
        } else {
            format!("OK, {}% used", self.used_pct)
        }
    }
}

/// STORAGE_DEVICE_DESCRIPTOR: (vendor + product, bus type code). The serial-number offset is never followed.
pub fn parse_storage_descriptor(d: &[u8]) -> Option<(String, u32)> {
    if d.len() < 32 {
        return None;
    }
    let off = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    let cstr = |o: usize| -> String {
        if o == 0 || o >= d.len() {
            return String::new();
        }
        let end = d[o..].iter().position(|b| *b == 0).map(|e| o + e).unwrap_or(d.len());
        String::from_utf8_lossy(&d[o..end]).trim().to_string()
    };
    Some((format!("{} {}", cstr(off(12)), cstr(off(16))).trim().to_string(), off(28) as u32))
}

pub fn bus_name(code: u32) -> &'static str {
    match code {
        7 => "USB",
        11 => "SATA",
        17 => "NVMe",
        3 => "ATA",
        8 => "RAID",
        _ => "",
    }
}

/// Windows audio endpoint names look like `Microphone (fifine SC3)`. The device is the part in the last brackets.
pub fn audio_device(endpoint: &str) -> String {
    match (endpoint.rfind('('), endpoint.ends_with(')')) {
        (Some(i), true) if i + 1 < endpoint.len() - 1 => endpoint[i + 1..endpoint.len() - 1].trim().to_string(),
        _ => endpoint.trim().to_string(),
    }
}

/// Groups endpoints by device: ("fifine SC3", ["Microphone", "Speakers"]) in the order first seen.
pub fn group_audio(endpoints: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for e in endpoints {
        let dev = audio_device(e);
        let kind = e.rfind('(').filter(|_| e.ends_with(')')).map(|i| e[..i].trim().to_string()).unwrap_or_default();
        match out.iter_mut().find(|(d, _)| *d == dev) {
            Some((_, kinds)) => {
                if !kind.is_empty() && !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            None => out.push((dev, if kind.is_empty() { vec![] } else { vec![kind] })),
        }
    }
    out
}

/// Adapters that are worth listing: up, not loopback or tunnel, and not one of the well-known virtual ones.
pub fn is_real_adapter(desc: &str, if_type: u32, up: bool) -> bool {
    if !up || if_type == 24 || if_type == 131 {
        return false;
    }
    let d = desc.to_lowercase();
    !["virtual", "vethernet", "loopback", "bluetooth", "vmware", "virtualbox", "pseudo", "wan miniport", "tap-", "wintun", "wireguard", "hyper-v"].iter().any(|w| d.contains(w))
}

// ───────────────────────── SMBIOS (Linux, root) ─────────────────────────

/// Memory devices (SMBIOS type 17) from a raw table, e.g. `/sys/firmware/dmi/tables/DMI` (root only).
/// Offsets are from the DMTF SMBIOS specification. The serial-number string is not read.
#[cfg_attr(windows, allow(dead_code))]
pub fn parse_smbios_memory(table: &[u8]) -> Vec<Component> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= table.len() {
        let (ty, len) = (table[i], table[i + 1] as usize);
        if len < 4 || i + len > table.len() {
            break;
        }
        let f = &table[i..i + len];
        // Strings follow the formatted area, each NUL-terminated, the set ends with a double NUL.
        let mut j = i + len;
        let mut strings: Vec<String> = Vec::new();
        if j + 1 < table.len() && table[j] == 0 && table[j + 1] == 0 {
            j += 2; // no strings
        } else {
            loop {
                if j >= table.len() {
                    break;
                }
                if table[j] == 0 {
                    j += 1; // the terminating NUL
                    break;
                }
                let start = j;
                while j < table.len() && table[j] != 0 {
                    j += 1;
                }
                strings.push(String::from_utf8_lossy(&table[start..j]).trim().to_string());
                j += 1;
            }
        }
        if ty == 127 {
            break;
        }
        if ty == 17 && len >= 0x1B {
            let s = |idx: u8| if idx == 0 { String::new() } else { strings.get(idx as usize - 1).cloned().unwrap_or_default() };
            let size_raw = u16::from_le_bytes([f[0x0C], f[0x0D]]);
            let size_mb: u64 = match size_raw {
                0 | 0xFFFF => 0,
                0x7FFF if len >= 0x20 => u32::from_le_bytes(f[0x1C..0x20].try_into().unwrap()) as u64,
                v if v & 0x8000 != 0 => (v & 0x7FFF) as u64 / 1024,
                v => v as u64,
            };
            if size_mb > 0 {
                let speed = u16::from_le_bytes([f[0x15], f[0x16]]) as u32;
                let conf = if len >= 0x22 { u16::from_le_bytes([f[0x20], f[0x21]]) as u32 } else { 0 };
                let mv = if len >= 0x28 { u16::from_le_bytes([f[0x26], f[0x27]]) as u32 } else { 0 };
                let row = vec![s(f[0x17]), s(f[0x1A]), (size_mb * 1024 * 1024).to_string(), speed.to_string(), conf.to_string(), f[0x12].to_string(), s(f[0x10]), mv.to_string()];
                out.push(memory_module(&row));
            }
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_wmi_memory_row_becomes_a_module() {
        // Captured shape of Win32_PhysicalMemory (values of the dev PC, no serial).
        let c = memory_module(&row(&["Kingston", "KF3600C18D4/16GX", "17179869184", "3600", "3600", "26", "Controller0-ChannelA-DIMM1", "1200"]));
        assert_eq!(c.title, "KF3600C18D4/16GX");
        assert_eq!(c.sub, "16 GB DDR4-3600");
        let get = |f| c.rows.iter().find(|r| r.0 == f).unwrap().1.clone();
        assert_eq!(get(Field::Voltage), "1.20 V");
        assert_eq!(get(Field::RatedSpeed), "3600 MT/s");
        assert_eq!(get(Field::Timings), DASH, "timings need the SPD driver: shown as a dash, never invented");
        assert_eq!(get(Field::Slot), "Controller0-ChannelA-DIMM1");
    }

    #[test]
    fn missing_memory_values_are_dashes() {
        let c = memory_module(&row(&["", "", "8589934592", "0", "0", "0", "", "0"]));
        assert_eq!(c.rows.iter().find(|r| r.0 == Field::RatedSpeed).unwrap().1, DASH);
        assert_eq!(c.rows.iter().find(|r| r.0 == Field::MemType).unwrap().1, DASH);
    }

    #[test]
    fn nvme_health_log_is_decoded() {
        let mut l = vec![0u8; 512];
        l[1..3].copy_from_slice(&316u16.to_le_bytes()); // 43 C
        l[3] = 100;
        l[5] = 2;
        l[48..52].copy_from_slice(&37_300_000u32.to_le_bytes());
        l[128..132].copy_from_slice(&7070u32.to_le_bytes());
        let h = parse_nvme_health(&l).unwrap();
        assert_eq!(h.temp_c, 43);
        assert_eq!((h.spare_pct, h.used_pct), (100, 2));
        assert_eq!(h.power_on_hours, 7070);
        assert!((h.written_tb - 19.1).abs() < 0.1);
        assert_eq!(h.summary(), "OK, 2% used");
        assert!(parse_nvme_health(&[0u8; 10]).is_none());
        assert!(parse_nvme_health(&vec![0u8; 512]).is_none(), "0 K is not a temperature");
    }

    #[test]
    fn storage_descriptor_gives_model_and_bus_without_the_serial() {
        let mut d = vec![0u8; 128];
        d[12..16].copy_from_slice(&40u32.to_le_bytes()); // vendor id offset
        d[16..20].copy_from_slice(&56u32.to_le_bytes()); // product id offset
        d[24..28].copy_from_slice(&80u32.to_le_bytes()); // serial offset: must stay unread
        d[28..32].copy_from_slice(&17u32.to_le_bytes());
        d[40..48].copy_from_slice(b"KINGSTON");
        d[56..72].copy_from_slice(b"SKC3000S1024G\0\0\0");
        d[80..88].copy_from_slice(b"SECRET01");
        let (model, bus) = parse_storage_descriptor(&d).unwrap();
        assert_eq!(model, "KINGSTON SKC3000S1024G");
        assert_eq!(bus_name(bus), "NVMe");
        assert!(!model.contains("SECRET"));
    }

    #[test]
    fn audio_endpoints_are_grouped_by_device() {
        let e = row(&["Microphone (fifine SC3)", "Speakers (fifine SC3)", "DELL UZ2215H (NVIDIA High Definition Audio)", "Digital Audio Interface (C1-1 USB3 Audio)"]);
        let g = group_audio(&e);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].0, "fifine SC3");
        assert_eq!(g[0].1, vec!["Microphone", "Speakers"]);
        assert_eq!(g[1].0, "NVIDIA High Definition Audio");
        assert_eq!(audio_device("Plain name"), "Plain name");
    }

    #[test]
    fn adapters_hide_virtual_and_down_ones() {
        assert!(is_real_adapter("Realtek Gaming 2.5GbE Family Controller", 6, true));
        assert!(!is_real_adapter("Realtek Gaming 2.5GbE Family Controller", 6, false));
        assert!(!is_real_adapter("Hyper-V Virtual Ethernet Adapter", 6, true));
        assert!(!is_real_adapter("Bluetooth Device (Personal Area Network)", 6, true));
        assert!(!is_real_adapter("Software Loopback Interface 1", 24, true));
    }

    #[test]
    fn pci_ids_and_chipsets() {
        assert_eq!(pci_dev_id(r"PCI\VEN_8086&DEV_7A86&SUBSYS_86941043&REV_11\3&1&0&F8").as_deref(), Some("7A86"));
        assert_eq!(pci_dev_id("USB\\VID_1&PID_2"), None);
        assert_eq!(chipset_name("7a86"), Some("Intel B660"));
        assert_eq!(chipset_name("FFFF"), None);
    }

    #[test]
    fn cpu_tables() {
        assert_eq!(codename("GenuineIntel", 6, 0x97), Some(("Alder Lake-S", "LGA1700")));
        assert_eq!(codename("GenuineIntel", 6, 0x01), None);
        assert_eq!(tdp_watts("12th Gen Intel(R) Core(TM) i5-12400F"), Some(65));
        assert_eq!(tdp_watts("Unknown CPU"), None);
    }

    fn smbios_type17(size_mb: u16, speed: u16, conf: u16, mfr: &str, part: &str, loc: &str) -> Vec<u8> {
        let mut f = vec![0u8; 0x28];
        f[0] = 17;
        f[1] = 0x28;
        f[0x0C..0x0E].copy_from_slice(&size_mb.to_le_bytes());
        f[0x10] = 1; // device locator = string 1
        f[0x12] = 0x1A; // DDR4
        f[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        f[0x17] = 2; // manufacturer
        f[0x18] = 4; // serial string index: present but must not be read
        f[0x1A] = 3; // part number
        f[0x20..0x22].copy_from_slice(&conf.to_le_bytes());
        f[0x26..0x28].copy_from_slice(&1200u16.to_le_bytes());
        for s in [loc, mfr, part, "SERIAL123"] {
            f.extend_from_slice(s.as_bytes());
            f.push(0);
        }
        f.push(0);
        f
    }

    #[test]
    fn smbios_type_17_is_parsed_and_the_serial_is_not_read() {
        let mut t = smbios_type17(16384, 3600, 3600, "Kingston", "KF3600C18D4/16GX", "DIMM_A1");
        t.extend(smbios_type17(0, 0, 0, "", "", "DIMM_B1")); // an empty slot
        t.extend([127, 4, 0, 0, 0, 0]); // end of table
        let m = parse_smbios_memory(&t);
        assert_eq!(m.len(), 1, "empty slot is skipped");
        assert_eq!(m[0].title, "KF3600C18D4/16GX");
        assert_eq!(m[0].sub, "16 GB DDR4-3600");
        assert!(m[0].rows.iter().all(|(_, v)| !v.contains("SERIAL")));
    }

    #[test]
    fn copy_text_has_groups_and_no_private_fields() {
        let s = Specs { comps: vec![Component::new(Group::Cpu, "Core i5").row(Field::Cores, "6")] };
        let t = copy_text(&s, Lang::En);
        assert!(t.contains("Core i5") && t.contains("Cores: 6"), "{t}");
        let low = t.to_lowercase();
        assert!(!low.contains("serial") && !low.contains("mac "));
    }
}
