//! Small drawing helpers: icons, buttons, dropdowns, the ring gauge, sparklines and bars.
//! Everything is flat: solid fills and hairlines, no gradients, no glows.

#![allow(dead_code)]

use crate::theme::{self, Btn, Pal};
use eframe::egui::{self, epaint::PathStroke, Align2, Color32, CursorIcon, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

/// Fill of the selected row on a surface that is not a card: raised on dark, the card white on light.
pub fn selected_fill(p: &Pal) -> Color32 {
    if p.dark { p.raised } else { p.card }
}

/// Hover fill that fades in, for rows drawn by hand.
pub fn hover_fill(ui: &Ui, id: egui::Id, hovered: bool, p: &Pal) -> Color32 {
    theme::alpha(selected_fill(p), (theme::fade(ui, id.with("hover"), hovered) * 255.0) as u8)
}

/// A solid track colour for gauges and bars (the hairline colours are translucent and would double up where shapes overlap).
pub fn track(p: &Pal) -> Color32 {
    theme::mix(p.card, p.text, 0.14)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Monitor,
    Specs,
    Gear,
    Close,
    Copy,
    Refresh,
    Chevron,
    Cpu,
    Gpu,
    Board,
    Ram,
    Storage,
    Audio,
    Network,
}

/// Points of an arc around `c`, angles in radians measured clockwise from 12 o'clock.
pub fn arc_points(c: Pos2, r: f32, a0: f32, a1: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            c + vec2(a.sin(), -a.cos()) * r
        })
        .collect()
}

/// Draws a line icon centred at `c` within a `size` box. Stroke 1.5, never its own colour.
pub fn icon(p: &Painter, ic: Icon, c: Pos2, size: f32, col: Color32) {
    let u = size / 16.0;
    let st = Stroke::new(1.5, col);
    let pt = |x: f32, y: f32| c + vec2(x, y) * u;
    let line = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], st);
    };
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        p.rect_stroke(Rect::from_min_max(pt(x0, y0), pt(x1, y1)), r * u, st, StrokeKind::Middle);
    };
    match ic {
        Icon::Monitor => {
            // A gauge: an open arc and a needle.
            p.add(Shape::line(arc_points(pt(0.0, 2.0), 6.5 * u, -2.2, 2.2, 20), st));
            line(pt(0.0, 2.0), pt(3.2, -2.4));
            p.circle_filled(pt(0.0, 2.0), 1.3 * u, col);
        }
        Icon::Specs => {
            for y in [-4.5, 0.0, 4.5] {
                line(pt(-6.0, y), pt(-3.5, y));
                line(pt(-1.0, y), pt(6.5, y));
            }
        }
        Icon::Gear => {
            p.circle_stroke(pt(0.0, 0.0), 2.6 * u, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.sin(), -a.cos());
                line(c + d * 5.0 * u, c + d * 7.0 * u);
            }
            p.circle_stroke(pt(0.0, 0.0), 5.0 * u, st);
        }
        Icon::Close => {
            line(pt(-4.5, -4.5), pt(4.5, 4.5));
            line(pt(4.5, -4.5), pt(-4.5, 4.5));
        }
        Icon::Copy => {
            rect(-2.0, -2.0, 6.5, 6.5, 1.5);
            p.add(Shape::line(vec![pt(-4.5, 2.0), pt(-4.5, -4.5), pt(2.0, -4.5)], st));
        }
        Icon::Refresh => {
            p.add(Shape::line(arc_points(pt(0.0, 0.0), 5.5 * u, 0.6, 5.6, 22), st));
            line(pt(2.2, -6.4), pt(4.2, -4.2));
            line(pt(4.2, -4.2), pt(1.2, -3.4));
        }
        Icon::Chevron => {
            line(pt(-2.0, -4.0), pt(2.5, 0.0));
            line(pt(2.5, 0.0), pt(-2.0, 4.0));
        }
        Icon::Cpu => {
            rect(-4.0, -4.0, 4.0, 4.0, 1.0);
            for v in [-2.0, 2.0] {
                line(pt(v, -6.5), pt(v, -4.0));
                line(pt(v, 4.0), pt(v, 6.5));
                line(pt(-6.5, v), pt(-4.0, v));
                line(pt(4.0, v), pt(6.5, v));
            }
        }
        Icon::Gpu => {
            rect(-7.0, -4.0, 7.0, 4.0, 1.5);
            p.circle_stroke(pt(-2.5, 0.0), 2.2 * u, st);
            line(pt(2.0, -1.5), pt(5.0, -1.5));
            line(pt(2.0, 1.5), pt(5.0, 1.5));
        }
        Icon::Board => {
            rect(-6.0, -6.0, 6.0, 6.0, 1.5);
            rect(-3.5, -3.5, 0.5, 0.5, 0.5);
            line(pt(3.0, -3.5), pt(3.0, 3.5));
        }
        Icon::Ram => {
            rect(-7.0, -3.5, 7.0, 3.0, 1.0);
            for x in [-4.0, -1.0, 2.0, 5.0] {
                line(pt(x, 3.0), pt(x, 5.0));
            }
        }
        Icon::Storage => {
            rect(-6.5, -3.5, 6.5, 3.5, 2.0);
            p.circle_filled(pt(4.0, 0.0), 1.0 * u, col);
        }
        Icon::Audio => {
            p.add(Shape::line(vec![pt(-5.0, -2.0), pt(-2.0, -2.0), pt(2.0, -5.0), pt(2.0, 5.0), pt(-2.0, 2.0), pt(-5.0, 2.0), pt(-5.0, -2.0)], st));
            p.add(Shape::line(arc_points(pt(2.0, 0.0), 4.0 * u, 0.9, 2.2, 8), st));
        }
        Icon::Network => {
            p.circle_stroke(pt(0.0, 0.0), 6.0 * u, st);
            line(pt(-6.0, 0.0), pt(6.0, 0.0));
            p.add(Shape::line(arc_points(pt(0.0, 0.0), 6.0 * u, 0.0, std::f32::consts::PI, 12).iter().map(|q| pos2(c.x + (q.x - c.x) * 0.45, q.y)).collect(), st));
        }
    }
}

