use std::path::Path;

use eframe::egui::{
    Align2, Color32, CursorIcon, Event, FontId, MouseWheelUnit, PointerButton, Rect, Response, Sense, TextureHandle, Ui,
    Vec2, pos2,
};
use eframe::egui_wgpu::RenderState;

use super::preview_chrome as chrome;
use super::theme;
use crate::formats::{self, MediaKind};
use crate::model::gpu::ModelRenderer;
use crate::preview::{PreviewState, ViewTransform, fit_area};

/// Space kept around a fitted image, below the banner.
const FIT_MARGIN: f32 = 8.0;
const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
pub(super) const NOTCH_FACTOR: f32 = 1.1;
/// Touchpads report points; this many make one wheel notch.
const POINTS_PER_NOTCH: f32 = 40.0;

/// What a 3D model needs to be drawn on eframe's device.
pub type Gpu<'a> = Option<(&'a RenderState, &'a ModelRenderer)>;

/// What the chrome prints about the previewed file.
pub struct Info {
    pub name: String,
    /// "2048 × 1365 · 1,9 Mo".
    pub meta: String,
    /// "modifié hier 09:10 · créé 12 nov. 2026 14:32".
    pub dates: String,
    pub rel_dir: String,
    /// Fallback for a model whose scene is not loaded yet.
    pub triangles: Option<u32>,
    /// 1-based position in the gallery order, and the order's length.
    pub position: (usize, usize),
}

/// What the chrome asks the tab to do.
pub enum Nav {
    Close,
    Step(isize),
}

/// Draws the preview in the remaining space; `thumb` stands in until the original is decoded.
pub fn show(ui: &mut Ui, state: &mut PreviewState, thumb: Option<&TextureHandle>, gpu: Gpu, info: &Info) -> Option<Nav> {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let help_t = chrome::help_alpha(ui, state);
    let layout = chrome::Layout::new(ui, rect, state.model.is_some(), help_t);
    let blocked = layout.pointer_over(ui);
    // Registered after the image response, these sit on top and swallow presses meant for pan or zoom.
    layout.block_input(ui, response.id);
    paint_content(ui, &response, rect, state, thumb, gpu, blocked);
    chrome::show(ui, state, info, &layout)
}

fn paint_content(
    ui: &mut Ui,
    response: &Response,
    rect: Rect,
    state: &mut PreviewState,
    thumb: Option<&TextureHandle>,
    gpu: Gpu,
    blocked: bool,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::palette(ui.ctx()).preview_bg);

    let texture = state.image.as_ref().or(thumb);
    if let Some(message) = &state.error {
        let text = error_text(&state.path, message);
        painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(16.0), Color32::GRAY);
        return;
    }
    if state.model.is_some() {
        show_model(ui, response, rect, state, gpu, blocked);
        return;
    }
    let Some(size) = state.display_size.or_else(|| texture.map(TextureHandle::size_vec2)) else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Chargement…", FontId::proportional(16.0), Color32::GRAY);
        return;
    };

    let mut transform = match state.transform {
        Some(t) if !state.fitted => t,
        _ => ViewTransform::fit(size, fit_area(rect, chrome::BANNER_H, FIT_MARGIN)),
    };
    if let Some(cursor) = response.hover_pos().or(ui.input(|i| i.pointer.hover_pos())) {
        let factor = ui.input(|i| wheel_factor(&i.events));
        if factor != 1.0 && rect.contains(cursor) && !blocked {
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

fn error_text(path: &Path, message: &str) -> String {
    let what = if formats::kind(path) == Some(MediaKind::Model) { "Modèle" } else { "Image" };
    format!("{what} illisible : {message}")
}

/// Middle drag orbits, Shift + middle drag pans, the wheel dollies; the left button does nothing.
fn show_model(ui: &mut Ui, response: &Response, rect: Rect, state: &mut PreviewState, gpu: Gpu, blocked: bool) {
    let painter = ui.painter_at(rect);
    let Some((rs, renderer)) = gpu else {
        let text = "Rendu 3D indisponible";
        painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(16.0), Color32::GRAY);
        return;
    };
    let Some(model) = &mut state.model else { return };
    let view = fit_area(rect, chrome::BANNER_H, 0.0);
    if response.dragged_by(PointerButton::Middle) {
        let shift = ui.input(|i| i.modifiers.shift);
        let delta = response.drag_delta();
        if shift {
            model.pan(delta, view.size());
        } else {
            model.orbit(delta);
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    let over_view = response.hover_pos().or(ui.input(|i| i.pointer.hover_pos())).is_some_and(|p| rect.contains(p));
    let notches = if over_view && !blocked { ui.input(|i| wheel_notches(&i.events)) } else { 0.0 };
    if notches != 0.0 {
        model.dolly(notches);
    }
    model.paint(ui, view, rs, renderer);
}

/// Wheel travel of this frame in notches, whatever unit the device reports.
fn wheel_notches(events: &[Event]) -> f32 {
    events
        .iter()
        .filter_map(|e| match e {
            Event::MouseWheel { unit, delta, .. } => Some(match unit {
                MouseWheelUnit::Line => delta.y,
                MouseWheelUnit::Point => delta.y / POINTS_PER_NOTCH,
                MouseWheelUnit::Page => delta.y * 10.0,
            }),
            _ => None,
        })
        .sum()
}

/// Combined zoom factor of this frame's wheel events: 1.1 per notch.
fn wheel_factor(events: &[Event]) -> f32 {
    NOTCH_FACTOR.powf(wheel_notches(events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Modifiers, TouchPhase, vec2};

    fn wheel(unit: MouseWheelUnit, y: f32) -> Event {
        Event::MouseWheel { unit, delta: vec2(0.0, y), phase: TouchPhase::Move, modifiers: Modifiers::NONE }
    }

    #[test]
    fn error_text_names_what_could_not_be_read() {
        assert_eq!(error_text(Path::new("a.glb"), "bad"), "Modèle illisible : bad");
        assert_eq!(error_text(Path::new("a.png"), "bad"), "Image illisible : bad");
    }

    #[test]
    fn wheel_notches_units() {
        assert_eq!(wheel_notches(&[wheel(MouseWheelUnit::Line, 2.0)]), 2.0);
        assert_eq!(wheel_notches(&[wheel(MouseWheelUnit::Point, 80.0)]), 2.0);
        assert_eq!(wheel_notches(&[wheel(MouseWheelUnit::Page, 1.0)]), 10.0);
        let both = [wheel(MouseWheelUnit::Line, 1.0), wheel(MouseWheelUnit::Line, -3.0), Event::Copy];
        assert_eq!(wheel_notches(&both), -2.0);
    }
}
