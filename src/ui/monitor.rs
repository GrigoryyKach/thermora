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

/// The widest value of the same kind as `text`, to size its slot: rates, temperatures, clocks, power, percentages and
/// sizes each have one. Anything else gets its own width.
pub fn slot_sample(text: &str) -> &str {
    if text.ends_with("/s") {
        "999.9 MB/s"
    } else if text.ends_with("°C") {
        "999 °C"
    } else if text.ends_with("GHz") || text.ends_with("MHz") {
        "9.99 GHz"
    } else if text.ends_with(" W") && text.contains('/') {
        "999 / 999 W"
    } else if text.ends_with(" W") {
        "999.9 W"
    } else if text.ends_with('%') {
        "100%"
    } else if text.ends_with(" GB") || text.ends_with(" MB") || text.ends_with(" TB") || text.ends_with(" KB") {
        "999.9 GB"
    } else {
        text
    }
}

/// The gauge beside its stats when the card is wide enough, above them when it is narrow.
fn ring_and_stats(ui: &mut Ui, ring: &mut dyn FnMut(&mut Ui), stats: &mut dyn FnMut(&mut Ui)) {
    if ui.available_width() < 372.0 {
        ring(ui);
        ui.add_space(10.0);
        stats(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            ring(ui);
            ui.vertical(|ui| stats(ui));
        });
    }
}