/// The Thermora mark: a thermometer with two ticks, drawn in a 16 px box around `c`.
pub fn mark_glyph(painter: &Painter, c: Pos2, col: Color32) {
    let st = Stroke::new(1.5, col);
    painter.rect_stroke(Rect::from_center_size(c + vec2(-1.5, -2.2), vec2(4.2, 10.0)), 2.1, st, StrokeKind::Middle);
    painter.circle_filled(c + vec2(-1.5, 4.6), 3.4, col);
    painter.line_segment([c + vec2(-1.5, 4.6), c + vec2(-1.5, -2.5)], Stroke::new(1.8, col));
    painter.line_segment([c + vec2(2.6, -4.0), c + vec2(5.0, -4.0)], st);
    painter.line_segment([c + vec2(2.6, -0.5), c + vec2(5.0, -0.5)], st);
}

/// `theme::button` derives its id from the label alone, so two buttons with the same text in one frame share
/// one id. This wrapper salts the id with the button's position.
pub fn btn(ui: &mut Ui, label: &str, kind: Btn, p: &Pal) -> Response {
    let pos = ui.cursor().min;
    ui.push_id(("btn-at", pos.x as i32, pos.y as i32), |ui| theme::button(ui, label, kind, p)).inner
}

pub fn btn_enabled(ui: &mut Ui, label: &str, kind: Btn, enabled: bool, p: &Pal) -> Response {
    if enabled {
        return btn(ui, label, kind, p);
    }
    let font = theme::medium(14.0);
    let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.muted);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0, kind.height()), Sense::hover());
    ui.painter().rect(rect, theme::R_CONTROL as f32, Color32::TRANSPARENT, Stroke::new(1.0, p.line), StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, p.muted);
    resp
}

/// Square icon button; a raised fill fades in on hover.
pub fn icon_button(ui: &mut Ui, ic: Icon, size: f32, p: &Pal) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let t = theme::fade(ui, resp.id.with("hover"), resp.hovered());
    ui.painter().rect_filled(rect, theme::R_CONTROL, theme::alpha(p.raised, (t * 255.0) as u8));
    icon(ui.painter(), ic, rect.center(), size * 0.55, theme::mix(p.muted, p.text, t));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Height shared by every control that sits in a row.
pub const ROW_BTN_H: f32 = 34.0;

fn chevron(ui: &Ui, rect: Rect, color: Color32) {
    theme::icon_chevron(ui.painter(), rect.center(), color, true);
}

