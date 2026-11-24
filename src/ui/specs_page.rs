//! The System specs page: the device list on the left, grouped like NZXT CAM, and a details panel that slides in
//! on the right when a row is clicked. Specs are gathered once on a background thread (a skeleton shows meanwhile).

use super::widgets::{self, Icon};
use super::App;
use crate::specs::{self, Component, Group, Specs};
use crate::theme::{self, Btn, Pal};
use eframe::egui::{self, vec2, Align2, Margin, RichText, Sense, Stroke, StrokeKind, Ui};
use std::sync::mpsc::{channel, Receiver};

pub struct State {
    pub data: Option<Specs>,
    pub loading: bool,
    pub selected: Option<usize>,
    /// The last selected row: its content stays while the panel slides out.
    shown: Option<usize>,
    rx: Option<Receiver<Specs>>,
    shot_select: Option<usize>,
}

impl State {
    pub fn new(demo: bool) -> Self {
        let select = if crate::config::is_test_instance() { std::env::var("THERMORA_SHOT_SELECT").ok().and_then(|v| v.parse().ok()) } else { None };
        Self { data: demo.then(crate::sensors::demo::demo_specs), loading: false, selected: select, shown: select, rx: None, shot_select: select }
    }
}

fn group_icon(g: Group) -> Icon {
    match g {
        Group::Cpu => Icon::Cpu,
        Group::Gpu => Icon::Gpu,
        Group::Board => Icon::Board,
        Group::Ram => Icon::Ram,
        Group::Storage => Icon::Storage,
        Group::Audio => Icon::Audio,
        Group::Network => Icon::Network,
    }
}

fn collect() -> Specs {
    #[cfg(windows)]
    return crate::sensors::win::collect_specs();
    #[cfg(unix)]
    return crate::sensors::linux::collect_specs();
}

impl App {
    /// Gathers the specs on a thread (once on first open, again on Refresh).
    pub(super) fn specs_load(&mut self, ctx: &egui::Context) {
        if self.specs.loading || self.demo {
            return;
        }
        self.specs.loading = true;
        let (tx, rx) = channel();
        self.specs.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(collect());
            ctx.request_repaint();
        });
    }

    pub(super) fn specs_enter(&mut self, ctx: &egui::Context) {
        if self.specs.data.is_none() {
            self.specs_load(ctx);
        }
    }

    pub(super) fn specs_tick(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.specs.rx {
            if let Ok(d) = rx.try_recv() {
                self.specs.data = Some(d);
                self.specs.loading = false;
                self.specs.rx = None;
                if let Some(i) = self.specs.shot_select.take() {
                    self.specs.selected = Some(i);
                    self.specs.shown = Some(i);
                }
            }
        }
        if self.nav == super::Nav::Specs && self.specs.selected.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.access.dialog {
            self.specs.selected = None;
        }
    }

    pub(super) fn specs_view(&mut self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let ctx = ui.ctx().clone();
        let mut refresh = false;
        let mut copy = false;
        let mut toggle: Option<usize> = None;
        let mut close = false;

        // Header (the same look as `page`), then a body that does not scroll as a whole.
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(t.nav_specs).font(theme::medium(24.0)).color(p.text));
                theme::hint(ui, t.specs_subtitle, p);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::btn_enabled(ui, t.copy_specs, Btn::Secondary, self.specs.data.is_some(), p).clicked() {
                    copy = true;
                }
                if widgets::icon_button(ui, Icon::Refresh, 34.0, p).on_hover_text(t.refresh).clicked() {
                    refresh = true;
                }
            });
        });
        ui.add_space(16.0);

        let anim = ctx.animate_bool_with_time(egui::Id::unique("specs-panel"), self.specs.selected.is_some(), 0.24);
        let ease = theme::ease(anim);
        let avail = ui.available_size();
        let panel_full = (avail.x * 0.42).clamp(320.0, 420.0);
        let panel_w = panel_full * ease;
        let gap = if ease > 0.001 { 16.0 * ease } else { 0.0 };
        let list_w = (avail.x - panel_w - gap).max(260.0);
        let h = avail.y.max(200.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(vec2(list_w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(list_w);
                ui.set_height(h);
                egui::ScrollArea::vertical().id_salt("spec-list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(list_w - 12.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    theme::section(ui, t.device_list, p);
                    ui.add_space(4.0);
                    match &self.specs.data {
                        None => skeleton(ui, p, t.loading_specs),
                        Some(data) if data.comps.is_empty() => {
                            ui.label(RichText::new(t.nothing_here).font(theme::reg(14.0)).color(p.text2));
                        }
                        Some(data) => {
                            for g in Group::ALL {
                                let items: Vec<(usize, &Component)> = data.of(g).collect();
                                if items.is_empty() {
                                    continue;
                                }
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                                    widgets::icon(ui.painter(), group_icon(g), r.center(), 15.0, p.muted);
                                    ui.label(RichText::new(t.group_name(g)).font(theme::medium(13.0)).color(p.text2));
                                });
                                for (idx, c) in items {
                                    if list_row(ui, p, c, self.specs.selected == Some(idx)).clicked() {
                                        toggle = Some(idx);
                                    }
                                }
                            }
                        }
                    }
                });
            });
            if ease > 0.001 {
                ui.add_space(gap);
                let (rect, _) = ui.allocate_exact_size(vec2(panel_w, h), Sense::hover());
                let shown = self.specs.shown.and_then(|i| self.specs.data.as_ref().and_then(|d| d.comps.get(i)));
                if let Some(c) = shown {
                    // The panel keeps its full size and is clipped, so the content slides in instead of squeezing.
                    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(rect.min, vec2(panel_full, h))).layout(egui::Layout::top_down(egui::Align::Min)));
                    cui.set_clip_rect(rect.intersect(ui.clip_rect()));
                    close |= self.details_panel(&mut cui, p, c, h);
                }
            }
        });

        if let Some(i) = toggle {
            if self.specs.selected == Some(i) {
                self.specs.selected = None;
            } else {
                self.specs.selected = Some(i);
                self.specs.shown = Some(i);
            }
        }
        if close {
            self.specs.selected = None;
        }
        if refresh {
            self.specs.data = None;
            self.specs.selected = None;
            self.specs_load(&ctx);
        }
        if copy {
            if let Some(d) = &self.specs.data {
                ctx.copy_text(specs::copy_text(d, self.lang));
                self.notify(&ctx, t.copied.to_string());
            }
        }
    }

    /// The side panel: the model, then label/value pairs. Returns true when its close button was clicked.
    fn details_panel(&self, ui: &mut Ui, p: &Pal, c: &Component, h: f32) -> bool {
        let t = self.s();
        let mut close = false;
        theme::card(p).inner_margin(Margin::same(18)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(h - 38.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::hover());
                widgets::icon(ui.painter(), group_icon(c.group), r.center(), 17.0, p.accent);
                ui.label(RichText::new(t.group_name(c.group)).font(theme::medium(13.0)).color(p.text2));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 28.0, p).on_hover_text(t.close).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(&c.title).font(theme::medium(18.0)).color(p.text)).wrap());
            if !c.sub.is_empty() {
                ui.add_space(2.0);
                theme::hint(ui, &c.sub, p);
            }
            ui.add_space(12.0);
            theme::hairline(ui, p);
            ui.add_space(8.0);
            theme::section(ui, t.details, p);
            egui::ScrollArea::vertical().id_salt("spec-details").auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(6.0);
                for (f, v) in &c.rows {
                    ui.add_space(6.0);
                    ui.label(RichText::new(t.field_name(*f)).font(theme::reg(12.0)).color(p.muted));
                    let col = if v == specs::DASH { p.muted } else { p.text };
                    ui.add(egui::Label::new(RichText::new(v).font(theme::mono(13.5)).color(col)).wrap());
                }
            });
        });
        close
    }
}

