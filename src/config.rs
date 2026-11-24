//! Settings (`config.json`) and the data folder.

use crate::theme::ThemeMode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `%APPDATA%\Thermora` (`~/.config/Thermora` on Linux), or the folder from `THERMORA_DATA_DIR` (tests and screenshots).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("THERMORA_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Thermora")
}

/// Held by tests that set or depend on `THERMORA_DATA_DIR` or the autostart override (the environment is shared).
#[cfg(test)]
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Is a data-dir override active? Test instances use their own names for single-instance locks and registry keys.
pub fn is_test_instance() -> bool {
    std::env::var_os("THERMORA_DATA_DIR").is_some_and(|d| !d.is_empty())
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    pub w: f32,
    pub h: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Interface language code (`en`, `ru`, `uk`); `None` until first resolved.
    pub language: Option<String>,
    pub theme: ThemeMode,
    /// Wanted state of "start with the system"; the real state is read from the system. Off by default.
    pub autostart: bool,
    pub check_updates: bool,
    /// The "Enable CPU temperature" banner on the Monitor page was dismissed.
    pub banner_dismissed: bool,
    pub window: Option<Window>,
}

impl Default for Config {
    fn default() -> Self {
        Self { language: None, theme: ThemeMode::System, autostart: false, check_updates: true, banner_dismissed: false, window: None }
    }
}

/// Writes a file in one step: a temporary file next to it, then a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

impl Config {
    pub fn load_from(dir: &Path) -> Self {
        let text = std::fs::read_to_string(dir.join("config.json")).ok();
        text.as_deref().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default()
    }

    pub fn save_to(&self, dir: &Path) -> bool {
        match serde_json::to_vec_pretty(self) {
            Ok(json) => write_atomic(&dir.join("config.json"), &json).is_ok(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("thermora-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn defaults_are_the_contract_defaults() {
        let c = Config::default();
        assert_eq!(c.theme, ThemeMode::System);
        assert!(c.language.is_none());
        assert!(!c.autostart && c.check_updates && !c.banner_dismissed);
    }

    #[test]
    fn settings_round_trip_and_survive_junk() {
        let d = tmp("cfg");
        let mut c = Config::default();
        c.language = Some("uk".into());
        c.theme = ThemeMode::Dark;
        c.banner_dismissed = true;
        assert!(c.save_to(&d));
        assert_eq!(Config::load_from(&d), c);
        std::fs::write(d.join("config.json"), "not json").unwrap();
        assert_eq!(Config::load_from(&d), Config::default());
        // An old file without the newer keys still loads.
        std::fs::write(d.join("config.json"), "\u{feff}{\"theme\":\"light\"}").unwrap();
        assert_eq!(Config::load_from(&d).theme, ThemeMode::Light);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_data_dir_override_is_honoured() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialised by ENV_LOCK; only this module's tests touch the variable.
        unsafe { std::env::set_var("THERMORA_DATA_DIR", "X:/somewhere") };
        assert_eq!(data_dir(), PathBuf::from("X:/somewhere"));
        assert!(is_test_instance());
        unsafe { std::env::remove_var("THERMORA_DATA_DIR") };
        assert!(!is_test_instance());
    }
}
