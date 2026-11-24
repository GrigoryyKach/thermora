//! The Monitor page: a grid of component cards. Ring gauges, tabular numbers, sparklines of the last 60 seconds,
//! temperature colours only for the temperature bands. A missing reading is a dash with the reason in its tooltip.

use super::access::{offered, why_text};
use super::widgets::{self, Icon};
use super::{page, App};
use crate::model::*;
use crate::theme::{self, Btn, Pal};
use eframe::egui::{self, vec2, Align2, Color32, Margin, RichText, Sense, Stroke, Ui};

fn band_color(b: Band, p: &Pal) -> Color32 {
    match b {
        Band::Normal => p.positive,
        Band::Warm => p.warning,
        Band::Hot => p.negative,
    }
}

/// A value that eases to its target: numbers glide instead of jumping.
fn glide(ui: &Ui, key: &str, target: f32) -> f32 {
    ui.ctx().animate_value_with_time(egui::Id::unique(("glide", key)), target, 0.2)
}

/// What a tile shows.
enum Val {
    /// Text and its colour.
    Text(String, Color32),
    Missing(Why),
}

fn from_rd(r: &Option<Rd<f32>>, f: impl Fn(f32) -> Val) -> Val {
    match r {
        Some(Ok(v)) => f(*v),
        Some(Err(w)) => Val::Missing(*w),
        None => Val::Missing(Why::NotReported),
    }
}

