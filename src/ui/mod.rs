//! The window: state, the per-frame logic, navigation. Pages live in the sibling modules.
//!
//! The window process owns its own sensor thread (a hidden tray process, when there is one, has another): readings
//! are published into `shared` once a second and drawn from there.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Frame, Id, Margin, Order, Rect, Sense, Stroke, Ui};

use crate::bus::{self, Cmd};
use crate::config::{Config, Window};
use crate::i18n::{self, Lang};
use crate::model::{History, Live};
use crate::nimvoro::Updater;
use crate::sensors::{self, Shared};
use crate::strings::{self, S};
use crate::theme::{self, Pal};

mod access;
mod monitor;
mod settings;
mod specs_page;
pub mod widgets;

/// The app accent: warm orange (heat), away from Random Wheel's red-orange and Save URLs' amber.
pub const ACCENT: theme::Accent = theme::Accent::new(0xf5813f, 0xc2560c, 0x141415, 0xffffff);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Monitor,
    Specs,
    Settings,
}

impl Nav {
    #[cfg(test)]
    const ALL: [Nav; 3] = [Nav::Monitor, Nav::Specs, Nav::Settings];

    fn from_name(s: &str) -> Option<Nav> {
        Some(match s {
            "monitor" => Nav::Monitor,
            "specs" => Nav::Specs,
            "settings" => Nav::Settings,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Nav::Monitor => "monitor",
            Nav::Specs => "specs",
            Nav::Settings => "settings",
        }
    }
}

pub struct Toast {
    pub text: String,
    pub until: f64,
}

/// Screenshot driver (set by `THERMORA_SHOT_DIR`): walks through the screens, saves a PNG of each and quits.
struct Shots {
    dir: PathBuf,
    prefix: String,
    queue: Vec<Nav>,
    wait: u32,
    wait_frames: u32,
    asked: bool,
    done: Arc<AtomicBool>,
}

pub struct App {
    pub cfg: Config,
    pub lang: Lang,
    pub dir: PathBuf,
    pub shared: Shared,
    pub live: Live,
    pub hist: History,
    pub nav: Nav,
    pub updater: Updater,
    pub demo: bool,
    pub toast: Option<Toast>,
    pub specs: specs_page::State,
    pub settings: settings::State,
    pub access: access::State,
    /// Readings that showed a value at least once this session; a reading the hardware never gives stays hidden.
    pub seen: std::cell::RefCell<std::collections::HashSet<String>>,
    visible: Arc<AtomicBool>,
    cfg_dirty: Option<f64>,
    shots: Option<Shots>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, mut cfg: Config, dir: PathBuf, demo: bool) -> Self {
        // Screenshots and tests may force the language and theme without touching the saved settings.
        if crate::config::is_test_instance() {
            if let Ok(l) = std::env::var("THERMORA_LANG") {
                cfg.language = Some(l);
            }
            if let Some(m) = std::env::var("THERMORA_THEME").ok().and_then(|t| match t.as_str() {
                "dark" => Some(theme::ThemeMode::Dark),
                "light" => Some(theme::ThemeMode::Light),
                _ => None,
            }) {
                cfg.theme = m;
            }
        }
        theme::set_accent(ACCENT);
        theme::install(&cc.egui_ctx, cfg.theme);
        cc.egui_ctx.all_styles_mut(|s| s.spacing.interact_size.y = widgets::ROW_BTN_H);
        let lang = i18n::resolve(cfg.language.as_deref());
        let mut cfg_dirty = None;
        if cfg.language.as_deref() != Some(lang.code()) {
            cfg.language = Some(lang.code().to_string());
            cfg_dirty = Some(0.0);
        }
        bus::init(cc.egui_ctx.clone());
        let shared = sensors::shared();
        let visible = Arc::new(AtomicBool::new(true));
        let ctx = cc.egui_ctx.clone();
        sensors::spawn(Box::new(move || ctx.request_repaint()), shared.clone(), demo, visible.clone());
        let updater = Updater::new("thermora", crate::REPO_URL.trim_start_matches("https://github.com/"), env!("CARGO_PKG_VERSION")).relaunch_args(&["--minimized"]).before_restart(crate::stop_background);
        if cfg.check_updates && !crate::config::is_test_instance() {
            let ctx = cc.egui_ctx.clone();
            updater.check(move || ctx.request_repaint());
        }
        let shots = std::env::var_os("THERMORA_SHOT_DIR").map(|d| {
            let names = std::env::var("THERMORA_SHOT_NAVS").unwrap_or_else(|_| "monitor,specs,settings".into());
            Shots {
                dir: PathBuf::from(d),
                prefix: std::env::var("THERMORA_SHOT_PREFIX").unwrap_or_else(|_| "shot".into()),
                queue: names.split(',').filter_map(Nav::from_name).collect(),
                wait: 0,
                wait_frames: std::env::var("THERMORA_SHOT_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(45),
                asked: false,
                done: Arc::new(AtomicBool::new(false)),
            }
        });
        let first = shots.as_ref().and_then(|s| s.queue.first().copied()).unwrap_or(Nav::Monitor);
        let mut app = Self {
            lang,
            dir,
            shared,
            live: Live::default(),
            hist: History::new(),
            nav: first,
            updater,
            demo,
            toast: None,
            specs: specs_page::State::new(demo),
            settings: settings::State::new(),
            access: access::State::new(demo),
            seen: Default::default(),
            visible,
            cfg_dirty,
            shots,
            cfg,
        };
        if app.nav == Nav::Specs {
            app.specs_enter(&cc.egui_ctx);
        }
        app
    }

    pub fn s(&self) -> &'static S {
        strings::TABLE.get(self.lang)
    }

