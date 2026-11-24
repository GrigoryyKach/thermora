//! The tray process (`--background`, or `--minimized` from the startup entry). It draws no window: it owns the tray
//! icon, whose tooltip shows the CPU and GPU temperature, and opens the window process on request. With the
//! window closed, this is all that runs, so Thermora stays small while it waits in the tray.

use crate::config::Config;
use crate::i18n;
use crate::sensors::{self, Shared};
use crate::tray;
use crate::tray_common::{tooltip, TrayState};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Ctx {
    dir: PathBuf,
    shared: Shared,
}

static CTX: Mutex<Option<Ctx>> = Mutex::new(None);
static QUIT: AtomicBool = AtomicBool::new(false);

pub fn request_quit() {
    QUIT.store(true, Ordering::Relaxed);
    #[cfg(windows)]
    crate::winloop::request_quit();
}

fn state() -> Option<TrayState> {
    let g = CTX.lock().ok()?;
    let c = g.as_ref()?;
    // The language is read from the settings each time, so a change in the window shows in the tray menu too.
    let cfg = Config::load_from(&c.dir);
    let lang = i18n::resolve(cfg.language.as_deref());
    let live = c.shared.lock().ok()?.live.clone();
    Some(TrayState { lang, tooltip: tooltip(&live, lang) })
}

/// Called every couple of seconds on the tray thread.
pub fn on_tick() {
    if let Some(s) = state() {
        tray::update(s);
    }
}

pub fn on_shutdown() {
    tray::remove();
}

pub fn run(dir: PathBuf, _cfg: Config, demo: bool) {
    let served = crate::ipc::serve("bg", |m| {
        if m == "quit" {
            request_quit();
        }
        "ok".into()
    });
    if !served {
        return; // another tray process of this data folder is already running
    }
    let shared = sensors::shared();
    sensors::spawn(Box::new(|| {}), shared.clone(), demo, Arc::new(AtomicBool::new(false)));
    // The CPU temperature access is set up by the window; the tray part just follows it (a cheap check every 30 s).
    #[cfg(windows)]
    std::thread::spawn(|| loop {
        crate::sensors::link::set_enabled(crate::setup::is_enabled());
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
    *CTX.lock().unwrap() = Some(Ctx { dir, shared });
    let initial = TrayState { lang: i18n::resolve(None), tooltip: "Thermora".into() };
    #[cfg(windows)]
    {
        crate::winloop::run(|| {
            let st = state().unwrap_or(initial);
            tray::create(&st, crate::ICON_PNG);
        });
        tray::remove();
    }
    #[cfg(unix)]
    {
        let st = state().unwrap_or(initial);
        if !tray::create(&st, crate::ICON_PNG) {
            // No tray host: nothing to show an icon in, nothing to do here.
            return;
        }
        while !QUIT.load(Ordering::Relaxed) {
            on_tick();
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        tray::remove();
    }
}