/// A dropdown button in the suite style.
pub fn combo(id: impl std::fmt::Debug, text: &str, width: f32) -> egui::ComboBox<'static> {
    egui::ComboBox::from_id_salt(format!("{id:?}"))
        .width(width)
        .selected_text(egui::RichText::new(text.to_string()).font(theme::reg(14.0)))
        .icon(|ui, rect, visuals, _open| chevron(ui, rect, visuals.fg_stroke.color))
}

/// Shortens `s` with an ellipsis so it fits `max_w` in `font`.
pub fn fit_text(painter: &Painter, s: &str, font: &egui::FontId, max_w: f32) -> String {
    if painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x <= max_w {
        return s.to_string();
    }
    let mut out: String = s.chars().collect();
    while !out.is_empty() {
        out.pop();
        let cand = format!("{}…", out.trim_end());
        if painter.layout_no_wrap(cand.clone(), font.clone(), Color32::WHITE).size().x <= max_w {
            return cand;
        }
    }
    String::new()
}

// ───────────────────────── gauges and charts ─────────────────────────

/// Start and end of the 270-degree gauge sweep, clockwise from 12 o'clock (gap at the bottom).
const SWEEP: (f32, f32) = (-2.356_194_5, 2.356_194_5);

/// A ring gauge: a track and a value arc in `color`, round ends, crisp (fine arc steps, fractional stroke).
/// `frac` is 0..1. The caller writes the number in the middle.
pub fn ring(painter: &Painter, c: Pos2, r: f32, thick: f32, frac: f32, color: Color32, track: Color32) {
    let steps = ((r * 2.0) as usize).clamp(48, 160);
    painter.add(Shape::line(arc_points(c, r, SWEEP.0, SWEEP.1, steps), PathStroke::new(thick, track)));
    cap(painter, c, r, SWEEP.0, thick, track);
    cap(painter, c, r, SWEEP.1, thick, track);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.004 {
        let end = SWEEP.0 + (SWEEP.1 - SWEEP.0) * f;
        let n = ((steps as f32 * f) as usize).max(2);
        painter.add(Shape::line(arc_points(c, r, SWEEP.0, end, n), PathStroke::new(thick, color)));
        cap(painter, c, r, SWEEP.0, thick, color);
        cap(painter, c, r, end, thick, color);
    }
}

fn cap(painter: &Painter, c: Pos2, r: f32, angle: f32, thick: f32, color: Color32) {
    painter.circle_filled(c + vec2(angle.sin(), -angle.cos()) * r, thick / 2.0, color);
}

/// A thin horizontal bar with a value fill.
pub fn bar(painter: &Painter, rect: Rect, frac: f32, color: Color32, track: Color32) {
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, track);
    let w = (rect.width() * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { rect.height() } else { 0.0 });
    if w > 0.0 {
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(w.min(rect.width()), rect.height())), r, color);
    }
}

/// A sparkline of `series` (each 0..=`max`) in `rect`: baseline, a flat 12% fill under the line, and the line.
/// Several series draw on top of each other; the first gets the fill.
pub fn sparkline(painter: &Painter, rect: Rect, series: &[(&[f32], Color32)], max: f32, grid: Color32) {
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, grid));
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, theme::alpha(grid, 28)));
    let max = max.max(1e-6);
    for (i, (data, col)) in series.iter().enumerate() {
        if data.len() < 2 {
            continue;
        }
        let n = crate::model::History::POINTS - 1;
        let off = n + 1 - data.len();
        let pts: Vec<Pos2> = data.iter().enumerate().map(|(k, v)| pos2(rect.left() + rect.width() * (k + off) as f32 / n as f32, rect.bottom() - 1.5 - (rect.height() - 3.0) * (v / max).clamp(0.0, 1.0))).collect();
        if i == 0 {
            // Flat fill: thin vertical strips between the baseline and the line.
            let fill = theme::alpha(*col, 30);
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let mut m = egui::epaint::Mesh::default();
                for q in [a, b, pos2(b.x, rect.bottom() - 1.0), pos2(a.x, rect.bottom() - 1.0)] {
                    m.colored_vertex(q, fill);
                }
                m.add_triangle(0, 1, 2);
                m.add_triangle(0, 2, 3);
                painter.add(Shape::mesh(m));
            }
        }
        painter.add(Shape::line(pts, PathStroke::new(1.6, *col)));
    }
}