    // ───────────────────────── settings ─────────────────────────

    pub fn touch_cfg(&mut self, ctx: &egui::Context) {
        self.cfg_dirty = Some(ctx.input(|i| i.time));
        ctx.request_repaint_after(Duration::from_millis(450));
    }

    pub fn flush(&mut self) -> bool {
        if self.cfg_dirty.take().is_some() {
            return self.cfg.save_to(&self.dir);
        }
        true
    }

    pub fn notify(&mut self, ctx: &egui::Context, text: String) {
        let now = ctx.input(|i| i.time);
        self.toast = Some(Toast { text, until: now + 4.0 });
    }

    pub fn go(&mut self, ctx: &egui::Context, nav: Nav) {
        if self.nav != nav && nav == Nav::Specs {
            self.specs_enter(ctx);
        }
        self.nav = nav;
    }

    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, minimized, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect, i.viewport().minimized, i.viewport().maximized));
        self.visible.store(minimized != Some(true), Ordering::Relaxed);
        if minimized == Some(true) || maximized == Some(true) {
            return;
        }
        if let (Some(inner), Some(outer)) = (inner, outer) {
            let w = Window { w: inner.width(), h: inner.height(), x: Some(outer.min.x), y: Some(outer.min.y) };
            let same = self.cfg.window.is_some_and(|c| (c.w - w.w).abs() < 1.0 && (c.h - w.h).abs() < 1.0 && c.x == w.x && c.y == w.y);
            if !same && w.w > 300.0 && w.h > 300.0 && self.shots.is_none() {
                self.cfg.window = Some(w);
                self.touch_cfg(ctx);
            }
        }
    }

    fn drive_screenshots(&mut self, ctx: &egui::Context) {
        let nav_name = self.nav.name();
        let Some(sh) = self.shots.as_mut() else { return };
        ctx.request_repaint();
        if sh.done.swap(false, Ordering::Relaxed) {
            sh.queue.remove(0);
            sh.asked = false;
            sh.wait = 0;
            match sh.queue.first().copied() {
                Some(n) => self.go(ctx, n),
                None => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
            return;
        }
        sh.wait += 1;
        if sh.wait > sh.wait_frames && !sh.asked {
            sh.asked = true;
            let path = sh.dir.join(format!("{}-{}.png", sh.prefix, nav_name));
            let _ = std::fs::create_dir_all(&sh.dir);
            let done = sh.done.clone();
            ctx.request_screenshot(move |img| {
                let mut raw = Vec::with_capacity(img.pixels.len() * 4);
                for px in &img.pixels {
                    raw.extend_from_slice(&[px.r(), px.g(), px.b(), px.a()]);
                }
                let _ = image::save_buffer(path, &raw, img.size[0] as u32, img.size[1] as u32, image::ColorType::Rgba8);
                done.store(true, Ordering::Relaxed);
            });
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = ctx.input(|i| i.time);
        for cmd in bus::drain() {
            match cmd {
                Cmd::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }
        if self.updater.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.updater.should_exit() {
            self.flush();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Ok(s) = self.shared.lock() {
            self.live = s.live.clone();
            if let Some(h) = &s.history {
                self.hist = h.clone();
            }
        }
        // A slow tick keeps the window honest when the sensor thread is waiting.
        ctx.request_repaint_after(Duration::from_millis(1100));
        if self.cfg_dirty.is_some_and(|d| t - d > 0.4) {
            self.flush();
        }
        if self.toast.as_ref().is_some_and(|x| t > x.until) {
            self.toast = None;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.flush();
        }
        self.access_tick(ctx);
        self.specs_tick(ctx);
        self.track_window(ctx);
        self.drive_screenshots(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Pal::of(ui.visuals().dark_mode);
        egui::Panel::left("sidebar")
            .exact_size(250.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.sidebar).stroke(Stroke::NONE).inner_margin(Margin { left: 12, right: 12, top: 16, bottom: 12 }))
            .show(ui, |ui| self.sidebar(ui, &p));

        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, p.bg);
        ui.painter().vline(area.left(), area.y_range(), Stroke::new(1.0, p.line));
        let inner = Rect::from_min_max(area.min + egui::vec2(28.0, 24.0), area.max - egui::vec2(28.0, 8.0));
        let mut cui = ui.new_child(egui::UiBuilder::new().id_salt("central").max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        cui.set_clip_rect(area);
        match self.nav {
            Nav::Monitor => self.monitor_view(&mut cui, &p),
            Nav::Specs => self.specs_view(&mut cui, &p),
            Nav::Settings => self.settings_view(&mut cui, &p),
        }
        self.enable_dialog(&ctx, &p);
        self.toast_ui(&ctx, &p);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.flush();
    }
}

// ───────────────────────── shared UI pieces ─────────────────────────

/// A page: a 24 px title with an optional subtitle, a right-aligned header, then a scrolling body in a column
/// no wider than 1120 px.
pub fn page(ui: &mut Ui, p: &Pal, title: &str, subtitle: Option<&str>, right: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui)) {
    let avail = ui.available_width();
    let avail_h = ui.available_height();
    let col_w = (avail - 14.0).clamp(300.0, 1120.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((avail - 14.0 - col_w) / 2.0).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(col_w);
            ui.set_height(avail_h);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new(title).font(theme::medium(24.0)).color(p.text));
                    if let Some(s) = subtitle {
                        theme::hint(ui, s, p);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            });
            ui.add_space(16.0);
            let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(y) = test_scroll() {
                area = area.vertical_scroll_offset(y);
            }
            area.show(ui, |ui| {
                ui.set_width(col_w - 12.0);
                body(ui);
                ui.add_space(24.0);
            });
        });
    });
}

