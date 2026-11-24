//! The client side of the helper: reads the lines `ThermoraSensor.exe` writes to its named pipe, and starts the
//! helper (through its scheduled task, which needs no permission prompt for the user it was created for) when the
//! pipe is not there. Without the CPU temperature access set up, this thread does nothing.

use crate::sensor_proto::{self as proto, Sample};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static LATEST: Mutex<Option<(Instant, Sample)>> = Mutex::new(None);
static STARTED: OnceLock<()> = OnceLock::new();

#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    /// The CPU temperature access has not been set up.
    NotSetUp,
    /// Set up, waiting for the helper's first line.
    Starting,
    /// The helper answered but cannot read this CPU; the reason is its short code.
    Unsupported(String),
    Ok(Sample),
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        if let Ok(mut l) = LATEST.lock() {
            *l = None;
        }
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// How a line is turned into a state; separated so it can be tested.
pub fn classify(s: &Sample) -> LinkState {
    if s.err.is_empty() {
        LinkState::Ok(s.clone())
    } else if s.err == "no-reading" {
        LinkState::Starting
    } else {
        LinkState::Unsupported(s.err.clone())
    }
}

pub fn state() -> LinkState {
    if !enabled() {
        return LinkState::NotSetUp;
    }
    match LATEST.lock().ok().and_then(|l| l.clone()) {
        Some((at, s)) if at.elapsed() < Duration::from_secs(5) => classify(&s),
        _ => LinkState::Starting,
    }
}

fn start_task() {
    let _ = std::process::Command::new("schtasks.exe")
        .args(["/Run", "/TN", proto::TASK_NAME])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}

/// Starts the reader thread once. Cheap while the access is not set up (a sleep and a flag).
pub fn start() {
    if STARTED.set(()).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("thermora-link".into())
        .spawn(|| {
            let mut last_start: Option<Instant> = None;
            loop {
                if !enabled() {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                let Some(name) = proto::pipe_name() else {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                };
                match std::fs::OpenOptions::new().read(true).open(&name) {
                    Ok(f) => {
                        for line in BufReader::new(f).lines() {
                            let Ok(line) = line else { break };
                            if let Some(s) = proto::from_line(&line) {
                                if let Ok(mut l) = LATEST.lock() {
                                    *l = Some((Instant::now(), s));
                                }
                            }
                            if !enabled() {
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        // Not running: ask the task to start it, at most every 15 s.
                        if last_start.is_none_or(|t| t.elapsed() > Duration::from_secs(15)) {
                            start_task();
                            last_start = Some(Instant::now());
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_classified() {
        let ok = Sample { pkg: Some(40), ..Default::default() };
        assert!(matches!(classify(&ok), LinkState::Ok(_)));
        let u = Sample { err: "unsupported-cpu".into(), ..Default::default() };
        assert_eq!(classify(&u), LinkState::Unsupported("unsupported-cpu".into()));
        let n = Sample { err: "no-reading".into(), ..Default::default() };
        assert_eq!(classify(&n), LinkState::Starting);
    }

    #[test]
    fn without_setup_the_state_is_not_set_up() {
        set_enabled(false);
        assert_eq!(state(), LinkState::NotSetUp);
    }
}
