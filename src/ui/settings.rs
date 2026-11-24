//! The Settings screen: language, theme, start with the system, CPU temperature access, data folder, updates,
//! support and about.

use super::access::Action;
use super::{group, page, widgets, App};
use crate::i18n;
use crate::nimvoro::{self, UpdateState};
use crate::theme::{self, Btn, Pal};
use crate::{platform, support, update_ui};
use eframe::egui::{self, Align, Layout, RichText, Ui};

pub struct State {
    pub autostart: bool,
    pub msg: Option<String>,
}

impl State {
    pub fn new() -> Self {
        Self { autostart: platform::autostart_enabled(), msg: None }
    }
}

pub(super) fn switch_row(ui: &mut Ui, p: &Pal, title: &str, hint: &str, value: &mut bool) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(w - 56.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(theme::reg(14.0)).color(p.text));
            if !hint.is_empty() {
                theme::hint(ui, hint, p);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = theme::switch(ui, value, p).changed();
        });
    });
    ui.add_space(8.0);
    changed
}

pub(super) fn combo_row<T: PartialEq + Copy>(ui: &mut Ui, p: &Pal, title: &str, id: &str, current: T, options: &[(T, String)], width: f32) -> Option<T> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::reg(14.0)).color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = options.iter().find(|o| o.0 == current).map(|o| o.1.as_str()).unwrap_or("");
            widgets::combo(id, label, width).show_ui(ui, |ui| {
                for (v, name) in options {
                    if ui.selectable_label(*v == current, RichText::new(name).font(theme::reg(13.5))).clicked() {
                        picked = Some(*v);
                    }
                }
            });
        });
    });
    ui.add_space(8.0);
    picked
}