/// Screenshots of a long page: `THERMORA_SCROLL=<pixels>` (test folder only) scrolls every page to that offset.
fn test_scroll() -> Option<f32> {
    static V: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| if crate::config::is_test_instance() { std::env::var("THERMORA_SCROLL").ok().and_then(|v| v.parse().ok()) } else { None })
}

/// A titled card: a section title (16 above, 8 below) over a card with 16 px padding.
pub fn group<R>(ui: &mut Ui, p: &Pal, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(16.0);
    theme::section(ui, title, p);
    ui.add_space(8.0);
    theme::card(p)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

impl App {
    fn sidebar(&mut self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (mark, _) = ui.allocate_exact_size(egui::vec2(28.0, 36.0), Sense::hover());
            widgets::mark_glyph(ui.painter(), mark.center(), p.accent);
            ui.label(egui::RichText::new("Thermora").font(theme::medium(18.0)).color(p.text));
        });
        ui.add_space(16.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for (nav, ic, label) in [(Nav::Monitor, widgets::Icon::Monitor, t.nav_monitor), (Nav::Specs, widgets::Icon::Specs, t.nav_specs)] {
            if nav_row(ui, p, ic, label, self.nav == nav).clicked() {
                self.go(&ctx, nav);
            }
        }
        let h = ui.available_height();
        ui.add_space((h - 36.0).max(8.0));
        if nav_row(ui, p, widgets::Icon::Gear, t.nav_settings, self.nav == Nav::Settings).clicked() {
            self.go(&ctx, Nav::Settings);
        }
    }
}

