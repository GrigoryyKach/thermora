//! What the tray menu does and the state the background process hands to the tray. Shared by both trays.

use crate::i18n::Lang;
use crate::model::Live;

#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    pub lang: Lang,
    pub tooltip: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Support,
    Exit,
}

pub fn perform(a: Action) {
    match a {
        Action::Open => crate::platform::spawn_ui(),
        Action::Support => crate::support::open(),
        Action::Exit => crate::background::request_quit(),
    }
}

/// `Thermora` and one line with the CPU and GPU temperature, or only what is known. A dash never shows as 0.
pub fn tooltip(live: &Live, lang: Lang) -> String {
    let t = crate::strings::TABLE.get(lang);
    let mut parts: Vec<String> = Vec::new();
    if let Some(Ok(c)) = live.cpu.temp {
        parts.push(format!("{} {:.0} °C", t.tray_cpu, c));
    }
    if let Some(Ok(g)) = live.gpu.as_ref().and_then(|g| g.temp) {
        parts.push(format!("{} {:.0} °C", t.tray_gpu, g));
    }
    if parts.is_empty() {
        "Thermora".to_string()
    } else {
        format!("Thermora\n{}", parts.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn tooltip_shows_known_temperatures_only() {
        let mut l = Live::default();
        assert_eq!(tooltip(&l, Lang::En), "Thermora");
        l.cpu.temp = Some(Err(Why::NeedsCpuAccess));
        l.gpu = Some(GpuLive { temp: Some(Ok(41.4)), ..Default::default() });
        assert_eq!(tooltip(&l, Lang::En), "Thermora\nGPU 41 °C");
        l.cpu.temp = Some(Ok(52.6));
        assert_eq!(tooltip(&l, Lang::Ru), "Thermora\nЦП 53 °C · ГП 41 °C");
    }
}
