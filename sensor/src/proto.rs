//! The tiny protocol between `ThermoraSensor.exe` (elevated, writes) and Thermora (reads), shared by both
//! crates through `#[path]`. One text line per second over a named pipe that only the installing user can open:
//!
//! `v=1;helper=0.1.0;tjmax=100;pkg=43;hot=45;w=12.3;cores=41,40,-,42;err=`
//!
//! Absent values are `-`. `err` is empty when the readings are valid. Nothing here can ask the helper to do
//! anything: the stream is read-only.

#![allow(dead_code)]

pub const PROTOCOL: u32 = 1;
pub const PIPE_PREFIX: &str = r"\\.\pipe\ThermoraSensor-";
pub const TASK_NAME: &str = "Thermora Sensor";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub helper: String,
    pub tjmax: u32,
    pub pkg: Option<u32>,
    /// Hottest core.
    pub hot: Option<u32>,
    pub watts: Option<f64>,
    pub cores: Vec<Option<u32>>,
    /// Why there are no readings (empty: all fine).
    pub err: String,
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
}

pub fn to_line(s: &Sample) -> String {
    let cores: Vec<String> = s.cores.iter().map(|c| opt(*c)).collect();
    let w = s.watts.map(|w| format!("{w:.1}")).unwrap_or_else(|| "-".into());
    let err: String = s.err.chars().filter(|c| *c != ';' && *c != '\n' && *c != '\r').collect();
    format!("v={PROTOCOL};helper={};tjmax={};pkg={};hot={};w={w};cores={};err={err}\n", s.helper, s.tjmax, opt(s.pkg), opt(s.hot), cores.join(","))
}

pub fn from_line(line: &str) -> Option<Sample> {
    let mut s = Sample::default();
    let mut version = 0;
    for part in line.trim().split(';') {
        let (k, v) = part.split_once('=')?;
        let num = |v: &str| v.parse::<u32>().ok();
        match k {
            "v" => version = num(v)?,
            "helper" => s.helper = v.to_string(),
            "tjmax" => s.tjmax = num(v)?,
            "pkg" => s.pkg = num(v),
            "hot" => s.hot = num(v),
            "w" => s.watts = v.parse().ok(),
            "cores" => s.cores = if v.is_empty() { vec![] } else { v.split(',').map(num).collect() },
            "err" => s.err = v.to_string(),
            _ => {}
        }
    }
    (version == PROTOCOL).then_some(s)
}

/// The SID of the user running this process, as the string form (`S-1-5-21-...`).
#[cfg(windows)]
pub fn user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: plain Win32 calls with buffers sized by the first call; the token handle is closed.
    unsafe {
        let mut tok = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return None;
        }
        let mut need = 0u32;
        GetTokenInformation(tok, TokenUser, std::ptr::null_mut(), 0, &mut need);
        let mut buf = vec![0u8; need.max(64) as usize];
        let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut need);
        CloseHandle(tok);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut wide: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
            return None;
        }
        let mut n = 0;
        while *wide.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
        LocalFree(wide.cast());
        Some(s)
    }
}

#[cfg(windows)]
pub fn pipe_name() -> Option<String> {
    user_sid().map(|s| format!("{PIPE_PREFIX}{s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_round_trips() {
        let s = Sample { helper: "0.1.0".into(), tjmax: 100, pkg: Some(43), hot: Some(47), watts: Some(12.34), cores: vec![Some(41), None, Some(47)], err: String::new() };
        let back = from_line(&to_line(&s)).unwrap();
        assert_eq!(back.pkg, Some(43));
        assert_eq!(back.hot, Some(47));
        assert_eq!(back.cores, vec![Some(41), None, Some(47)]);
        assert_eq!(back.watts, Some(12.3));
        assert_eq!(back.helper, "0.1.0");
    }

    #[test]
    fn errors_and_garbage_are_handled() {
        let s = Sample { err: "no driver; really\n".into(), ..Default::default() };
        let back = from_line(&to_line(&s)).unwrap();
        assert!(!back.err.contains(';') && !back.err.contains('\n'));
        assert!(from_line("garbage").is_none());
        assert!(from_line("v=2;tjmax=1").is_none());
    }
}
