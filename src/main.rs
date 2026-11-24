#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Thermora: a hardware monitor with live load and temperatures and the specs of this computer.
//!
//! One executable, three roles:
//! - the **window** (the default) is the monitor and the specs page; it ends when it is closed;
//! - the **tray part** (`--background`, or `--minimized` from the startup entry) has no window: it owns the tray icon
//!   (the tooltip shows the CPU and GPU temperature) and opens the window on request. It runs when "Start with the
//!   system" is on, so closing the window then keeps Thermora in the tray; with it off, closing exits;
//! - the **elevated steps** (`--setup-sensor`, `--refresh-sensor`, `--remove-sensor`, Windows) are run by the window
//!   itself after the user agrees and Windows asks for permission (see `setup.rs`).
//!
//! `--demo` (or `THERMORA_DEMO=1`) runs with plausible fake readings and a fake hardware list, for screenshots.
//! `--probe [--seconds N]` prints the readings of this computer once a second and exits (a console tool).

mod background;
mod bus;
mod config;
mod download;
#[allow(dead_code)]
mod i18n;
mod ipc;
mod model;
#[allow(dead_code)]
mod nimvoro;
#[cfg(windows)]
mod platform;
#[cfg(not(windows))]
#[path = "platform_linux.rs"]
mod platform;
#[allow(dead_code)]
mod selfupdate;
mod sensors;
mod setup;
mod specs;
mod strings;
mod support;
#[allow(dead_code)]
mod theme;
mod tray_common;
mod ui;
#[allow(dead_code)]
mod update_ui;

#[cfg(windows)]
#[path = "../sensor/src/proto.rs"]
mod sensor_proto;
#[cfg(windows)]
mod tray;
#[cfg(not(windows))]
#[path = "tray_linux.rs"]
mod tray;
#[cfg(windows)]
mod winloop;

use config::Config;
use eframe::egui;

/// The repository, kept in one place (About page and the update check).
pub const REPO_URL: &str = "https://github.com/GrigoryyKach/thermora";
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn main() {
    selfupdate::startup("thermora");
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        if setup::is_elevated_request(&args) {
            std::process::exit(setup::elevated_main(&args));
        }
        if args.iter().any(|a| a == "--probe") {
            // SAFETY: plain Win32 call; failing (no parent console) is fine.
            unsafe { windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS) };
        }
    }
    if args.iter().any(|a| a == "--probe") {
        let secs = args.iter().position(|a| a == "--seconds").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(10);
        probe(secs);
        return;
    }
    let dir = config::data_dir();
    let demo = args.iter().any(|a| a == "--demo") || std::env::var("THERMORA_DEMO").is_ok_and(|v| v == "1");
    if args.iter().any(|a| a == "--background" || a == "--minimized") {
        background::run(dir.clone(), Config::load_from(&dir), demo);
    } else if let Err(e) = run_window(dir, demo) {
        eprintln!("thermora: {e}");
    }
}

/// Asks the tray part to quit and waits a moment for it to leave (before an update replaces the files).
pub fn stop_background() {
    if ipc::request("bg", "quit").is_some() {
        for _ in 0..30 {
            if !ipc::is_running("bg") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn run_window(dir: std::path::PathBuf, demo: bool) -> eframe::Result<()> {
    // One window per data folder: a second launch brings the first one forward.
    let served = ipc::serve("ui", |m| {
        if m == "show" {
            bus::push(bus::Cmd::Show);
        }
        "ok".into()
    });
    if !served {
        platform::allow_foreground();
        ipc::request("ui", "show");
        return Ok(());
    }
    let cfg = Config::load_from(&dir);
    // With "Start with the system" on, the tray part keeps running after the window closes.
    if platform::autostart_enabled() && !demo && std::env::var_os("THERMORA_NO_BACKGROUND").is_none() && !ipc::is_running("bg") {
        platform::spawn_background();
    }

    let mut viewport = egui::ViewportBuilder::default().with_title(platform::WINDOW_TITLE).with_app_id("local.thermora").with_min_inner_size([900.0, 600.0]).with_icon(std::sync::Arc::new(app_icon()));
    let mut centered = true;
    match cfg.window {
        Some(w) if w.w >= 900.0 && w.h >= 600.0 => {
            viewport = viewport.with_inner_size([w.w, w.h]);
            if let (Some(x), Some(y)) = (w.x, w.y) {
                if x > -20000.0 && y > -20000.0 && x < 20000.0 && y < 20000.0 {
                    viewport = viewport.with_position([x, y]);
                    centered = false;
                }
            }
        }
        _ => viewport = viewport.with_inner_size([1180.0, 760.0]),
    }
    // Screenshots may ask for a size (test folder only): THERMORA_WINDOW=1280x900.
    if config::is_test_instance() {
        if let Some((w, h)) = std::env::var("THERMORA_WINDOW").ok().and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))) {
            viewport = viewport.with_inner_size([w, h]);
        }
    }
    let options = eframe::NativeOptions { viewport, centered, ..Default::default() };
    eframe::run_native(platform::WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, cfg, dir, demo)))))
}

fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(ICON_PNG).unwrap_or_default()
}

/// A console check of this computer's sensors: prints a few readings once a second.
fn probe(secs: u32) {
    let mut c = sensors::platform_collector();
    println!("Thermora probe, {secs} s");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..secs {
        let l = sensors::Collector::poll(&mut *c);
        let show = |r: &Option<model::Rd<f32>>| match r {
            Some(Ok(v)) => format!("{v:.1}"),
            Some(Err(w)) => format!("- ({w:?})"),
            None => "-".into(),
        };
        println!(
            "t={i:>2} cpu {:>5.1}% clock {} temp {} hot {} power {} | gpu {} | ram {}/{} MB | net {} {:.0} B/s down | disks {}",
            l.cpu.load,
            l.cpu.clock_mhz.map(|m| format!("{m:.0} MHz")).unwrap_or("-".into()),
            show(&l.cpu.temp),
            show(&l.cpu.hotspot),
            show(&l.cpu.power),
            l.gpu.as_ref().map(|g| format!("{:.0}% {} C hot {}", g.load, show(&g.temp), show(&g.hotspot))).unwrap_or("-".into()),
            l.ram.used >> 20,
            l.ram.total >> 20,
            l.net.adapter,
            l.net.down,
            l.disks.iter().map(|d| format!("{} {:.0}% {}", d.name, d.busy, show(&d.temp))).collect::<Vec<_>>().join("; ")
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
