//! "Enable CPU temperature" (Windows): the one-time elevated setup, its undo, and the checks the window needs.
//!
//! Design (see `docs/SENSORS.md`): the window never runs elevated. When the user agrees, Thermora starts ITSELF
//! with the Windows permission prompt (`--setup-sensor`); that short elevated run
//!   1. installs PawnIO from its official installer when it is missing (hash-pinned download, run only if it matches),
//!   2. copies `ThermoraSensor.exe` and the signed PawnIO module into `%ProgramFiles%\Thermora` (admin-only
//!      writable, so a user-level program cannot swap what runs elevated),
//!   3. registers the scheduled task "Thermora Sensor" (run with highest privileges, for this user, on demand).
//! From then on Thermora starts the helper through that task with no prompt. The helper is a separate exe with its
//! own version: the copy in Program Files is refreshed (one more prompt, explained in Settings) only when its hash
//! differs from the one shipped next to Thermora.exe. `--remove-sensor` undoes steps 2 and 3; PawnIO stays.

#![cfg_attr(not(windows), allow(dead_code))]

/// Pinned downloads. Official sources; each is checked against its SHA-256 before anything runs or is copied.
/// The hashes are the SHA-256 digests GitHub records for these release assets (`gh api` /releases/tags/<tag>, field
/// `digest`), read without downloading the files, and the same `InstallerSha256` as the winget manifest of the
/// installer (checked by Microsoft's repository); the download is checked against them again before anything runs.
/// The `IntelMSR.bin` inside the modules zip is covered by the zip's hash.
pub struct Pin {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const PAWNIO_INSTALLER: Pin = Pin {
    name: "PawnIO_setup.exe",
    url: "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe",
    sha256: "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032",
    size: 3_410_960,
};
pub const PAWNIO_MODULES: Pin = Pin {
    name: "release_0_2_12.zip",
    url: "https://github.com/namazso/PawnIO.Modules/releases/download/0.2.12/release_0_2_12.zip",
    sha256: "09698897ce19b92f1043a2e322cfcf0b1fe823e0b576971b178a65244fb0af9d",
    size: 83_556,
};

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::download;
    use crate::selfupdate::sha256_file;
    use crate::sensor_proto as proto;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    const NO_WINDOW: u32 = 0x0800_0000;
    pub const SENSOR_EXE: &str = "ThermoraSensor.exe";

    /// `%ProgramFiles%\Thermora`, or the folder named by `THERMORA_PF_DIR` in a test instance (never the real one).
    pub fn pf_dir() -> PathBuf {
        if crate::config::is_test_instance() {
            if let Some(d) = std::env::var_os("THERMORA_PF_DIR").filter(|d| !d.is_empty()) {
                return PathBuf::from(d);
            }
        }
        std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("Thermora")
    }

    pub fn installed_sensor() -> PathBuf {
        pf_dir().join(SENSOR_EXE)
    }

    /// The helper shipped next to `Thermora.exe` (inside the release zip).
    pub fn bundled_sensor() -> Option<PathBuf> {
        let p = std::env::current_exe().ok()?.parent()?.join(SENSOR_EXE);
        p.exists().then_some(p)
    }

    fn quiet(program: &str) -> Command {
        let mut c = Command::new(program);
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(NO_WINDOW);
        c
    }

    pub fn task_exists() -> bool {
        quiet("schtasks.exe").args(["/Query", "/TN", proto::TASK_NAME]).status().is_ok_and(|s| s.success())
    }

    /// Is the CPU temperature access set up: the helper is in place and its task is registered?
    pub fn is_enabled() -> bool {
        installed_sensor().exists() && task_exists()
    }

    /// The shipped helper differs from the installed one (hash), so a refresh is offered.
    pub fn helper_needs_update() -> bool {
        match (bundled_sensor(), installed_sensor()) {
            (Some(b), i) if i.exists() => match (sha256_file(&b), sha256_file(&i)) {
                (Ok(x), Ok(y)) => x != y,
                _ => false,
            },
            _ => false,
        }
    }

