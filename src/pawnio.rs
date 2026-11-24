//! Minimal PawnIO client over its documented device interface (no PawnIOLib.dll needed).
//!
//! The driver is NOT installed by this prototype. This module only runs when the user already has it.
//! Device and IOCTL numbers come from PawnIO `PawnIO/include/pawnio_um.h` (device type 41394, buffered,
//! FILE_ANY_ACCESS). Execute input = 32-byte zero-padded function name followed by u64 arguments;
//! output = u64 results. Same protocol LibreHardwareMonitor uses (`PawnIo.cs`).
//! The device ACL is `D:P(A;;GA;;;SY)(A;;GA;;;BA)` (PawnIO.inf.in), so opening it needs an elevated token.
//! Our code talks to the driver only through the device IO control interface, which is the case the
//! PawnIO GPL-2.0 licence exception explicitly allows; modules are LGPL-2.1.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

const fn ctl(function: u32) -> u32 {
    (41394 << 16) | (function << 2) // METHOD_BUFFERED (0), FILE_ANY_ACCESS (0)
}
const IOCTL_LOAD: u32 = ctl(0x821);
const IOCTL_EXECUTE: u32 = ctl(0x841);

pub struct Pawn {
    h: HANDLE,
}

impl Drop for Pawn {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.h) };
    }
}

pub fn open() -> Result<Pawn, String> {
    let path: Vec<u16> = r"\\?\GLOBALROOT\Device\PawnIO".encode_utf16().chain([0]).collect();
    let h = unsafe {
        CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        let e = unsafe { GetLastError() };
        return Err(match e {
            5 => "access denied: the process is not elevated (device is SYSTEM+Administrators only)".into(),
            2 | 3 => "PawnIO device not present (driver not installed)".into(),
            _ => format!("CreateFile error {e}"),
        });
    }
    Ok(Pawn { h })
}

impl Pawn {
    /// Load a signed module blob (for example `IntelMSR.bin` from the PawnIO.Modules release).
    pub fn load(&self, blob: &[u8]) -> Result<(), String> {
        let mut ret = 0u32;
        let ok = unsafe { DeviceIoControl(self.h, IOCTL_LOAD, blob.as_ptr().cast(), blob.len() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) };
        if ok == 0 {
            Err(format!("load module failed, error {}", unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    }

    pub fn exec(&self, name: &str, input: &[u64], out_n: usize) -> Result<Vec<u64>, String> {
        let mut buf = vec![0u8; 32 + input.len() * 8];
        buf[..name.len().min(31)].copy_from_slice(&name.as_bytes()[..name.len().min(31)]);
        for (i, v) in input.iter().enumerate() {
            buf[32 + i * 8..40 + i * 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = vec![0u64; out_n];
        let mut ret = 0u32;
        let ok = unsafe { DeviceIoControl(self.h, IOCTL_EXECUTE, buf.as_ptr().cast(), buf.len() as u32, out.as_mut_ptr().cast(), (out_n * 8) as u32, &mut ret, std::ptr::null_mut()) };
        if ok == 0 {
            Err(format!("{name} failed, error {}", unsafe { GetLastError() }))
        } else {
            Ok(out)
        }
    }

    pub fn read_msr(&self, msr: u32) -> Result<u64, String> {
        self.exec("ioctl_read_msr", &[msr as u64], 1).map(|v| v[0])
    }
}

/// Intel CPU temperatures from MSRs. Register meanings: Intel SDM vol. 4.
///  - IA32_TEMPERATURE_TARGET (0x1A2) bits 23:16 = TjMax
///  - IA32_THERM_STATUS (0x19C) bit 31 = reading valid, bits 22:16 = degrees below TjMax (per core, thread affinity needed)
///  - IA32_PACKAGE_THERM_STATUS (0x1B1) same layout, package scope
pub struct IntelTemps {
    pub tjmax: u32,
    pub package: Option<u32>,
    pub cores: Vec<Option<u32>>,
}

pub fn intel_temps(p: &Pawn, logical: usize) -> Result<IntelTemps, String> {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};
    let tjmax = ((p.read_msr(0x1A2)? >> 16) & 0xFF) as u32;
    let tjmax = if tjmax == 0 { 100 } else { tjmax };
    let pkg = p.read_msr(0x1B1).ok().and_then(|v| if v >> 31 & 1 == 1 { Some(tjmax - ((v >> 16) & 0x7F) as u32) } else { None });
    let mut cores = Vec::new();
    // One reading per physical core would need SMT-sibling knowledge; the prototype reads every logical CPU
    // (the two siblings share the sensor, so values repeat) and the app can take the max.
    let old = unsafe { SetThreadAffinityMask(GetCurrentThread(), usize::MAX) };
    for i in 0..logical.min(64) {
        unsafe { SetThreadAffinityMask(GetCurrentThread(), 1usize << i) };
        let v = p.read_msr(0x19C).ok();
        cores.push(v.and_then(|v| if v >> 31 & 1 == 1 { Some(tjmax - ((v >> 16) & 0x7F) as u32) } else { None }));
    }
    unsafe { SetThreadAffinityMask(GetCurrentThread(), old) };
    Ok(IntelTemps { tjmax, package: pkg, cores })
}

/// RAPL: package energy in joules (counter wraps at 32 bits). Unit = 1/2^ESU joules, ESU = MSR_RAPL_POWER_UNIT bits 12:8.
pub fn rapl_package_joules(p: &Pawn) -> Result<f64, String> {
    let unit = p.read_msr(0x606)?;
    let esu = (unit >> 8) & 0x1F;
    let e = p.read_msr(0x611)? & 0xFFFF_FFFF;
    Ok(e as f64 / (1u64 << esu) as f64)
}
