use eframe::egui::{
    Align2, Color32, CursorIcon, Event, FontId, MouseWheelUnit, Rect, Sense, TextureHandle, Ui, Vec2, pos2,
};

use crate::preview::{PreviewState, ViewTransform};

const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
pub const BACKGROUND: Color32 = Color32::from_gray(18);
const NOTCH_FACTOR: f32 = 1.1;
/// Touchpads report points; this many make one wheel notch.
const POINTS_PER_NOTCH: f32 = 40.0;

/// Draws the preview in the remaining space; `thumb` stands in until the original is decoded.
pub fn show(ui: &mut Ui, state: &mut PreviewState, thumb: Option<&TextureHandle>) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, BACKGROUND);

    let texture = state.image.as_ref().or(thumb);
    if let Some(message) = &state.error {
        let text = format!("Image illisible : {message}");
        painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(16.0), Color32::GRAY);
        return;
    }
    let Some(size) = state.display_size.or_else(|| texture.map(TextureHandle::size_vec2)) else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Chargement…", FontId::proportional(16.0), Color32::GRAY);
        return;
    };

    let mut transform = match state.transform {
        Some(t) if !state.fitted => t,
        _ => ViewTransform::fit(size, rect),
    };
    if let Some(cursor) = response.hover_pos().or(ui.input(|i| i.pointer.hover_pos())) {
        let factor = ui.input(|i| wheel_factor(&i.events));
        if factor != 1.0 && rect.contains(cursor) {
            transform.zoom_at(factor, cursor);
            state.fitted = false;
        }
    }
    if response.dragged() {
        transform.pan(response.drag_delta());
        state.fitted = false;
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    state.transform = Some(transform);

    if let Some(texture) = texture {
        painter.image(texture.id(), transform.image_rect(size), FULL_UV, Color32::WHITE);
    }
    if state.image.is_none() {
        let pos = rect.center_bottom() - Vec2::new(0.0, 24.0);
        painter.text(pos, Align2::CENTER_CENTER, "Chargement…", FontId::proportional(14.0), Color32::GRAY);
    }
}

/// Combined zoom factor of this frame's wheel events: 1.1 per notch.
fn wheel_factor(events: &[Event]) -> f32 {
    let notches: f32 = events
        .iter()
        .filter_map(|e| match e {
            Event::MouseWheel { unit, delta, .. } => Some(match unit {
                MouseWheelUnit::Line => delta.y,
                MouseWheelUnit::Point => delta.y / POINTS_PER_NOTCH,
                MouseWheelUnit::Page => delta.y * 10.0,
            }),
            _ => None,
        })
        .sum();
    NOTCH_FACTOR.powf(notches)
}
