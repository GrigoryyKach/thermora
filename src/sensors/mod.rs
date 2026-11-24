//! Sensor collection: one thread polls the platform collector once a second and publishes the result.
//! Windows: `win` (PDH, Win32, NVML) plus the CPU temperatures from the elevated helper (`link`).
//! Linux: `linux` (procfs, sysfs, hwmon). `demo` makes plausible fake readings for screenshots.

use crate::model::{History, Live};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub mod demo;
#[cfg(unix)]
pub mod linux;
#[cfg(windows)]
pub mod link;
pub mod nvidia;
#[cfg(windows)]
pub mod win;

#[derive(Default)]
pub struct Shot {
    pub live: Live,
    pub history: Option<History>,
}

pub type Shared = Arc<Mutex<Shot>>;

pub trait Collector: Send {
    fn poll(&mut self) -> Live;
}

pub fn shared() -> Shared {
    Arc::new(Mutex::new(Shot { live: Live::default(), history: Some(History::new()) }))
}

pub fn platform_collector() -> Box<dyn Collector> {
    #[cfg(windows)]
    return Box::new(win::WinCollector::new());
    #[cfg(unix)]
    return Box::new(linux::LinuxCollector::new());
}

/// Starts the polling thread. `wake` repaints the window; it is only called while `visible` is set.
pub fn spawn(wake: Box<dyn Fn() + Send>, shared: Shared, demo: bool, visible: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("thermora-sensors".into())
        .spawn(move || {
            let mut c: Box<dyn Collector> = if demo { Box::new(demo::DemoCollector::new()) } else { platform_collector() };
            // The demo starts with a full minute of history, so a screenshot taken at once has real-looking charts.
            let mut warm = if demo { crate::model::History::POINTS } else { 0 };
            loop {
                let t0 = Instant::now();
                let live = c.poll();
                if let Ok(mut s) = shared.lock() {
                    if let Some(h) = s.history.as_mut() {
                        h.push(&live);
                    }
                    s.live = live;
                }
                if visible.load(Ordering::Relaxed) {
                    wake();
                }
                if warm > 0 {
                    warm -= 1;
                    continue;
                }
                std::thread::sleep(Duration::from_millis(1000).saturating_sub(t0.elapsed()));
            }
        })
        .ok();
}