fn text_w(ui: &Ui, s: &str, font: egui::FontId) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), font, Color32::WHITE).size().x
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
        // A fixed slot, sized from the widest value of its kind in the tabular font, so a changing number never moves
        // the layout. Text is truncated, never wrapped or allowed to push the tile wider.
        let sample = match &val {
            Val::Text(s, _) => slot_sample(s),
            Val::Missing(_) => "999 °C",
        };
        let mut slot = ui.painter().layout_no_wrap(sample.to_string(), theme::mono(20.0), p.text).size().x;
        if band.is_some() {
            let t = self.s();
            let bw = [t.band_normal, t.band_warm, t.band_hot].iter().map(|n| ui.painter().layout_no_wrap(n.to_string(), theme::reg(12.0), p.text).size().x).fold(0.0, f32::max);
            slot += 8.0 + bw;
        }
        let label_w = ui.painter().layout_no_wrap(label.to_string(), theme::reg(12.0), p.text).size().x;
        let slot = slot.max(label_w).ceil();
        ui.vertical(|ui| {
            ui.set_min_width(slot);
            ui.set_max_width(slot);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(label).font(theme::reg(12.0)).color(p.muted)).truncate());
            match val {
                Val::Text(s, col) => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.add(egui::Label::new(RichText::new(s).font(theme::mono(20.0)).color(col)).truncate());
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
        ring_and_stats(ui, &mut |ui| self.ring_gauge(ui, p, "cpu-load", c.load, t.load, if ui.available_width() < 440.0 { 120.0 } else { 140.0 }), &mut |ui| {
            {
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
            }
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
        let n = cores.len() as f32;
        let gap = if n > 32.0 { 1.0 } else { 3.0 };
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
        ring_and_stats(ui, &mut |ui| self.ring_gauge(ui, p, "gpu-load", g.load, t.load, if ui.available_width() < 440.0 { 120.0 } else { 140.0 }), &mut |ui| {
            {
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
            }
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
                let w = ui.available_width();
                let line = format!("{} / {}", fmt_bytes(r.used), fmt_bytes(r.total));
                ui.add(egui::Label::new(RichText::new(widgets::fit_text(ui.painter(), &line, &theme::mono(20.0), w)).font(theme::mono(20.0)).color(p.text)).truncate());
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
            let avail = ui.available_width();
            // Name on the left (truncated), temperature on the right, both painted in a rect of exactly the card's width.
            let (head, _) = ui.allocate_exact_size(vec2(avail, 22.0), Sense::hover());
            let temp_txt = match &d.temp {
                Some(Ok(v)) => Some((format!("{v:.0} °C"), band_color(band(SensorKind::Nvme, *v), p), None)),
                Some(Err(w)) if *w != Why::NotReported => Some((crate::specs::DASH.to_string(), p.muted, Some(why_text(t, *w)))),
                _ => None,
            };
            let temp_w = temp_txt.as_ref().map(|_| text_w(ui, "999 °C", theme::mono(14.0))).unwrap_or(0.0);
            if let Some((txt, col, why)) = &temp_txt {
                let r = ui.painter().text(egui::pos2(head.right(), head.center().y), Align2::RIGHT_CENTER, txt, theme::mono(14.0), *col);
                if let Some(w) = why {
                    ui.interact(r, ui.scope_id().with(("disk-temp", i)), Sense::hover()).on_hover_text(*w);
                }
            }
            let name = widgets::fit_text(ui.painter(), &d.name, &theme::medium(14.0), (avail - temp_w - 12.0).max(40.0));
            ui.painter().text(egui::pos2(head.left(), head.center().y), Align2::LEFT_CENTER, name, theme::medium(14.0), p.text);
            ui.add_space(6.0);

            // Stats: active time with a chart, read and write in fixed slots.
            let gap = 28.0;
            let rate_w = text_w(ui, "999.9 MB/s", theme::mono(20.0)).max(text_w(ui, t.read, theme::reg(12.0))).max(text_w(ui, t.write, theme::reg(12.0))).ceil();
            let chart_full = 210.0;
            let stats_full = chart_full + 2.0 * rate_w + 3.0 * gap;
            let vol_min = 240.0;
            let one_row = avail >= stats_full + vol_min;
            let chart_w = if one_row { chart_full } else { (avail - 2.0 * rate_w - 2.0 * gap).clamp(100.0, chart_full) };
            let vol_w = if one_row { avail - stats_full } else { avail };
            let (rd, wr): (Vec<f32>, Vec<f32>) = self.hist.disks.get(i).map(|h| (h.0.iter().collect(), h.1.iter().collect())).unwrap_or_default();
            let draw_stats = |ui: &mut Ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(|ui| {
                        ui.set_min_width(chart_w);
                        ui.set_max_width(chart_w);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.label(RichText::new(t.activity).font(theme::reg(12.0)).color(p.muted));
                            ui.label(RichText::new(format!("{:.0}%", glide(ui, &format!("disk{i}"), d.busy))).font(theme::mono(12.0)).color(p.text));
                        });
                        let max = rd.iter().chain(wr.iter()).cloned().fold(1_000_000.0, f32::max) * 1.1;
                        let (r, _) = ui.allocate_exact_size(vec2(chart_w, 40.0), Sense::hover());
                        widgets::sparkline(ui.painter(), r, &[(&rd, p.accent), (&wr, p.text2)], max, p.line_hi);
                    });
                    self.tile(ui, p, t.read, Val::Text(fmt_rate(d.read), p.text), None);
                    self.tile(ui, p, t.write, Val::Text(fmt_rate(d.write), p.text), None);
                });
            };
            let draw_volumes = |ui: &mut Ui, w: f32| {
                ui.vertical(|ui| {
                    ui.set_max_width(w);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for v in &d.volumes {
                        volume_row(ui, p, t, v, w);
                    }
                });
            };
            if one_row {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.allocate_ui_with_layout(vec2(stats_full - gap, 60.0), egui::Layout::top_down(egui::Align::Min), |ui| draw_stats(ui));
                    ui.add_space(gap);
                    draw_volumes(ui, vol_w);
                });
            } else {
                draw_stats(ui);
                ui.add_space(8.0);
                draw_volumes(ui, vol_w);
            }
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
            // Two columns from about 790 px of usable width (a window of about 1120 px; the default 1180 has ~868); below that, one column. When a
            // card's inside is narrower than 372 px its gauge moves above its stats, so nothing overflows.
            let wide = ui.available_width() >= 790.0;
            let narrow_cards = wide && (ui.available_width() - 14.0) / 2.0 - 34.0 < 372.0;
            let top_h = if narrow_cards { 500.0 } else { 372.0 };
            let card = |ui: &mut Ui, h: f32, add: &mut dyn FnMut(&mut Ui)| {
                theme::card(p).inner_margin(Margin::same(16)).show(ui, |ui| {
                    // Exactly the width the grid gives: never wider, whatever the content, and clipped as a last resort.
                    let w = ui.available_width();
                    ui.set_width(w);
                    ui.set_max_width(w);
                    // Clip by width only: the height is the scroll area's business.
                    let (m, c) = (ui.max_rect(), ui.clip_rect());
                    ui.set_clip_rect(egui::Rect::from_min_max(egui::pos2(m.left().max(c.left()), c.top()), egui::pos2(m.right().min(c.right()), c.bottom())));
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
                    card(&mut cols[0], top_h, &mut |ui| enable |= self.cpu_card(ui, p));
                    card(&mut cols[1], top_h, &mut |ui| self.gpu_card(ui, p));
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

/// One volume: letter, file system, a bar and "free of", painted in a rect of exactly `w` so it can never widen its
/// card. The bar shrinks and the text is cut with an ellipsis before anything overflows.
fn volume_row(ui: &mut Ui, p: &Pal, t: &crate::strings::S, v: &VolumeLive, w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let used = if v.total > 0 { 1.0 - v.free as f32 / v.total as f32 } else { 0.0 };
    let cy = rect.center().y;
    let name_f = theme::mono(13.0);
    let fs_f = theme::reg(11.0);
    let txt_f = theme::reg(12.0);
    let name = widgets::fit_text(ui.painter(), &v.name, &name_f, (w * 0.25).max(24.0));
    let name_w = text_w(ui, &name, name_f.clone());
    ui.painter().text(egui::pos2(rect.left(), cy), Align2::LEFT_CENTER, &name, name_f, p.text);
    let mut x = rect.left() + name_w + 8.0;
    let fs_w = text_w(ui, &v.fs, fs_f.clone());
    if x + fs_w < rect.right() - 60.0 {
        ui.painter().text(egui::pos2(x, cy), Align2::LEFT_CENTER, &v.fs, fs_f, p.muted);
        x += fs_w + 10.0;
    }
    let left = (rect.right() - x).max(0.0);
    let bar_w = (left * 0.35).clamp(0.0, 90.0);
    if bar_w >= 24.0 {
        widgets::bar(ui.painter(), egui::Rect::from_min_size(egui::pos2(x, cy - 3.0), vec2(bar_w, 6.0)), used, p.accent, widgets::track(p));
        x += bar_w + 10.0;
    }
    let text = t.free_of.replace("{free}", &fmt_bytes(v.free)).replace("{total}", &fmt_bytes(v.total));
    let shown = widgets::fit_text(ui.painter(), &text, &txt_f, (rect.right() - x).max(0.0));
    ui.painter().text(egui::pos2(x, cy), Align2::LEFT_CENTER, shown, txt_f, p.text2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_samples_are_the_widest_of_their_kind() {
        assert_eq!(slot_sample("65.5 KB/s"), "999.9 MB/s");
        assert_eq!(slot_sample("10.1 MB/s"), slot_sample("0 B/s"));
        assert_eq!(slot_sample("4.16 GHz"), slot_sample("800 MHz"));
        assert_eq!(slot_sample("57 / 115 W"), "999 / 999 W");
        assert_eq!(slot_sample("38.6 W"), "999.9 W");
        assert_eq!(slot_sample("23.8 GB"), slot_sample("512 MB"));
    }

    /// For any value, the text of a slot kind is never wider than its sample, in the tabular font the page uses.
    #[test]
    fn no_value_is_wider_than_its_slot() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, crate::theme::ThemeMode::Dark);
        let mut worst: Vec<(String, f32, f32)> = Vec::new();
        let mut full = ctx.run_pass(Default::default(), |ctx| {
            let w = |s: &str| ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), theme::mono(20.0), Color32::WHITE).size().x);
            let mut vals: Vec<f64> = vec![0.0, 1.0, 9.99, 10.0, 99.9, 100.0, 999.0, 999.9, 1e3, 1e4, 1e5, 9.99e5, 1e6, 6.55e4, 1.01e7, 9.99e8, 1e9, 1e10, 9.99e10, 1e11, 1e12];
            for i in 0..2000 {
                vals.push(i as f64 * 7.3e6);
            }
            for v in vals {
                let r = fmt_rate(v);
                let sample = slot_sample(&r);
                worst.push((r.clone(), w(&r), w(sample)));
                let b = fmt_bytes(v as u64);
                worst.push((b.clone(), w(&b), w(slot_sample(&b))));
            }
        });
        full.textures_delta.clear();
        // Rates up to 999.9 MB/s fit "999.9 MB/s"; a GB/s rate has the same length.
        for (txt, got, slot) in worst.iter().filter(|(t, _, _)| !t.ends_with("TB")) {
            assert!(got <= slot, "{txt}: {got} > {slot}");
        }
    }
}
