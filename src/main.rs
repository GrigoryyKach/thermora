//! Thermora, T1 sensor prototype.
//!
//! `Thermora --probe [--seconds N]` prints the specs once, then every reading it can get once per
//! second. It never installs anything, never elevates and never loads a driver by itself: if the
//! PawnIO driver is already installed AND this process already runs elevated, it also tries MSR
//! reads through it (CPU temperatures, package power, APERF/MPERF clock).
//!
//! Privacy: serial numbers, MAC addresses, UUIDs and the public IP are never read or printed.

mod nvidia;

#[cfg(windows)]
mod pawnio;
#[cfg(windows)]
mod win;

#[cfg(unix)]
mod linux;

use std::time::{Duration, Instant};

/// One line of probe output. `value == None` prints as an em dash plus the reason.
pub fn line(section: &str, name: &str, value: Result<String, String>) {
    match value {
        Ok(v) => println!("  [{section:<8}] {name:<34} {v}"),
        Err(why) => println!("  [{section:<8}] {name:<34} -- ({why})"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        let secs = args
            .iter()
            .position(|a| a == "--seconds")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10);
        let module_dir = args
            .iter()
            .position(|a| a == "--pawn-modules")
            .and_then(|i| args.get(i + 1))
            .cloned();
        probe(secs, module_dir);
    } else {
        eprintln!("Thermora T1 prototype. Run with --probe [--seconds N] [--pawn-modules <dir with IntelMSR.bin>]");
        std::process::exit(2);
    }
}

fn probe(secs: u32, module_dir: Option<String>) {
    println!("Thermora probe, {} s, 1 s poll", secs);
    #[cfg(windows)]
    let mut p = win::Probe::new(module_dir);
    #[cfg(unix)]
    let mut p = linux::Probe::new(module_dir);

    println!("\n== Specs ==");
    p.specs();
    let gpu = nvidia::Nvidia::new();
    gpu.specs();

    let cost0 = p.process_cost();
    let t0 = Instant::now();
    println!("\n== Live readings ==");
    p.sample(); // priming sample: rates need two points
    for i in 1..=secs {
        std::thread::sleep(Duration::from_secs(1));
        println!("\n-- t = {i} s --");
        p.sample();
        gpu.sample();
    }
    let wall = t0.elapsed().as_secs_f64();
    let cost1 = p.process_cost();
    println!("\n== Cost of the polling loop (specs excluded) ==");
    let cpu = (cost1.0 - cost0.0).max(0.0);
    println!(
        "  cpu time {:.3} s over {:.1} s wall = {:.2}% of one core; working set {:.1} MB (peak {:.1} MB)",
        cpu,
        wall,
        100.0 * cpu / wall,
        cost1.1,
        cost1.2
    );
}