/// A list row: the model and one line under it. The selected row is highlighted in the accent.
fn list_row(ui: &mut Ui, p: &Pal, c: &Component, selected: bool) -> egui::Response {
    let h = if c.sub.is_empty() { 44.0 } else { 56.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let hover = theme::fade(ui, resp.id.with("hover"), resp.hovered() && !selected);
    if selected {
        ui.painter().rect(rect, theme::R_CONTROL as f32, theme::alpha(p.accent, if p.dark { 34 } else { 28 }), Stroke::new(1.0, p.accent), StrokeKind::Inside);
    } else {
        ui.painter().rect(rect, theme::R_CONTROL as f32, theme::alpha(p.card, (hover * 255.0) as u8), Stroke::new(1.0, p.line), StrokeKind::Inside);
    }
    let left = rect.left() + 14.0;
    let w = rect.width() - 52.0;
    let tf = theme::medium(14.0);
    let title = widgets::fit_text(ui.painter(), &c.title, &tf, w);
    let ty = if c.sub.is_empty() { rect.center().y } else { rect.top() + 20.0 };
    ui.painter().text(egui::pos2(left, ty), Align2::LEFT_CENTER, title, tf, if selected { p.text } else { p.text });
    if !c.sub.is_empty() {
        let sf = theme::reg(12.0);
        let sub = widgets::fit_text(ui.painter(), &c.sub, &sf, w);
        ui.painter().text(egui::pos2(left, rect.top() + 39.0), Align2::LEFT_CENTER, sub, sf, p.muted);
    }
    widgets::icon(ui.painter(), Icon::Chevron, egui::pos2(rect.right() - 20.0, rect.center().y), 14.0, if selected { p.accent } else { p.muted });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Flat placeholder rows while the hardware is read (no pulsing, no shimmer).
fn skeleton(ui: &mut Ui, p: &Pal, text: &str) {
    ui.label(RichText::new(text).font(theme::reg(13.0)).color(p.muted));
    ui.add_space(6.0);
    for i in 0..7 {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        ui.painter().rect(rect, theme::R_CONTROL as f32, p.card, Stroke::new(1.0, p.line), StrokeKind::Inside);
        let w = [0.55, 0.42, 0.62, 0.38, 0.5, 0.58, 0.45][i];
        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min + vec2(14.0, 13.0), vec2(rect.width() * w, 10.0)), 4.0, p.line_hi);
        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min + vec2(14.0, 32.0), vec2(rect.width() * w * 0.6, 8.0)), 4.0, p.line);
    }
}
