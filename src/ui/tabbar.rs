use eframe::egui::{
    Align2, Color32, Context, CornerRadius, FontFamily, FontId, Id, LayerId, Order, PointerButton, Pos2, Rect,
    ScrollArea, Sense, Shadow, Stroke, StrokeKind, Ui, Vec2, epaint::Shape, pos2, scroll_area::ScrollBarVisibility,
    text::LayoutJob, vec2,
};

use super::theme::{self, Palette, icons};
use crate::tab::Tab;

pub const HEIGHT: f32 = 36.0;
const MAX_LABEL_WIDTH: f32 = 180.0;
const TAB_HEIGHT: f32 = 30.0;
const GAP: f32 = 2.0;
const PAD: f32 = 12.0;
const CLOSE: f32 = 20.0;
const NEW_TAB: f32 = 28.0;
const PILL_TEXT: Color32 = Color32::from_rgb(20, 20, 22);

#[derive(Default)]
pub struct TabBarEvents {
    pub activate: Option<usize>,
    pub close: Option<usize>,
    /// Index of the tab the user began dragging.
    pub drag_started: Option<usize>,
    /// The "+" button was pressed.
    pub new_tab: bool,
    /// Screen-space rect of every tab (label and close button), in order.
    pub tab_rects: Vec<Rect>,
}

/// What an ongoing tab drag changes in this bar's drawing.
#[derive(Default, Clone, Copy)]
pub struct DragView {
    /// Tab being dragged, shown dimmed wherever it still is.
    pub dragged: Option<u64>,
    /// Insertion index to mark when the drag would drop into this bar.
    pub marker: Option<usize>,
}

/// Where dropping the dragged tab would put it, for the pill under the cursor.
pub enum GhostKind {
    Reorder,
    NewWindow,
    /// Window number, counted from 1.
    OtherWindow(usize),
}

pub fn title(tab: &Tab) -> String {
    match tab.unseen {
        0 => tab.label(),
        n => format!("{} ({n})", tab.label()),
    }
}

fn medium() -> FontId {
    FontId::new(13.0, FontFamily::Name(theme::MEDIUM.into()))
}

/// Single-row job that ends in "…" instead of growing past `MAX_LABEL_WIDTH`.
fn truncated(text: &str, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), medium(), color);
    job.wrap.max_width = MAX_LABEL_WIDTH;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

fn tab_fill(p: &Palette, active: bool, hovered: bool) -> Color32 {
    if active {
        p.toolbar_bg
    } else if hovered {
        p.chrome_bg.lerp_to_gamma(p.toolbar_bg, 0.5)
    } else {
        Color32::TRANSPARENT
    }
}

fn pill_width(text_width: f32) -> f32 {
    (text_width + 10.0).max(18.0)
}

