//! Windows implementation of the helper. See `main.rs`.


use crate::proto::{self, Sample};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_OUTBOUND};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::Threading::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The process leaves when nobody has been connected for this long.
const IDLE_EXIT: Duration = Duration::from_secs(60);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn is_elevated() -> bool {
    // SAFETY: plain Win32 calls; the buffer is a TOKEN_ELEVATION and its size is passed.
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret = 0u32;
        let ok = GetTokenInformation(tok, TokenElevation, (&mut e as *mut TOKEN_ELEVATION).cast(), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut ret);
        CloseHandle(tok);
        ok != 0 && e.TokenIsElevated != 0
    }
}

// ───────────────────────── PawnIO (device interface only, read-only use) ─────────────────────────

const fn ctl(function: u32) -> u32 {
    (41394 << 16) | (function << 2) // METHOD_BUFFERED, FILE_ANY_ACCESS (PawnIO `pawnio_um.h`)
}
const IOCTL_LOAD: u32 = ctl(0x821);
const IOCTL_EXECUTE: u32 = ctl(0x841);

struct Pawn(HANDLE);

impl Drop for Pawn {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by us and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Pawn {
    fn open() -> Result<Pawn, String> {
        let path = wide(r"\\?\GLOBALROOT\Device\PawnIO");
        // SAFETY: NUL-terminated path, no security attributes.
        let h = unsafe { CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            // SAFETY: reads the thread's last error.
            return Err(match unsafe { GetLastError() } {
                2 | 3 => "driver-missing".into(),
                5 => "access-denied".into(),
                e => format!("open-{e}"),
            });
        }
        Ok(Pawn(h))
    }

    fn load(&self, blob: &[u8]) -> Result<(), String> {
        let mut ret = 0u32;
        // SAFETY: input buffer is the module blob, no output.
        let ok = unsafe { DeviceIoControl(self.0, IOCTL_LOAD, blob.as_ptr().cast(), blob.len() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) };
        if ok == 0 { Err("module-rejected".into()) } else { Ok(()) }
    }

    /// The only function ever called: the module's `ioctl_read_msr` (a read; the module whitelists the registers).
    fn read_msr(&self, msr: u32) -> Option<u64> {
        let mut buf = [0u8; 40];
        buf[..14].copy_from_slice(b"ioctl_read_msr");
        buf[32..40].copy_from_slice(&(msr as u64).to_le_bytes());
        let mut out = 0u64;
        let mut ret = 0u32;
        // SAFETY: 40 bytes in (name + one u64), 8 bytes out.
        let ok = unsafe { DeviceIoControl(self.0, IOCTL_EXECUTE, buf.as_ptr().cast(), 40, (&mut out as *mut u64).cast(), 8, &mut ret, std::ptr::null_mut()) };
        (ok != 0).then_some(out)
    }
}

fn is_intel() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let v = std::arch::x86_64::__cpuid(0);
        let mut s = Vec::new();
        for r in [v.ebx, v.edx, v.ecx] {
            s.extend_from_slice(&r.to_le_bytes());
        }
        s == b"GenuineIntel"
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

struct Reader {
    pawn: Pawn,
    tjmax: u32,
    logical: usize,
    esu: u32,
    last_energy: Option<(u64, Instant)>,
}

impl Reader {
    /// Temperatures: Intel SDM vol. 4. TjMax = IA32_TEMPERATURE_TARGET (0x1A2) bits 23:16; a core reads
    /// TjMax - IA32_THERM_STATUS (0x19C) bits 22:16 when bit 31 (valid) is set; the package uses 0x1B1.
    /// Power: MSR_PKG_ENERGY_STATUS (0x611), unit 2^-ESU J with ESU from MSR_RAPL_POWER_UNIT (0x606) bits 12:8.
    fn sample(&mut self) -> Sample {
        let mut s = Sample { helper: VERSION.into(), tjmax: self.tjmax, ..Default::default() };
        let valid = |v: u64, tj: u32| (v >> 31 & 1 == 1).then(|| tj.saturating_sub(((v >> 16) & 0x7F) as u32));
        s.pkg = self.pawn.read_msr(0x1B1).and_then(|v| valid(v, self.tjmax));
        // SAFETY: pins this thread to one logical CPU at a time and restores it afterwards.
        unsafe {
            let th = GetCurrentThread();
            let old = SetThreadAffinityMask(th, usize::MAX);
            for i in 0..self.logical.min(64) {
                SetThreadAffinityMask(th, 1usize << i);
                s.cores.push(self.pawn.read_msr(0x19C).and_then(|v| valid(v, self.tjmax)));
            }
            if old != 0 {
                SetThreadAffinityMask(th, old);
            }
        }
        s.hot = s.cores.iter().flatten().max().copied();
        if let Some(e) = self.pawn.read_msr(0x611) {
            let e = e & 0xFFFF_FFFF;
            let now = Instant::now();
            if let Some((e0, t0)) = self.last_energy {
                let dt = now.duration_since(t0).as_secs_f64();
                let d = if e >= e0 { e - e0 } else { e + (1 << 32) - e0 };
                if dt > 0.2 {
                    s.watts = Some(d as f64 / (1u64 << self.esu) as f64 / dt);
                }
            }
            self.last_energy = Some((e, now));
        }
        if s.pkg.is_none() && s.hot.is_none() {
            s.err = "no-reading".into();
        }
        s
    }
}

fn make_reader() -> Result<Reader, String> {
    if !is_intel() {
        return Err("unsupported-cpu".into());
    }
    let pawn = Pawn::open()?;
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).ok_or("no-exe-dir")?;
    let blob = std::fs::read(dir.join("modules").join("IntelMSR.bin")).map_err(|_| "module-missing".to_string())?;
    pawn.load(&blob)?;
    let tj = pawn.read_msr(0x1A2).map(|v| ((v >> 16) & 0xFF) as u32).filter(|t| *t > 0).unwrap_or(100);
    let esu = pawn.read_msr(0x606).map(|v| ((v >> 8) & 0x1F) as u32).unwrap_or(14);
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    Ok(Reader { pawn, tjmax: tj, logical, esu, last_energy: None })
}

