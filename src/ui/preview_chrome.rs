//! Banner, arrows and help pill laid over the full-frame preview.

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Galley, Id, Rect, Response, Sense, Ui, Vec2, pos2,
    vec2,
};

use super::preview_view::{Info, NOTCH_FACTOR, Nav};
use super::theme::{self, icons};
use crate::model::UpAxis;
use crate::preview::PreviewState;

pub const BANNER_H: f32 = 44.0;
const BANNER_BG: Color32 = Color32::from_black_alpha(200);
const OVERLAY_BG: Color32 = Color32::from_black_alpha(153);
const TEXT: Color32 = Color32::from_rgb(0xC9, 0xC9, 0xD1);
const STRONG: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF5);
const HINT: Color32 = Color32::from_rgb(0x6E, 0x6E, 0x78);
const SEPARATOR: Color32 = Color32::from_rgb(0x34, 0x34, 0x3B);
const RAIL: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x22);
const BUTTON: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x30);
const GAP: f32 = 12.0;
const ARROW_SIZE: f32 = 44.0;
const ARROW_MARGIN: f32 = 16.0;
const PILL_H: f32 = 30.0;
const PILL_PAD: f32 = 14.0;
const PILL_GAP: f32 = 14.0;
/// The pill stays up this long after the last pointer movement.
const HELP_IDLE_SECS: f64 = 2.0;
const HELP_FADE_SECS: f32 = 0.15;

const IMAGE_HELP: [(&str, &str); 5] = [
    ("← →", "précédente / suivante"),
    ("Molette", "zoom au curseur"),
    ("Glisser", "déplacer"),
    ("0", "ajuster"),
    ("Espace", "fermer"),
];
const MODEL_HELP: [(&str, &str); 5] = [
    ("Clic milieu", "orbite"),
    ("Maj + clic milieu", "déplacer"),
    ("Molette", "rapprocher"),
    ("U", "Y/Z-up"),
    ("← →", "modèle suivant"),
];

fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::MEDIUM.into()))
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::SEMIBOLD.into()))
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn help_visible(now: f64, last_motion: f64) -> bool {
    now - last_motion < HELP_IDLE_SECS
}

/// `48 212` with a no-break space between thousands.
fn group_thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

/// Where the chrome sits; computed before the content so it can shield the image from input.
pub struct Layout {
    rect: Rect,
    is_model: bool,
    banner: Rect,
    prev: Rect,
    next: Rect,
    pill: Rect,
}

impl Layout {
    pub fn new(ui: &Ui, rect: Rect, is_model: bool) -> Self {
        let banner = Rect::from_min_size(rect.min, vec2(rect.width(), BANNER_H));
        let arrow = |x: f32| Rect::from_center_size(pos2(x, rect.center().y), Vec2::splat(ARROW_SIZE));
        let bottom = if is_model { 14.0 } else { 16.0 };
        let width = {
            let painter = ui.painter();
            let items = help_items(is_model);
            let text: f32 = items
                .iter()
                .map(|(key, label)| {
                    painter.layout_no_wrap(key.to_string(), semibold(12.0), STRONG).size().x
                        + 5.0
                        + painter.layout_no_wrap(label.to_string(), FontId::proportional(12.0), TEXT).size().x
                })
                .sum();
            2.0 * PILL_PAD + text + PILL_GAP * (items.len() - 1) as f32
        };
        let pill = Rect::from_min_size(
            pos2(rect.center().x - width / 2.0, rect.max.y - bottom - PILL_H),
            vec2(width, PILL_H),
        );
        Self {
            rect,
            is_model,
            banner,
            prev: arrow(rect.min.x + ARROW_MARGIN + ARROW_SIZE / 2.0),
            next: arrow(rect.max.x - ARROW_MARGIN - ARROW_SIZE / 2.0),
            pill,
        }
    }

    fn rects(&self) -> [Rect; 4] {
        [self.banner, self.prev, self.next, self.pill]
    }

    pub fn pointer_over(&self, ui: &Ui) -> bool {
        let pos = ui.input(|i| i.pointer.hover_pos());
        pos.is_some_and(|p| self.rects().iter().any(|r| r.contains(p)))
    }

    pub fn block_input(&self, ui: &Ui, base: Id) {
        for (i, r) in self.rects().into_iter().enumerate() {
            ui.interact(r, base.with(("chrome-block", i)), Sense::click_and_drag());
        }
    }
}