impl App {
    /// One small metric: a muted label over a mono value. Returns true when a dash that offers the access was clicked.
    fn tile(&self, ui: &mut Ui, p: &Pal, label: &str, val: Val, band: Option<(Band, &str)>) -> bool {
        let t = self.s();
        let mut clicked = false;
        // A reading this hardware can never give is hidden (it would be a permanent dash). A dash stays only for what
        // needs an action (CPU temperature before Enable) or is just starting.
        match &val {
            Val::Text(..) => {
                self.seen.borrow_mut().insert(label.to_string());
            }
            Val::Missing(Why::NotReported) if !self.seen.borrow().contains(label) => return false,
            _ => {}
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).font(theme::reg(12.0)).color(p.muted));
            match val {
                Val::Text(s, col) => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new(s).font(theme::mono(20.0)).color(col));
                        if let Some((b, name)) = band {
                            ui.label(RichText::new(name).font(theme::reg(12.0)).color(band_color(b, p)));
                        }
                    });
                }
                Val::Missing(why) => {
                    let can_enable = why == Why::NeedsCpuAccess && offered(self.live.cpu.vendor);
                    let r = ui.add(egui::Label::new(RichText::new(crate::specs::DASH).font(theme::mono(20.0)).color(p.muted)).sense(if can_enable { Sense::click() } else { Sense::hover() }));
                    let text = if can_enable { format!("{}. {}", why_text(t, why), t.why_enable) } else { why_text(t, why).to_string() };
                    let r = r.on_hover_text(text);
                    if can_enable && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        clicked = true;
                    }
                }
            }
        });
        clicked
    }

    fn card_header(&self, ui: &mut Ui, p: &Pal, ic: Icon, title: &str, right: &str) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::hover());
            widgets::icon(ui.painter(), ic, r.center(), 16.0, p.accent);
            ui.label(RichText::new(title).font(theme::medium(14.0)).color(p.text));
            if !right.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let shown = widgets::fit_text(ui.painter(), right, &theme::mono(12.0), (ui.available_width() - 8.0).max(40.0));
                    ui.label(RichText::new(shown).font(theme::mono(12.0)).color(p.muted));
                });
            }
        });
        ui.add_space(10.0);
    }

    /// Ring gauge with the percentage in the middle.
    fn ring_gauge(&self, ui: &mut Ui, p: &Pal, key: &str, pct: f32, caption: &str, size: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        let v = glide(ui, key, pct.clamp(0.0, 100.0));
        let thick = (size * 0.075).max(6.0);
        widgets::ring(ui.painter(), rect.center(), size / 2.0 - thick, thick, v / 100.0, p.accent, widgets::track(p));
        ui.painter().text(rect.center() + vec2(0.0, -4.0), Align2::CENTER_CENTER, format!("{:.0}", v), theme::mono(size * 0.27), p.text);
        ui.painter().text(rect.center() + vec2(0.0, size * 0.20), Align2::CENTER_CENTER, caption, theme::reg(12.0), p.muted);
    }

    fn legend(&self, ui: &mut Ui, p: &Pal, items: &[(&str, Color32)]) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.label(RichText::new(self.s().last_60s).font(theme::reg(12.0)).color(p.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (name, col) in items.iter().rev() {
                    ui.label(RichText::new(*name).font(theme::reg(12.0)).color(p.text2));
                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 8.0), Sense::hover());
                    ui.painter().rect_filled(egui::Rect::from_center_size(r.center(), vec2(10.0, 3.0)), 1.5, *col);
                }
            });
        });
    }

    fn chart(&self, ui: &mut Ui, p: &Pal, series: &[(&[f32], Color32)], max: f32, h: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        widgets::sparkline(ui.painter(), rect, series, max, p.line_hi);
    }

    // ───────────────────────── cards ─────────────────────────

    fn cpu_card(&self, ui: &mut Ui, p: &Pal) -> bool {
        let t = self.s();
        let c = &self.live.cpu;
        let kind = SensorKind::from(c.vendor);
        let mut enable = false;
        self.card_header(ui, p, Icon::Cpu, t.card_cpu, "");
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            self.ring_gauge(ui, p, "cpu-load", c.load, t.load, 148.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 30.0;
                    let temp = from_rd(&c.temp, |v| {
                        let g = glide(ui, "cpu-temp", v);
                        let b = band(kind, v);
                        Val::Text(format!("{g:.0} °C"), band_color(b, p))
                    });
                    let tb = c.temp.as_ref().and_then(|r| r.as_ref().ok()).map(|v| (band(kind, *v), band_name(t, band(kind, *v))));
                    enable |= self.tile(ui, p, t.temperature, temp, tb);
                    let hot = from_rd(&c.hotspot, |v| Val::Text(format!("{:.0} °C", glide(ui, "cpu-hot", v)), band_color(band(kind, v), p)));
                    enable |= self.tile(ui, p, t.hotspot, hot, None);
                });
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 30.0;
                    let clock = match c.clock_mhz {
                        Some(m) => Val::Text(fmt_mhz(glide(ui, "cpu-clock", m)), p.text),
                        None => Val::Missing(Why::NotReported),
                    };
                    enable |= self.tile(ui, p, t.clock, clock, None);
                    let power = from_rd(&c.power, |w| Val::Text(format!("{:.1} W", glide(ui, "cpu-power", w)), p.text));
                    enable |= self.tile(ui, p, t.power, power, None);
                });
            });
        });
        ui.add_space(12.0);
        ui.label(RichText::new(t.per_core).font(theme::reg(12.0)).color(p.muted));
        ui.add_space(4.0);
        self.core_strip(ui, p);
        ui.add_space(10.0);
        let temps: Vec<f32> = self.hist.cpu_temp.iter().collect();
        let loads: Vec<f32> = self.hist.cpu_load.iter().collect();
        self.legend(ui, p, &[(t.load, p.accent), (t.temperature, p.text2)]);
        ui.add_space(4.0);
        self.chart(ui, p, &[(&loads, p.accent), (&temps, p.text2)], 100.0, 54.0);
        enable
    }

    fn core_strip(&self, ui: &mut Ui, p: &Pal) {
        let cores = &self.live.cpu.per_core;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        if cores.is_empty() {
            return;
        }
        let gap = 3.0;
        let n = cores.len() as f32;
        let w = ((rect.width() - gap * (n - 1.0)) / n).clamp(3.0, 40.0);
        let total = w * n + gap * (n - 1.0);
        let x0 = rect.left() + (rect.width() - total).max(0.0) / 2.0 * 0.0;
        for (i, v) in cores.iter().enumerate() {
            let x = x0 + i as f32 * (w + gap);
            let col = egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(w, rect.height()));
            ui.painter().rect_filled(col, 3.0, widgets::track(p));
            let h = (rect.height() * glide(ui, &format!("core{i}"), *v) / 100.0).max(if *v > 0.5 { 2.0 } else { 0.0 });
            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.bottom() - h), egui::pos2(x + w, rect.bottom())), 3.0, p.accent);
        }
    }

    fn gpu_card(&self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let Some(g) = &self.live.gpu else {
            self.card_header(ui, p, Icon::Gpu, t.card_gpu, "");
            ui.label(RichText::new(t.no_gpu).font(theme::reg(14.0)).color(p.text2));
            return;
        };
        self.card_header(ui, p, Icon::Gpu, t.card_gpu, &g.name);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            self.ring_gauge(ui, p, "gpu-load", g.load, t.load, 148.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let temp = from_rd(&g.temp, |v| Val::Text(format!("{:.0} °C", glide(ui, "gpu-temp", v)), band_color(band(SensorKind::Gpu, v), p)));
                let tb = g.temp.as_ref().and_then(|r| r.as_ref().ok()).map(|v| (band(SensorKind::Gpu, *v), band_name(t, band(SensorKind::Gpu, *v))));
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    self.tile(ui, p, t.temperature, temp, tb);
                    let hot = from_rd(&g.hotspot, |v| Val::Text(format!("{:.0} °C", glide(ui, "gpu-hot", v)), band_color(band(SensorKind::Gpu, v), p)));
                    self.tile(ui, p, t.hotspot, hot, None);
                });
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    let mem = from_rd(&g.mem_temp, |v| Val::Text(format!("{:.0} °C", v), band_color(band(SensorKind::Gpu, v), p)));
                    self.tile(ui, p, t.mem_junction, mem, None);
                    let clock = from_rd(&g.clock_mhz, |m| Val::Text(fmt_mhz(glide(ui, "gpu-clock", m)), p.text));
                    self.tile(ui, p, t.clock, clock, None);
                });
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    let power = match (&g.power, &g.power_limit) {
                        (Some(Ok(w)), Some(Ok(l))) => Val::Text(format!("{:.0} / {:.0} W", glide(ui, "gpu-power", *w), l), p.text),
                        (Some(Ok(w)), _) => Val::Text(format!("{:.0} W", w), p.text),
                        (Some(Err(why)), _) => Val::Missing(*why),
                        _ => Val::Missing(Why::NotReported),
                    };
                    self.tile(ui, p, t.power, power, None);
                    let fan = from_rd(&g.fan, |f| Val::Text(format!("{:.0}%", f), p.text));
                    self.tile(ui, p, t.fan, fan, None);
                });
            });
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.vram).font(theme::reg(12.0)).color(p.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let s = if g.vram_total > 0 { format!("{} / {}", fmt_bytes(g.vram_used), fmt_bytes(g.vram_total)) } else { fmt_bytes(g.vram_used) };
                ui.label(RichText::new(s).font(theme::mono(12.0)).color(p.text));
            });
        });
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
        let frac = if g.vram_total > 0 { g.vram_used as f32 / g.vram_total as f32 } else { 0.0 };
        widgets::bar(ui.painter(), r, glide(ui, "vram", frac), p.accent, widgets::track(p));
        ui.add_space(10.0);
        let loads: Vec<f32> = self.hist.gpu_load.iter().collect();
        let temps: Vec<f32> = self.hist.gpu_temp.iter().collect();
        self.legend(ui, p, &[(t.load, p.accent), (t.temperature, p.text2)]);
        ui.add_space(4.0);
        self.chart(ui, p, &[(&loads, p.accent), (&temps, p.text2)], 100.0, 40.0);
    }

    fn ram_card(&self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let r = &self.live.ram;
        self.card_header(ui, p, Icon::Ram, t.card_ram, "");
        let pct = if r.total > 0 { r.used as f32 / r.total as f32 * 100.0 } else { 0.0 };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            self.ring_gauge(ui, p, "ram", pct, t.used, 112.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(RichText::new(format!("{} / {}", fmt_bytes(r.used), fmt_bytes(r.total))).font(theme::mono(20.0)).color(p.text));
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    self.tile(ui, p, t.cached, Val::Text(fmt_bytes(r.cached), p.text), None);
                    self.tile(ui, p, t.available, Val::Text(fmt_bytes(r.available), p.text), None);
                });
            });
        });
        ui.add_space(10.0);
        let h: Vec<f32> = self.hist.ram.iter().collect();
        self.chart(ui, p, &[(&h, p.accent)], 100.0, 40.0);
    }

    fn net_card(&self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let n = &self.live.net;
        self.card_header(ui, p, Icon::Network, t.card_net, if n.adapter.is_empty() { t.no_network } else { &n.adapter });
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 40.0;
            self.tile(ui, p, t.download, Val::Text(fmt_rate(glide(ui, "net-down", n.down as f32) as f64), p.text), None);
            self.tile(ui, p, t.upload, Val::Text(fmt_rate(glide(ui, "net-up", n.up as f32) as f64), p.text), None);
        });
        ui.add_space(12.0);
        let down: Vec<f32> = self.hist.down.iter().collect();
        let up: Vec<f32> = self.hist.up.iter().collect();
        let max = down.iter().chain(up.iter()).cloned().fold(100_000.0, f32::max) * 1.15;
        self.legend(ui, p, &[(t.download, p.accent), (t.upload, p.text2)]);
        ui.add_space(4.0);
        self.chart(ui, p, &[(&down, p.accent), (&up, p.text2)], max, 56.0);
    }

    fn storage_card(&self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        self.card_header(ui, p, Icon::Storage, t.card_storage, "");
        if self.live.disks.is_empty() {
            ui.label(RichText::new(t.no_drives).font(theme::reg(14.0)).color(p.text2));
            return;
        }
        for (i, d) in self.live.disks.iter().enumerate() {
            if i > 0 {
                theme::divider(ui, p);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(&d.name).font(theme::medium(14.0)).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &d.temp {
                        Some(Ok(v)) => {
                            let b = band(SensorKind::Nvme, *v);
                            ui.label(RichText::new(format!("{v:.0} °C")).font(theme::mono(14.0)).color(band_color(b, p)));
                        }
                        Some(Err(w)) if *w != Why::NotReported => {
                            ui.label(RichText::new(crate::specs::DASH).font(theme::mono(14.0)).color(p.muted)).on_hover_text(why_text(t, *w));
                        }
                        Some(Err(_)) | None => {}
                    };
                });
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 28.0;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new(t.activity).font(theme::reg(12.0)).color(p.muted));
                        ui.label(RichText::new(format!("{:.0}%", glide(ui, &format!("disk{i}"), d.busy))).font(theme::mono(12.0)).color(p.text));
                    });
                    let (rd, wr): (Vec<f32>, Vec<f32>) = self.hist.disks.get(i).map(|h| (h.0.iter().collect(), h.1.iter().collect())).unwrap_or_default();
                    let max = rd.iter().chain(wr.iter()).cloned().fold(1_000_000.0, f32::max) * 1.1;
                    let (r, _) = ui.allocate_exact_size(vec2(210.0, 40.0), Sense::hover());
                    widgets::sparkline(ui.painter(), r, &[(&rd, p.accent), (&wr, p.text2)], max, p.line_hi);
                });
                self.tile(ui, p, t.read, Val::Text(fmt_rate(d.read), p.text), None);
                self.tile(ui, p, t.write, Val::Text(fmt_rate(d.write), p.text), None);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for v in &d.volumes {
                        let used = if v.total > 0 { 1.0 - v.free as f32 / v.total as f32 } else { 0.0 };
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.label(RichText::new(&v.name).font(theme::mono(13.0)).color(p.text));
                            ui.label(RichText::new(&v.fs).font(theme::reg(11.0)).color(p.muted));
                            let (r, _) = ui.allocate_exact_size(vec2(90.0, 6.0), Sense::hover());
                            widgets::bar(ui.painter(), r, used, p.accent, widgets::track(p));
                            ui.label(RichText::new(t.free_of.replace("{free}", &fmt_bytes(v.free)).replace("{total}", &fmt_bytes(v.total))).font(theme::reg(12.0)).color(p.text2));
                        });
                    }
                });
            });
        }
    }

    pub(super) fn monitor_view(&mut self, ui: &mut Ui, p: &Pal) {
        let t = self.s();
        let ctx = ui.ctx().clone();
        let mut enable = false;
        let mut dismiss = false;
        let banner = self.live.cpu_access_missing && !self.cfg.banner_dismissed && offered(self.live.cpu.vendor) && !self.access.enabled;
        page(ui, p, t.nav_monitor, None, |_| {}, |ui| {
            if !self.live.ready {
                ui.add_space(24.0);
                ui.label(RichText::new(t.waiting).font(theme::reg(14.0)).color(p.text2));
                return;
            }
            if banner {
                egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0, p.line_hi)).corner_radius(theme::R_CARD).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
                        ui.label(RichText::new(t.banner_text).font(theme::reg(14.0)).color(p.text));
                        if widgets::btn(ui, t.banner_enable, Btn::Primary, p).clicked() {
                            enable = true;
                        }
                        if widgets::btn(ui, t.banner_dismiss, Btn::Secondary, p).clicked() {
                            dismiss = true;
                        }
                    });
                });
                ui.add_space(14.0);
            }
            let wide = ui.available_width() >= 640.0;
            let card = |ui: &mut Ui, h: f32, add: &mut dyn FnMut(&mut Ui)| {
                theme::card(p).inner_margin(Margin::same(16)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(h);
                    add(ui);
                });
            };
            let gap = 14.0;
            ui.spacing_mut().item_spacing.x = 14.0;
            if wide {
                ui.columns(2, |cols| {
                    cols[0].spacing_mut().item_spacing.y = gap;
                    cols[1].spacing_mut().item_spacing.y = gap;
                    card(&mut cols[0], 372.0, &mut |ui| enable |= self.cpu_card(ui, p));
                    card(&mut cols[1], 372.0, &mut |ui| self.gpu_card(ui, p));
                });
                ui.add_space(gap);
                ui.columns(2, |cols| {
                    card(&mut cols[0], 222.0, &mut |ui| self.ram_card(ui, p));
                    card(&mut cols[1], 222.0, &mut |ui| self.net_card(ui, p));
                });
            } else {
                card(ui, 372.0, &mut |ui| enable |= self.cpu_card(ui, p));
                ui.add_space(gap);
                card(ui, 372.0, &mut |ui| self.gpu_card(ui, p));
                ui.add_space(gap);
                card(ui, 222.0, &mut |ui| self.ram_card(ui, p));
                ui.add_space(gap);
                card(ui, 222.0, &mut |ui| self.net_card(ui, p));
            }
            ui.add_space(gap);
            card(ui, 0.0, &mut |ui| self.storage_card(ui, p));
        });
        if enable {
            self.open_enable_dialog();
        }
        if dismiss {
            self.cfg.banner_dismissed = true;
            self.touch_cfg(&ctx);
        }
    }
}

fn band_name(t: &crate::strings::S, b: Band) -> &'static str {
    match b {
        Band::Normal => t.band_normal,
        Band::Warm => t.band_warm,
        Band::Hot => t.band_hot,
    }
}