impl App {
    pub(super) fn settings_view(&mut self, ui: &mut Ui, p: &Pal) {
        let ctx = ui.ctx().clone();
        let t = self.s();
        let lang = self.lang;
        let mut run: Option<Action> = None;
        let mut open_dialog = false;
        page(ui, p, t.nav_settings, None, |_| {}, |ui| {
            // ── General
            group(ui, p, t.group_general, |ui| {
                let langs: Vec<(i18n::Lang, String)> = i18n::Lang::ALL.iter().map(|l| (*l, l.native_name().to_string())).collect();
                if let Some(l) = combo_row(ui, p, i18n::language_label(lang), "language", lang, &langs, 150.0) {
                    self.lang = l;
                    self.cfg.language = Some(l.code().to_string());
                    self.touch_cfg(&ctx);
                }
                let themes: Vec<(theme::ThemeMode, String)> = theme::ThemeMode::ALL.iter().map(|m| (*m, i18n::theme_name(*m, lang).to_string())).collect();
                if let Some(m) = combo_row(ui, p, i18n::theme_label(lang), "theme", self.cfg.theme, &themes, 150.0) {
                    self.cfg.theme = m;
                    theme::apply(&ctx, m);
                    self.touch_cfg(&ctx);
                }
                let mut auto = self.settings.autostart;
                if switch_row(ui, p, t.autostart, t.autostart_hint, &mut auto) {
                    let ok = platform::set_autostart(auto);
                    self.settings.autostart = platform::autostart_enabled();
                    self.cfg.autostart = self.settings.autostart;
                    self.settings.msg = if ok && self.settings.autostart == auto { None } else { Some(t.autostart_failed.to_string()) };
                    if self.settings.autostart {
                        if !crate::ipc::is_running("bg") && !self.demo {
                            platform::spawn_background();
                        }
                    } else {
                        crate::stop_background();
                    }
                    self.touch_cfg(&ctx);
                }
                if let Some(m) = &self.settings.msg {
                    ui.label(RichText::new(m).font(theme::reg(12.0)).color(p.negative));
                }
            });

            // ── CPU temperature access (Windows, Intel)
            if super::access::offered(self.live.cpu.vendor) || self.access.enabled {
                group(ui, p, t.cpu_access_title, |ui| {
                    let busy = self.access.busy.is_some();
                    let (state_text, ok) = if self.access.enabled { (t.cpu_access_on, true) } else { (t.cpu_access_off, false) };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, if ok { p.positive } else { p.muted });
                        ui.label(RichText::new(state_text).font(theme::reg(14.0)).color(p.text));
                    });
                    if !self.access.enabled {
                        ui.add_space(4.0);
                        theme::hint(ui, t.cpu_access_hint, p);
                    }
                    if let Some(v) = &self.access.pawn {
                        ui.add_space(4.0);
                        theme::meta(ui, &format!("PawnIO {v}. {}", t.pawn_hint), p);
                    }
                    if self.access.update_needed {
                        ui.add_space(10.0);
                        theme::hint(ui, t.helper_update_hint, p);
                    }
                    if busy {
                        ui.add_space(8.0);
                        ui.label(RichText::new(t.enable_working).font(theme::reg(13.0)).color(p.text2));
                    }
                    if let Some((m, good)) = &self.access.msg {
                        ui.add_space(8.0);
                        ui.add(egui::Label::new(RichText::new(m).font(theme::reg(13.0)).color(if *good { p.positive } else { p.negative })).wrap());
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if !self.access.enabled {
                            if widgets::btn_enabled(ui, t.enable_btn, Btn::Primary, !busy, p).clicked() {
                                open_dialog = true;
                            }
                        } else {
                            if self.access.update_needed && widgets::btn_enabled(ui, t.helper_update_btn, Btn::Primary, !busy, p).clicked() {
                                run = Some(Action::Refresh);
                            }
                            if widgets::btn_enabled(ui, t.disable_btn, Btn::Secondary, !busy, p).clicked() {
                                run = Some(Action::Disable);
                            }
                        }
                    });
                    if self.access.enabled {
                        ui.add_space(6.0);
                        theme::hint(ui, t.disable_hint, p);
                    }
                });
            }

            if cfg!(windows) && !matches!(self.live.cpu.vendor, crate::model::CpuVendor::Intel) && self.live.ready {
                group(ui, p, t.cpu_access_title, |ui| {
                    theme::hint(ui, t.enable_amd, p);
                });
            }

            // ── Data
            group(ui, p, t.group_data, |ui| {
                // A test or screenshot folder is shown as the usual place, so screenshots do not carry a temporary path.
                let shown = if crate::config::is_test_instance() { if cfg!(windows) { r"%APPDATA%\Thermora".to_string() } else { "~/.config/Thermora".to_string() } } else { self.dir.display().to_string() };
                ui.label(RichText::new(shown).font(theme::mono(12.0)).color(p.muted));
                ui.add_space(8.0);
                if widgets::btn(ui, t.open_data_folder, Btn::Secondary, p).clicked() {
                    open_folder(&self.dir);
                }
            });

            // ── Updates
            let u = nimvoro::strings(lang);
            group(ui, p, u.updates, |ui| {
                let mut v = self.cfg.check_updates;
                if switch_row(ui, p, u.check_on_start, u.check_on_start_hint, &mut v) {
                    self.cfg.check_updates = v;
                    self.touch_cfg(&ctx);
                }
                if !update_ui::progress(ui, &self.updater, lang, p) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(self.updater.status_text(lang)).font(theme::reg(13.0)).color(p.text2));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if let UpdateState::Available { .. } = self.updater.state() {
                                if widgets::btn(ui, u.update_btn, Btn::Primary, p).clicked() {
                                    let saved = self.flush();
                                    if self.updater.apply_if(saved) {
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                    }
                                }
                            }
                            if widgets::btn(ui, u.check_now, Btn::Secondary, p).clicked() {
                                let c = ctx.clone();
                                self.updater.check(move || c.request_repaint());
                            }
                        });
                    });
                }
            });

            // ── Support the author
            ui.add_space(16.0);
            support::support_section(ui, lang);

            // ── About
            group(ui, p, t.about, |ui| {
                ui.horizontal(|ui| {
                    widgets::app_icon(ui, 32.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Thermora").font(theme::medium(15.0)).color(p.text));
                        theme::meta(ui, &format!("{} {}", t.version, env!("CARGO_PKG_VERSION")), p);
                    });
                });
                ui.add_space(8.0);
                theme::hint(ui, t.privacy_note, p);
                ui.add_space(8.0);
                if widgets::btn(ui, crate::REPO_URL, Btn::Link, p).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(crate::REPO_URL));
                }
            });
        });
        if open_dialog {
            self.open_enable_dialog();
        }
        if let Some(a) = run {
            self.run_access(&ctx, a);
        }
    }
}

/// Opens a folder in the file manager.
pub fn open_folder(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new(if cfg!(windows) { "explorer.exe" } else { "xdg-open" }).arg(dir).spawn();
}