// ───────────────────────── the pipe ─────────────────────────

static LAST_ACTIVE: AtomicU64 = AtomicU64::new(0);
static CLIENTS: AtomicUsize = AtomicUsize::new(0);
static LINE: Mutex<String> = Mutex::new(String::new());
/// Thermora's window and its tray part may both read at once.
const MAX_CLIENTS: u32 = 4;

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn create_pipe(name: &str, sid: &str, first: bool) -> Option<HANDLE> {
    // Only the installing user and SYSTEM; remote clients are refused by the flag below.
    let sddl = wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
    let mut sd = std::ptr::null_mut();
    // SAFETY: SDDL string is NUL-terminated; the descriptor lives as long as the process.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return None;
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let flags = PIPE_ACCESS_OUTBOUND | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let h = CreateNamedPipeW(wide(name).as_ptr(), flags, PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, MAX_CLIENTS, 4096, 4096, 0, &sa);
        if h == INVALID_HANDLE_VALUE { None } else { Some(h) }
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        // A GUI-subsystem exe has no console; the version is also in the file's version info.
        return;
    }
    if !is_elevated() {
        std::process::exit(2);
    }
    let (Some(name), Some(sid)) = (proto::pipe_name(), proto::user_sid()) else { std::process::exit(3) };
    LAST_ACTIVE.store(now_ms(), Ordering::Relaxed);
    // Watchdog: leave when nobody has been connected for IDLE_EXIT.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(5));
        if CLIENTS.load(Ordering::Relaxed) == 0 && now_ms().saturating_sub(LAST_ACTIVE.load(Ordering::Relaxed)) > IDLE_EXIT.as_millis() as u64 {
            std::process::exit(0);
        }
    });
    // One sampler for all readers; it only works while somebody is connected.
    std::thread::spawn(|| {
        let mut reader = make_reader();
        loop {
            if CLIENTS.load(Ordering::Relaxed) == 0 {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let line = match &mut reader {
                Ok(r) => proto::to_line(&r.sample()),
                Err(e) => proto::to_line(&Sample { helper: VERSION.into(), err: e.clone(), ..Default::default() }),
            };
            if let Ok(mut l) = LINE.lock() {
                *l = line;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    let mut first = true;
    loop {
        let Some(pipe) = create_pipe(&name, &sid, first) else {
            // Another instance already owns the pipe (or it cannot be created): leave quietly.
            std::process::exit(4);
        };
        first = false;
        // SAFETY: valid pipe handle; the watchdog covers the wait.
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
        if !connected {
            // SAFETY: handle created above.
            unsafe { CloseHandle(pipe) };
            continue;
        }
        let h = pipe as isize;
        CLIENTS.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            serve(h as HANDLE);
            // SAFETY: the handle belongs to this thread now.
            unsafe {
                DisconnectNamedPipe(h as HANDLE);
                CloseHandle(h as HANDLE);
            }
            CLIENTS.fetch_sub(1, Ordering::Relaxed);
            LAST_ACTIVE.store(now_ms(), Ordering::Relaxed);
        });
    }
}

fn serve(pipe: HANDLE) {
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let line = LINE.lock().map(|l| l.clone()).unwrap_or_default();
        if !line.is_empty() {
            let mut written = 0u32;
            // SAFETY: valid pipe and buffer.
            if unsafe { WriteFile(pipe, line.as_ptr(), line.len() as u32, &mut written, std::ptr::null_mut()) } == 0 {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(900));
    }
}