/// One navigation row.
fn nav_row(ui: &mut Ui, p: &Pal, ic: widgets::Icon, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), Sense::click());
    if selected {
        ui.painter().rect(rect, theme::R_CONTROL as f32, widgets::selected_fill(p), Stroke::new(1.0, p.line_hi), egui::StrokeKind::Inside);
        ui.painter().hline(rect.x_range().shrink(8.0), rect.top() + 0.5, Stroke::new(1.0, p.top_light()));
    } else {
        ui.painter().rect_filled(rect, theme::R_CONTROL as f32, widgets::hover_fill(ui, resp.id, resp.hovered(), p));
    }
    let col = if selected { p.text } else { p.text2 };
    widgets::icon(ui.painter(), ic, egui::pos2(rect.left() + 20.0, rect.center().y), 16.0, if selected { p.text } else { p.muted });
    let font = if selected { theme::medium(14.0) } else { theme::reg(14.0) };
    let shown = widgets::fit_text(ui.painter(), label, &font, rect.width() - 52.0);
    ui.painter().text(egui::pos2(rect.left() + 36.0, rect.center().y), egui::Align2::LEFT_CENTER, shown, font, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl App {
    fn toast_ui(&mut self, ctx: &egui::Context, p: &Pal) {
        let Some(toast) = &self.toast else { return };
        let text = toast.text.clone();
        let dark = Pal::dark();
        egui::Area::new(Id::unique("toast")).order(Order::Tooltip).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -22.0)).show(ctx, |ui| {
            Frame::new()
                .fill(dark.raised)
                .stroke(Stroke::new(1.0, dark.line_hi))
                .corner_radius(theme::R_CARD)
                .inner_margin(Margin::symmetric(16, 8))
                .shadow(theme::shadow_for(p))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&text).font(theme::reg(14.0)).color(dark.text));
                });
        });
    }
}

#[allow(dead_code)]
type Unused = Mutex<()>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_readable_in_both_themes() {
        theme::set_accent(ACCENT);
        assert!(theme::accent_problems().is_empty(), "{:?}", theme::accent_problems());
    }

    #[test]
    fn accent_is_distinct_from_neighbours_in_the_suite() {
        fn hue(c: u32) -> f32 {
            let (r, g, b) = (((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0);
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let d = mx - mn;
            let h = if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
            h * 60.0
        }
        // Random Wheel #ec5b45 (hue ~8) and Save URLs #e0a82e (hue ~42): Thermora sits between at ~23.
        let ours = hue(0xf5813f);
        assert!((ours - hue(0xec5b45)).abs() >= 10.0, "too close to Random Wheel");
        assert!((hue(0xe0a82e) - ours).abs() >= 10.0, "too close to Save URLs");
    }

    #[test]
    fn nav_names_round_trip() {
        for n in Nav::ALL {
            assert_eq!(Nav::from_name(n.name()), Some(n));
        }
        assert_eq!(Nav::from_name("nope"), None);
    }
}