fn help_items(is_model: bool) -> &'static [(&'static str, &'static str); 5] {
    if is_model { &MODEL_HELP } else { &IMAGE_HELP }
}

pub fn show(ui: &mut Ui, state: &mut PreviewState, info: &Info, layout: &Layout) -> Option<Nav> {
    let mut nav = banner(ui, state, info, layout);
    nav = arrows(ui, info, layout).or(nav);
    help_pill(ui, state, layout);
    nav
}

fn banner(ui: &mut Ui, state: &mut PreviewState, info: &Info, layout: &Layout) -> Option<Nav> {
    let b = layout.banner;
    ui.painter().rect_filled(b, 0.0, BANNER_BG);
    let weak = theme::DARK.text_weak;
    let cy = b.center().y;
    let mut nav = None;
    let mut refit = false;

    // Right cluster first, from the edge inwards, so the left text knows how much room is left.
    let mut right = b.max.x - GAP;
    if layout.is_model {
        if icon_text_button(ui, "recenter", &mut right, cy, &[("Recentrer", medium(12.5), STRONG), ("0", mono(12.0), weak)]) {
            refit = true;
        }
        let up = state.model.as_ref().map(|m| m.camera().up);
        if segmented_up(ui, &mut right, cy, up)
            && let Some(model) = &mut state.model
        {
            model.toggle_up();
        }
    } else {
        if icon_text_button(ui, "fit", &mut right, cy, &[("Ajuster", medium(12.5), STRONG), ("0", mono(12.0), weak)]) {
            refit = true;
        }
        separator(ui, &mut right, cy);
        zoom_group(ui, &mut right, cy, state, layout.rect.center());
        separator(ui, &mut right, cy);
        let (n, total) = info.position;
        let galley = ui.painter().layout_no_wrap(format!("{n} / {total}"), mono(12.0), TEXT);
        right -= galley.size().x;
        ui.painter().galley(pos2(right, cy - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
        right -= GAP;
    }
    if refit {
        match &mut state.model {
            Some(model) => model.reset(),
            None => state.fitted = true,
        }
        ui.ctx().request_repaint();
    }

    let mut x = b.min.x + GAP;
    let max_x = right - GAP;
    let back = {
        let parts = [
            galley(ui, icons::CARET_LEFT, theme::icon_font(14.0), TEXT),
            galley(ui, "Galerie", medium(13.0), TEXT),
            galley(ui, "Échap", mono(11.5), HINT),
        ];
        let width = 6.0 + parts.iter().map(|g| g.size().x).sum::<f32>() + 6.0 * 2.0 + 10.0;
        let rect = Rect::from_min_size(pos2(x, cy - 14.0), vec2(width, 28.0));
        let response = button(ui, "back", rect, Color32::TRANSPARENT, &parts, 6.0, 6.0);
        x = rect.max.x + GAP;
        response
    };
    if back.clicked() {
        nav = Some(Nav::Close);
    }
    x = place_separator(ui, x, cy);

    let name = fit_text(ui, &info.name, semibold(13.0), STRONG, max_x - x);
    let name_width = name.size().x;
    ui.painter().galley(pos2(x, cy - name.size().y / 2.0), name, Color32::PLACEHOLDER);
    x += name_width + GAP;

    let details = if layout.is_model {
        let tri = state.model.as_ref().map(|m| m.triangle_count()).or(info.triangles);
        let tri = tri.map_or_else(|| "? tri".to_owned(), |n| format!("{} tri", group_thousands(n)));
        format!("{tri} · {} / {}", info.position.0, info.position.1)
    } else {
        info.meta.clone()
    };
    x = weak_text(ui, x, cy, &details, mono(12.0), weak, max_x);
    if !layout.is_model {
        weak_text(ui, x, cy, &info.rel_dir, FontId::proportional(12.5), weak, max_x);
    }
    nav
}

fn weak_text(ui: &Ui, x: f32, cy: f32, text: &str, font: FontId, color: Color32, max_x: f32) -> f32 {
    if text.is_empty() || max_x - x < 24.0 {
        return x;
    }
    let galley = fit_text(ui, text, font, color, max_x - x);
    let width = galley.size().x;
    ui.painter().galley(pos2(x, cy - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    x + width + GAP
}

fn galley(ui: &Ui, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

fn fit_text(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}

fn hovered_fill(fill: Color32) -> Color32 {
    if fill.a() == 0 { Color32::from_white_alpha(22) } else { fill.lerp_to_gamma(Color32::WHITE, 0.1) }
}

/// A rounded button showing `parts` left to right, `pad` from the left edge.
fn button(ui: &mut Ui, name: &str, rect: Rect, fill: Color32, parts: &[Arc<Galley>], pad: f32, gap: f32) -> Response {
    let response = ui.interact(rect, Id::new(("preview-chrome", name)), Sense::click());
    let fill = if response.hovered() { hovered_fill(fill) } else { fill };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    let mut x = rect.min.x + pad;
    for part in parts {
        painter.galley(pos2(x, rect.center().y - part.size().y / 2.0), part.clone(), Color32::PLACEHOLDER);
        x += part.size().x + gap;
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The grey "label + key" button, placed with its right edge at `*right`; true when clicked.
fn icon_text_button(ui: &mut Ui, name: &str, right: &mut f32, cy: f32, parts: &[(&str, FontId, Color32)]) -> bool {
    let galleys: Vec<_> = parts.iter().map(|(t, f, c)| galley(ui, t, f.clone(), *c)).collect();
    let width = 20.0 + galleys.iter().map(|g| g.size().x).sum::<f32>() + 6.0 * (galleys.len() - 1) as f32;
    let rect = Rect::from_min_size(pos2(*right - width, cy - 14.0), vec2(width, 28.0));
    *right = rect.min.x - GAP;
    button(ui, name, rect, BUTTON, &galleys, 10.0, 6.0).clicked()
}

fn separator(ui: &Ui, right: &mut f32, cy: f32) {
    let rect = Rect::from_min_size(pos2(*right - 1.0, cy - 9.0), vec2(1.0, 18.0));
    ui.painter().rect_filled(rect, 0.0, SEPARATOR);
    *right = rect.min.x - GAP;
}

fn place_separator(ui: &Ui, x: f32, cy: f32) -> f32 {
    ui.painter().rect_filled(Rect::from_min_size(pos2(x, cy - 9.0), vec2(1.0, 18.0)), 0.0, SEPARATOR);
    x + 1.0 + GAP
}

/// A glyph button inside a rail; the rail paints its own background.
fn rail_button(ui: &mut Ui, name: &str, rect: Rect, glyph: &str, tip: &str) -> bool {
    let response = ui.interact(rect, Id::new(("preview-chrome", name)), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), Color32::from_white_alpha(22));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, theme::icon_font(14.0), TEXT);
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip).clicked()
}

fn zoom_group(ui: &mut Ui, right: &mut f32, cy: f32, state: &mut PreviewState, center: eframe::egui::Pos2) {
    let rail = Rect::from_min_size(pos2(*right - 108.0, cy - 14.0), vec2(108.0, 28.0));
    *right = rail.min.x - GAP;
    ui.painter().rect_filled(rail, CornerRadius::same(7), RAIL);
    let slot = |x: f32, w: f32| Rect::from_min_size(pos2(rail.min.x + x, cy - 12.0), vec2(w, 24.0));
    let factor = if rail_button(ui, "zoom-out", slot(2.0, 26.0), icons::MINUS, "Zoom arrière") {
        Some(1.0 / NOTCH_FACTOR)
    } else if rail_button(ui, "zoom-in", slot(80.0, 26.0), icons::PLUS, "Zoom avant") {
        Some(NOTCH_FACTOR)
    } else {
        None
    };
    if let (Some(factor), Some(transform)) = (factor, &mut state.transform) {
        transform.zoom_at(factor, center);
        state.fitted = false;
        ui.ctx().request_repaint();
    }
    let percent = state.transform.map_or_else(|| "–".to_owned(), |t| format!("{} %", (t.scale * 100.0).round()));
    ui.painter().text(slot(28.0, 52.0).center(), Align2::CENTER_CENTER, percent, mono(12.0), STRONG);
}

/// "Y-up / Z-up" toggle; true when the inactive side was clicked.
fn segmented_up(ui: &mut Ui, right: &mut f32, cy: f32, up: Option<UpAxis>) -> bool {
    let labels = [("Y-up", UpAxis::Y), ("Z-up", UpAxis::Z)];
    let widths: Vec<f32> = labels.iter().map(|(t, _)| galley(ui, t, medium(12.0), TEXT).size().x + 20.0).collect();
    let total = 4.0 + widths.iter().sum::<f32>() + 2.0;
    let rail = Rect::from_min_size(pos2(*right - total, cy - 14.0), vec2(total, 28.0));
    *right = rail.min.x - GAP;
    ui.painter().rect_filled(rail, CornerRadius::same(7), RAIL);
    let mut x = rail.min.x + 2.0;
    let mut clicked = false;
    for ((text, axis), width) in labels.into_iter().zip(widths) {
        let rect = Rect::from_min_size(pos2(x, cy - 12.0), vec2(width, 24.0));
        let active = up == Some(axis);
        let response = ui.interact(rect, Id::new(("preview-chrome", text)), Sense::click());
        if active {
            ui.painter().rect_filled(rect, CornerRadius::same(5), SEPARATOR);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(5), Color32::from_white_alpha(22));
        }
        let color = if active { STRONG } else { theme::DARK.text_weak };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, medium(12.0), color);
        let response = response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Axe vertical (U)");
        clicked |= response.clicked() && !active;
        x += width + 2.0;
    }
    clicked
}

fn arrows(ui: &mut Ui, info: &Info, layout: &Layout) -> Option<Nav> {
    let (n, total) = info.position;
    let (prev_tip, next_tip) = if layout.is_model {
        ("Modèle précédent (←)", "Modèle suivant (→)")
    } else {
        ("Image précédente (←)", "Image suivante (→)")
    };
    let mut nav = None;
    for (name, rect, glyph, tip, enabled, step) in [
        ("prev", layout.prev, icons::CARET_LEFT, prev_tip, n > 1, -1),
        ("next", layout.next, icons::CARET_RIGHT, next_tip, n < total, 1),
    ] {
        let response = ui.interact(rect, Id::new(("preview-chrome", name)), Sense::click());
        let fade = if enabled { 1.0 } else { 0.35 };
        let fill = if response.hovered() && enabled { Color32::from_black_alpha(200) } else { OVERLAY_BG };
        ui.painter().circle_filled(rect.center(), ARROW_SIZE / 2.0, fill.gamma_multiply(fade));
        let ink = Color32::from_rgb(0xE8, 0xE8, 0xEC).gamma_multiply(fade);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, theme::icon_font(18.0), ink);
        let response = response.on_hover_text(tip);
        if enabled && response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            nav = Some(Nav::Step(step));
        }
    }
    nav
}

