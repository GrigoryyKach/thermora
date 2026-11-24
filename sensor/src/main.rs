#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! ThermoraSensor: the small elevated helper of Thermora (Windows only).
//!
//! It does exactly three things: opens the PawnIO driver (already installed by the user's consent), asks the signed
//! `IntelMSR` module to READ a few Intel registers (temperatures, package energy), and writes one text line per
//! second to a named pipe that only the installing user (and SYSTEM) can open. It has no commands, never writes
//! any register and never touches anything else. It is meant to stay this small and to change rarely: it has its
//! own version, and Thermora refreshes the copy in Program Files only when this file's hash changes.
//!
//! Started on demand by the scheduled task "Thermora Sensor" (run with highest privileges). It leaves by itself
//! 60 s after the last reader went away.

#[cfg(windows)]
#[allow(dead_code)]
mod proto;

#[cfg(windows)]
mod imp;

fn main() {
    #[cfg(windows)]
    imp::run();
    #[cfg(not(windows))]
    eprintln!("ThermoraSensor is Windows only.");
}