pub fn show(ui: &mut Ui, tabs: &[Tab], active: usize, drag: DragView) -> TabBarEvents {
    let mut events = TabBarEvents::default();
    let p = theme::palette(ui.ctx());
    ScrollArea::horizontal()
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let row = ui.available_rect_before_wrap();
            let band_top = row.top() + HEIGHT - TAB_HEIGHT;
            let mut x = row.left();
            for (index, tab) in tabs.iter().enumerate() {
                let is_active = index == active;
                let base = if is_active { p.text } else { p.text_weak };
                let label_color = if drag.dragged == Some(tab.id) { base.gamma_multiply(0.4) } else { base };
                let galley = ui.painter().layout_job(truncated(&tab.label(), label_color));
                let pill = (!is_active && tab.unseen > 0).then(|| {
                    ui.painter().layout_no_wrap(
                        tab.unseen.to_string(),
                        FontId::new(11.0, FontFamily::Name(theme::SEMIBOLD.into())),
                        PILL_TEXT,
                    )
                });
                let pill_slot = pill.as_ref().map_or(0.0, |g| 8.0 + pill_width(g.size().x));
                let width = PAD + galley.size().x + pill_slot + 8.0 + CLOSE + 8.0;
                let rect = Rect::from_min_size(pos2(x, band_top), vec2(width, TAB_HEIGHT));
                x = rect.right() + GAP;

                let body = ui
                    .interact(rect, Id::new(("tab", tab.id)), Sense::click_and_drag())
                    .on_hover_text(tab.root.display().to_string());
                let pointer_over = ui.rect_contains_pointer(rect);
                let close_rect =
                    Rect::from_center_size(pos2(rect.right() - 8.0 - CLOSE / 2.0, rect.center().y), Vec2::splat(CLOSE));
                // Registered after the body so it wins the hit test where they overlap.
                let close = (is_active || pointer_over)
                    .then(|| ui.interact(close_rect, Id::new(("tab_close", tab.id)), Sense::click()));

                let radius = CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 };
                ui.painter().rect_filled(rect, radius, tab_fill(p, is_active, pointer_over));
                let text_pos = pos2(rect.left() + PAD, rect.center().y - galley.size().y / 2.0);
                let label_right = text_pos.x + galley.size().x;
                ui.painter().galley(text_pos, galley, label_color);
                if let Some(g) = pill {
                    let r = Rect::from_min_size(
                        pos2(label_right + 8.0, rect.center().y - 9.0),
                        vec2(pill_width(g.size().x), 18.0),
                    );
                    ui.painter().rect_filled(r, CornerRadius::same(9), p.arrival);
                    ui.painter().galley(r.center() - g.size() / 2.0, g, PILL_TEXT);
                }
                if let Some(close) = &close {
                    let hovered = close.hovered();
                    if hovered {
                        ui.painter().rect_filled(close_rect, CornerRadius::same(4), p.hover_bg);
                    }
                    let color = if hovered { p.text } else { p.text_weak };
                    ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, icons::X, theme::icon_font(12.0), color);
                }

                if body.drag_started_by(PointerButton::Primary) {
                    events.drag_started = Some(index);
                    events.activate = Some(index);
                } else if body.clicked() {
                    events.activate = Some(index);
                }
                let close_hit = close.is_some_and(|c| c.clicked() || c.middle_clicked());
                if close_hit || body.middle_clicked() {
                    events.close = Some(index);
                }
                events.tab_rects.push(rect);
            }

            let plus = Rect::from_min_size(pos2(x + 2.0, row.top() + HEIGHT - 4.0 - NEW_TAB), Vec2::splat(NEW_TAB));
            let response =
                ui.interact(plus, Id::new("new_tab"), Sense::click()).on_hover_text("Nouvel onglet (Ctrl+T)");
            if response.hovered() {
                ui.painter().rect_filled(plus, CornerRadius::same(6), p.hover_bg);
            }
            let color = if response.hovered() { p.text } else { p.text_weak };
            ui.painter().text(plus.center(), Align2::CENTER_CENTER, icons::PLUS, theme::icon_font(14.0), color);
            events.new_tab = response.clicked();
            ui.allocate_rect(Rect::from_min_max(row.min, pos2(plus.right(), row.top() + HEIGHT)), Sense::hover());

            if let Some(at) = drag.marker {
                paint_marker(ui, &events.tab_rects, at, p);
            }
        });
    events
}

fn paint_marker(ui: &Ui, tabs: &[Rect], at: usize, p: &Palette) {
    let Some(last) = tabs.last() else { return };
    let (x, rect) = match tabs.get(at) {
        Some(next) => (next.min.x - GAP / 2.0, next),
        None => (last.max.x + GAP / 2.0, last),
    };
    let marker = Rect::from_center_size(pos2(x, rect.center().y), vec2(2.0, 24.0));
    ui.painter().rect_filled(marker, CornerRadius::same(1), p.accent);
}

/// The pill that follows the cursor during a drag; every window paints it when the cursor is over it.
pub fn paint_ghost(ctx: &Context, at: Pos2, name: &str, kind: &GhostKind) {
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("tab_drag_ghost")));
    let name_galley = painter.layout_no_wrap(name.to_owned(), medium(), Color32::from_rgb(242, 242, 245));
    let suffix = match kind {
        GhostKind::Reorder => None,
        GhostKind::NewWindow => Some("→ nouvelle fenêtre".to_owned()),
        GhostKind::OtherWindow(n) => Some(format!("→ fenêtre {n}")),
    };
    let weak = Color32::from_rgb(150, 150, 160);
    let suffix = suffix.map(|s| painter.layout_no_wrap(s, FontId::proportional(13.0), weak));
    let icon = suffix.is_some().then(|| {
        painter.layout_no_wrap(icons::IMAGE.to_owned(), theme::icon_font(14.0), Color32::from_rgb(143, 184, 255))
    });
    let (height, pad) = if suffix.is_some() { (30.0, 12.0) } else { (26.0, 10.0) };
    let mut width = pad * 2.0 + name_galley.size().x;
    width += icon.as_ref().map_or(0.0, |i| i.size().x + 8.0);
    width += suffix.as_ref().map_or(0.0, |s| s.size().x + 8.0);

    let rect = Rect::from_min_size(at + vec2(14.0, 14.0), vec2(width, height));
    let radius = CornerRadius::same((height / 2.0) as u8);
    let shadow = Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(110) };
    painter.add(Shape::from(shadow.as_shape(rect, radius)));
    painter.rect(
        rect,
        radius,
        Color32::from_rgb(11, 11, 12),
        Stroke::new(1.0, Color32::from_rgb(52, 52, 59)),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + pad;
    for galley in icon.into_iter().chain(Some(name_galley)).chain(suffix) {
        let w = galley.size().x;
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
        x += w + 8.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pill_is_at_least_a_circle() {
        assert_eq!(pill_width(4.0), 18.0);
        assert_eq!(pill_width(14.0), 24.0);
    }
}