fn help_pill(ui: &mut Ui, state: &mut PreviewState, layout: &Layout) {
    let ctx = ui.ctx().clone();
    let (now, moved) = ctx.input(|i| (i.time, i.pointer.delta() != Vec2::ZERO));
    if moved {
        state.pointer_moved_at = now;
    }
    let visible = help_visible(now, state.pointer_moved_at);
    if visible {
        let left = HELP_IDLE_SECS - (now - state.pointer_moved_at);
        ctx.request_repaint_after_secs(left.max(0.0) as f32 + 0.02);
    }
    let t = ctx.animate_bool_with_time(Id::new("preview-help"), visible, HELP_FADE_SECS);
    if t <= 0.0 {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(layout.pill, CornerRadius::same(15), BANNER_BG.gamma_multiply(t));
    let mut x = layout.pill.min.x + PILL_PAD;
    let cy = layout.pill.center().y;
    for (key, label) in help_items(layout.is_model) {
        for (text, font, color, gap) in
            [(key, semibold(12.0), STRONG, 5.0), (label, FontId::proportional(12.0), TEXT, PILL_GAP)]
        {
            let g = painter.layout_no_wrap(text.to_string(), font, color.gamma_multiply(t));
            let width = g.size().x;
            painter.galley(pos2(x, cy - g.size().y / 2.0), g, Color32::PLACEHOLDER);
            x += width + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_pill_hides_after_two_idle_seconds() {
        assert!(help_visible(10.0, 10.0));
        assert!(help_visible(11.9, 10.0));
        assert!(!help_visible(12.0, 10.0));
    }

    #[test]
    fn thousands_are_grouped_with_no_break_spaces() {
        assert_eq!(group_thousands(48_212), "48\u{a0}212");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000_000), "1\u{a0}000\u{a0}000");
    }
}
