//! The CPU temperature access in the window: its state, the consent dialog that names the driver, and the three
//! elevated actions (enable, refresh the helper, disable). The actual work is in `setup.rs`; here only the
//! window's side: asking, waiting without blocking the UI, and showing the outcome.

use super::{widgets, App};
use crate::model::CpuVendor;
use crate::theme::{self, Btn, Pal};
use eframe::egui::{self, Align2, Area, Color32, Frame, Id, Margin, Order, RichText, Sense, Stroke, Ui};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Enable,
    Refresh,
    Disable,
}

pub enum Msg {
    Status { enabled: bool, update: bool, pawn: Option<String> },
    Done(Action, Result<(), String>, bool),
}

pub struct State {
    pub enabled: bool,
    pub update_needed: bool,
    pub pawn: Option<String>,
    pub dialog: bool,
    pub busy: Option<Action>,
    pub msg: Option<(String, bool)>,
    started: bool,
    rx: Option<Receiver<Msg>>,
}

impl State {
    pub fn new(demo: bool) -> Self {
        let dialog = crate::config::is_test_instance() && std::env::var("THERMORA_SHOT_DIALOG").is_ok_and(|v| v == "1");
        Self { enabled: demo && std::env::var("THERMORA_DEMO_NOACCESS").is_err(), update_needed: false, pawn: None, dialog, busy: None, msg: None, started: demo, rx: None }
    }
}

/// Is the access something this system can offer: Windows, and an Intel processor (the helper reads Intel registers).
pub fn offered(vendor: CpuVendor) -> bool {
    cfg!(windows) && vendor == CpuVendor::Intel
}

impl App {
    /// Polls the background checks. Cheap; called every frame.
    pub(super) fn access_tick(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            if !self.access.started {
                self.access.started = true;
                self.check_access(ctx);
            }
            let mut msgs = Vec::new();
            if let Some(rx) = &self.access.rx {
                while let Ok(m) = rx.try_recv() {
                    msgs.push(m);
                }
            }
            for m in msgs {
                match m {
                    Msg::Status { enabled, update, pawn } => {
                        self.access.enabled = enabled;
                        self.access.update_needed = update;
                        self.access.pawn = pawn;
                        crate::sensors::link::set_enabled(enabled);
                    }
                    Msg::Done(action, res, declined) => {
                        self.access.busy = None;
                        let t = self.s();
                        self.access.msg = Some(match (res, declined) {
                            (Ok(()), _) => (if action == Action::Disable { t.disabled_ok } else { t.enable_ok }.to_string(), true),
                            (Err(_), true) => (t.enable_declined.to_string(), false),
                            (Err(e), false) => (format!("{}: {e}", t.enable_failed), false),
                        });
                        self.check_access(ctx);
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    #[cfg(windows)]
    fn check_access(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.access.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let enabled = crate::setup::is_enabled();
            let update = enabled && crate::setup::helper_needs_update();
            let _ = tx.send(Msg::Status { enabled, update, pawn: crate::setup::pawnio_version() });
            ctx.request_repaint();
        });
    }

    /// Runs one of the elevated actions on a thread; the window stays alive while Windows asks.
    pub(super) fn run_access(&mut self, ctx: &egui::Context, action: Action) {
        #[cfg(windows)]
        {
            if self.access.busy.is_some() {
                return;
            }
            self.access.busy = Some(action);
            if action == Action::Disable {
                // Stop this process from starting the helper again through its task while it is being removed.
                crate::sensors::link::set_enabled(false);
            }
            self.access.msg = None;
            let (tx, rx) = channel();
            self.access.rx = Some(rx);
            let ctx = ctx.clone();
            let file = self.dir.join("setup-result.txt");
            std::thread::spawn(move || {
                let flag = match action {
                    Action::Enable => "--setup-sensor",
                    Action::Refresh => "--refresh-sensor",
                    Action::Disable => "--remove-sensor",
                };
                let _ = std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new(".")));
                let r = crate::setup::run_elevated(flag, &file);
                let _ = std::fs::remove_file(&file);
                let (res, declined) = match r {
                    crate::setup::Outcome::Ok => (Ok(()), false),
                    crate::setup::Outcome::Declined => (Err(String::new()), true),
                    crate::setup::Outcome::Failed(e) => (Err(e), false),
                };
                let _ = tx.send(Msg::Done(action, res, declined));
                ctx.request_repaint();
            });
        }
        #[cfg(not(windows))]
        let _ = (ctx, action);
    }

    /// The consent dialog: it names the driver and what the helper does before anything is installed.
    pub(super) fn enable_dialog(&mut self, ctx: &egui::Context, p: &Pal) {
        if !self.access.dialog {
            return;
        }
        let t = self.s();
        let screen = ctx.content_rect();
        let mut start = false;
        let mut cancel = false;
        let busy = self.access.busy.is_some();
        Area::new(Id::unique("dialog-dim")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(if p.dark { 150 } else { 90 }));
            let _ = resp;
        });
        let w = (screen.width() - 80.0).clamp(360.0, 560.0);
        Area::new(Id::unique("dialog")).order(Order::Tooltip).anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.line_hi)).corner_radius(theme::R_WINDOW).inner_margin(Margin::same(24)).shadow(theme::shadow_for(p)).show(ui, |ui| {
                ui.set_width(w);
                ui.label(RichText::new(t.enable_title).font(theme::medium(20.0)).color(p.text));
                ui.add_space(10.0);
                ui.add(egui::Label::new(RichText::new(t.enable_body).font(theme::reg(14.0)).color(p.text2)).wrap());
                ui.add_space(12.0);
                for fact in t.enable_facts {
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), Sense::hover());
                        ui.painter().circle_filled(dot.center() + egui::vec2(-2.0, 1.0), 2.5, p.accent);
                        ui.add(egui::Label::new(RichText::new(fact).font(theme::reg(13.5)).color(p.text)).wrap());
                    });
                    ui.add_space(6.0);
                }
                if busy {
                    ui.add_space(8.0);
                    ui.label(RichText::new(t.enable_working).font(theme::reg(13.0)).color(p.text2));
                }
                if let Some((m, ok)) = &self.access.msg {
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(m).font(theme::reg(13.0)).color(if *ok { p.positive } else { p.negative })).wrap());
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if widgets::btn_enabled(ui, t.enable_agree, Btn::Primary, !busy && !self.access.enabled, p).clicked() {
                        start = true;
                    }
                    if widgets::btn_enabled(ui, if self.access.enabled { t.close } else { t.enable_cancel }, Btn::Secondary, !busy, p).clicked() {
                        cancel = true;
                    }
                });
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !busy {
            cancel = true;
        }
        if start {
            self.run_access(ctx, Action::Enable);
        }
        if cancel {
            self.access.dialog = false;
        }
        if self.access.enabled && self.access.msg.as_ref().is_some_and(|m| m.1) && !busy {
            // Done: the dialog closes by itself after a success; the result stays in Settings.
            self.access.dialog = false;
        }
    }

    pub(super) fn open_enable_dialog(&mut self) {
        self.access.msg = None;
        self.access.dialog = true;
    }
}

/// A reason text for a dash. Returns the words of the tooltip.
pub fn why_text(t: &crate::strings::S, why: crate::model::Why) -> &'static str {
    use crate::model::Why::*;
    match why {
        NeedsCpuAccess => t.why_needs_cpu,
        HelperStarting => t.why_starting,
        HelperUnsupported => t.why_unsupported,
        NotReported => t.why_not_reported,
        NeedsRoot => t.why_needs_root,
    }
}

#[allow(dead_code)]
fn _unused(_: &mut Ui) {}