    /// The installed PawnIO version from its uninstall entry (readable without elevation).
    pub fn pawnio_version() -> Option<String> {
        use winreg::{enums::*, RegKey};
        let hk = RegKey::predef(HKEY_LOCAL_MACHINE);
        for flag in [KEY_READ | KEY_WOW64_64KEY, KEY_READ | KEY_WOW64_32KEY] {
            if let Ok(k) = hk.open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO", flag) {
                return Some(k.get_value::<String, _>("DisplayVersion").unwrap_or_else(|_| "installed".into()));
            }
        }
        None
    }

    // ───────────────────────── the window side ─────────────────────────

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Outcome {
        Ok,
        Declined,
        Failed(String),
    }

    /// Runs `Thermora.exe <flag> --result-file <file>` with the Windows permission prompt and waits for it.
    pub fn run_elevated(flag: &str, result_file: &Path) -> Outcome {
        let Ok(exe) = std::env::current_exe() else { return Outcome::Failed("no exe path".into()) };
        let _ = std::fs::remove_file(result_file);
        let wide = |s: &std::ffi::OsStr| -> Vec<u16> { std::os::windows::ffi::OsStrExt::encode_wide(s).chain(std::iter::once(0)).collect() };
        let verb = wide(std::ffi::OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let params = wide(std::ffi::OsStr::new(&format!("{flag} --result-file \"{}\"", result_file.display())));
        // SAFETY: a zero-initialised SHELLEXECUTEINFOW with its size set and NUL-terminated strings that outlive the call.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = 0;
            if ShellExecuteExW(&mut info) == 0 {
                let e = windows_sys::Win32::Foundation::GetLastError();
                return if e == 1223 { Outcome::Declined } else { Outcome::Failed(format!("could not start the elevated step (error {e})")) };
            }
            let h = info.hProcess;
            if h.is_null() {
                return Outcome::Failed("no process handle".into());
            }
            let mut code = 1u32;
            if WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0 {
                GetExitCodeProcess(h, &mut code);
            }
            CloseHandle(h);
            if code == 0 {
                Outcome::Ok
            } else {
                let why = std::fs::read_to_string(result_file).unwrap_or_default();
                Outcome::Failed(if why.trim().is_empty() { format!("exit code {code}") } else { why.trim().to_string() })
            }
        }
    }

    // ───────────────────────── the elevated side ─────────────────────────

    fn is_elevated() -> bool {
        use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        // SAFETY: plain Win32 calls with a correctly sized output struct; the token is closed.
        unsafe {
            let mut tok = std::ptr::null_mut();
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

    /// The scheduled task: this user, highest privileges, no trigger (started on demand by Thermora).
    pub fn task_xml(sid: &str, exe: &Path) -> String {
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Thermora sensor helper: reads CPU temperature through PawnIO for Thermora.</Description></RegistrationInfo>
  <Principals><Principal id="Author"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command></Exec></Actions>
</Task>
"#,
            esc(sid),
            esc(&exe.display().to_string())
        )
    }

    fn fail(result_file: &Path, msg: &str) -> i32 {
        let _ = std::fs::write(result_file, msg);
        1
    }

    /// Stops the scheduled task (so it cannot start the helper again by itself).
    fn stop_task() -> Result<(), String> {
        // `/End` fails when the task is not running: that is fine.
        let _ = quiet("schtasks.exe").args(["/End", "/TN", proto::TASK_NAME]).status();
        Ok(())
    }

    /// Kills every `ThermoraSensor.exe` whose image is inside `dir`, each by its own PID (not by name).
    fn kill_helpers(dir: &Path) -> Result<(), String> {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
        let want = dir.display().to_string().to_lowercase();
        let mut failed = Vec::new();
        // SAFETY: a ToolHelp snapshot walked with the documented first/next calls; every handle is closed.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return Err("cannot list processes".into());
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case(SENSOR_EXE) {
                    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, 0, e.th32ProcessID);
                    if !h.is_null() {
                        let mut buf = [0u16; 520];
                        let mut len = buf.len() as u32;
                        let inside = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 && String::from_utf16_lossy(&buf[..len as usize]).to_lowercase().starts_with(&want);
                        if inside && TerminateProcess(h, 0) == 0 {
                            failed.push(e.th32ProcessID);
                        }
                        CloseHandle(h);
                    } else {
                        failed.push(e.th32ProcessID);
                    }
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        if failed.is_empty() { Ok(()) } else { Err(format!("could not end the helper (pid {failed:?})")) }
    }

    fn delete_task() -> Result<(), String> {
        let st = quiet("schtasks.exe").args(["/Delete", "/TN", proto::TASK_NAME, "/F"]).status().map_err(|e| e.to_string())?;
        // "Does not exist" counts as deleted.
        if st.success() || !task_exists() { Ok(()) } else { Err("the scheduled task could not be deleted".into()) }
    }

    /// Deletes the program folder, retrying a few times (a helper that just ended may still hold its files).
    fn delete_folder(dir: &Path) -> Result<(), String> {
        let mut last = String::new();
        for _ in 0..12 {
            match std::fs::remove_dir_all(dir) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => {
                    last = e.to_string();
                    // A helper restarted in the meantime would block the delete: end it again.
                    let _ = kill_helpers(dir);
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
        Err(format!("the helper folder could not be removed: {last}"))
    }

    /// The Disable steps, in the order they must run: a running helper holds its files and a live task would start
    /// it again, so stop the task, end the helper by PID, delete the task, then delete the folder.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RemoveStep {
        StopTask,
        KillHelper,
        DeleteTask,
        DeleteFolder,
    }

    pub const REMOVE_ORDER: [RemoveStep; 4] = [RemoveStep::StopTask, RemoveStep::KillHelper, RemoveStep::DeleteTask, RemoveStep::DeleteFolder];

    /// Runs all steps in order, never skipping a later one because an earlier one failed; returns every error.
    pub fn run_remove(exec: &mut dyn FnMut(RemoveStep) -> Result<(), String>) -> Result<(), String> {
        let mut errors = Vec::new();
        for step in REMOVE_ORDER {
            if let Err(e) = exec(step) {
                errors.push(e);
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    fn install_pawnio(tmp: &Path) -> Result<(), String> {
        if pawnio_version().is_some() {
            return Ok(());
        }
        let p = &PAWNIO_INSTALLER;
        let dest = tmp.join(p.name);
        download::download_verified(p.url, &dest, p.sha256, p.size.max(1) * 2, false).map_err(|e| format!("PawnIO installer: {e}"))?;
        // Checked again right before running: the file sat on disk for a moment (in an admin-only folder).
        download::verify(&dest, p.sha256).map_err(|e| e.to_string())?;
        let st = Command::new(&dest).args(PAWNIO_INSTALL_ARGS).stdin(Stdio::null()).status().map_err(|e| format!("could not start the PawnIO installer: {e}"))?;
        if !st.code().is_some_and(|c| PAWNIO_OK_CODES.contains(&c)) && pawnio_version().is_none() {
            return Err(format!("the PawnIO installer ended with {st}"));
        }
        if pawnio_version().is_none() {
            return Err("PawnIO does not appear as installed after its installer finished".into());
        }
        Ok(())
    }

    /// The silent switches and success codes of the PawnIO installer, as published in its winget manifest
    /// (`namazso.PawnIO` 2.2.0: `InstallerSwitches: Silent: -install -silent`, `InstallerSuccessCodes: 3010`,
    /// `ElevationRequirement: elevatesSelf`, `ProductCode: PawnIO`).
    pub const PAWNIO_INSTALL_ARGS: &[&str] = &["-install", "-silent"];
    const PAWNIO_OK_CODES: &[i32] = &[0, 3010];

    fn install_module(tmp: &Path, dir: &Path) -> Result<(), String> {
        let p = &PAWNIO_MODULES;
        let zip = tmp.join(p.name);
        download::download_verified(p.url, &zip, p.sha256, p.size.max(1) * 2, false).map_err(|e| format!("PawnIO modules: {e}"))?;
        let out = tmp.join("modules");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let st = quiet(root.join("System32").join("tar.exe").to_str().unwrap_or("tar.exe")).arg("-xf").arg(&zip).arg("-C").arg(&out).status().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err("could not unpack the PawnIO modules".into());
        }
        let bin = find_file(&out, "IntelMSR.bin").ok_or("IntelMSR.bin is not in the PawnIO modules release")?;
        std::fs::create_dir_all(dir.join("modules")).map_err(|e| e.to_string())?;
        std::fs::copy(&bin, dir.join("modules").join("IntelMSR.bin")).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(f) = find_file(&p, name) {
                    return Some(f);
                }
            } else if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                return Some(p);
            }
        }
        None
    }

    fn register_task(dir: &Path) -> Result<(), String> {
        let sid = proto::user_sid().ok_or("could not read the user")?;
        let xml = task_xml(&sid, &dir.join(SENSOR_EXE));
        let file = dir.join("task.xml");
        let mut bytes = vec![0xFF, 0xFE];
        for u in xml.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
        let st = quiet("schtasks.exe").args(["/Create", "/TN", proto::TASK_NAME, "/XML"]).arg(&file).arg("/F").status().map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&file);
        if st.success() { Ok(()) } else { Err("the scheduled task could not be created".into()) }
    }

    /// Entry point of the elevated runs. Returns the process exit code (0 is success).
    pub fn elevated_main(args: &[String]) -> i32 {
        let result = args.iter().position(|a| a == "--result-file").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("thermora-setup.txt"));
        if !is_elevated() {
            return fail(&result, "not elevated");
        }
        let dir = pf_dir();
        let has = |f: &str| args.iter().any(|a| a == f);
        if has("--remove-sensor") {
            let r = run_remove(&mut |step| match step {
                RemoveStep::StopTask => stop_task(),
                RemoveStep::KillHelper => kill_helpers(&dir),
                RemoveStep::DeleteTask => delete_task(),
                RemoveStep::DeleteFolder => {
                    // The task is gone now, so nothing starts the helper again; end any straggler, then delete.
                    kill_helpers(&dir)?;
                    delete_folder(&dir)
                }
            });
            return match r {
                Ok(()) if !dir.exists() => 0,
                Ok(()) => fail(&result, "the helper folder is still there"),
                Err(e) => fail(&result, &e),
            };
        }
        let refresh_only = has("--refresh-sensor");
        let Some(src) = bundled_sensor() else { return fail(&result, "ThermoraSensor.exe is missing next to Thermora.exe") };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return fail(&result, &format!("cannot create the program folder: {e}"));
        }
        // Downloads go into the admin-only folder, so nothing can swap them between the check and the run.
        let tmp = dir.join("tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        if !refresh_only {
            if let Err(e) = install_pawnio(&tmp) {
                let _ = std::fs::remove_dir_all(&tmp);
                return fail(&result, &e);
            }
            if let Err(e) = install_module(&tmp, &dir) {
                let _ = std::fs::remove_dir_all(&tmp);
                return fail(&result, &e);
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = stop_task();
        if let Err(e) = kill_helpers(&dir) {
            return fail(&result, &e);
        }
        if let Err(e) = std::fs::copy(&src, dir.join(SENSOR_EXE)) {
            return fail(&result, &format!("cannot copy the helper: {e}"));
        }
        if let Err(e) = register_task(&dir) {
            return fail(&result, &e);
        }
        0
    }

    /// Does this command line ask for an elevated step?
    pub fn is_elevated_request(args: &[String]) -> bool {
        args.iter().any(|a| a == "--setup-sensor" || a == "--remove-sensor" || a == "--refresh-sensor")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_task_runs_for_one_user_with_highest_privileges_and_no_trigger() {
            let x = task_xml("S-1-5-21-1-2-3-1001", Path::new(r"C:\Program Files\Thermora\ThermoraSensor.exe"));
            assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
            assert!(x.contains("<LogonType>InteractiveToken</LogonType>"));
            assert!(x.contains("<UserId>S-1-5-21-1-2-3-1001</UserId>"));
            assert!(x.contains(r"<Command>C:\Program Files\Thermora\ThermoraSensor.exe</Command>"));
            assert!(!x.contains("<Triggers>"), "no trigger: Thermora starts it on demand");
            assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        }

        #[test]
        fn xml_special_characters_are_escaped() {
            let x = task_xml("S-1", Path::new(r"C:\A&B\x.exe"));
            assert!(x.contains("C:\\A&amp;B\\x.exe"));
        }

        #[test]
        fn disable_runs_its_steps_in_order_and_does_not_stop_at_the_first_error() {
            let mut seen = Vec::new();
            let r = run_remove(&mut |s| {
                seen.push(s);
                if s == RemoveStep::KillHelper { Err("pid 1 would not end".into()) } else { Ok(()) }
            });
            assert_eq!(seen, vec![RemoveStep::StopTask, RemoveStep::KillHelper, RemoveStep::DeleteTask, RemoveStep::DeleteFolder]);
            assert!(r.unwrap_err().contains("would not end"), "the error reaches the caller (and so the window)");
            assert_eq!(run_remove(&mut |_| Ok(())), Ok(()));
        }

        #[test]
        fn deleting_a_missing_folder_is_fine_and_a_real_one_goes() {
            let d = std::env::temp_dir().join(format!("thermora-del-{}", std::process::id()));
            assert_eq!(delete_folder(&d), Ok(()));
            std::fs::create_dir_all(d.join("modules")).unwrap();
            std::fs::write(d.join("modules").join("x.bin"), b"x").unwrap();
            assert_eq!(delete_folder(&d), Ok(()));
            assert!(!d.exists());
        }

        #[test]
        fn the_elevated_flags_are_recognised() {
            assert!(is_elevated_request(&["--setup-sensor".into()]));
            assert!(is_elevated_request(&["--remove-sensor".into(), "--result-file".into(), "x".into()]));
            assert!(!is_elevated_request(&["--demo".into()]));
        }

        #[test]
        fn the_pins_are_well_formed_and_https() {
            for p in [&PAWNIO_INSTALLER, &PAWNIO_MODULES] {
                assert_eq!(p.sha256.len(), 64, "{}", p.name);
                assert!(p.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", p.name);
                assert!(p.url.starts_with("https://github.com/namazso/"), "{}", p.name);
                assert!(p.size > 0 && p.url.ends_with(p.name));
            }
        }

        #[test]
        fn pf_dir_ends_with_thermora() {
            assert!(pf_dir().ends_with("Thermora"));
        }
    }
}
