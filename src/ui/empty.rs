//! The window with no tab: logo, call to action and a hint.

use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2,
    vec2,
};

use super::theme::{self, icons};

const LOGO_SIZE: f32 = 56.0;
const BUTTON_H: f32 = 36.0;

/// True when the user asked to open a folder.
pub fn show(ui: &mut Ui) -> bool {
    let rect = ui.available_rect_before_wrap();
    let p = theme::palette(ui.ctx());
    ui.painter().rect_filled(rect, 0.0, p.gallery_bg);

    let accent_text = if ui.visuals().dark_mode { Color32::from_rgb(0x0D, 0x1A, 0x2E) } else { Color32::WHITE };
    let title = ui.painter().layout_no_wrap(
        "Aucun dossier ouvert".to_owned(),
        FontId::new(16.0, FontFamily::Name(theme::SEMIBOLD.into())),
        p.text,
    );
    let label = ui.painter().layout_no_wrap(
        "Ouvrir un dossier".to_owned(),
        FontId::new(13.5, FontFamily::Name(theme::SEMIBOLD.into())),
        accent_text,
    );
    let shortcut = ui.painter().layout_no_wrap(
        "Ctrl+T".to_owned(),
        FontId::new(12.0, FontFamily::Monospace),
        accent_text.gamma_multiply(0.7),
    );
    let hint = ui.painter().layout_no_wrap(
        "ou clic droit sur un dossier dans l'Explorateur › Ouvrir dans Image Moz".to_owned(),
        FontId::proportional(12.5),
        p.text_weak,
    );

    let button_w = 16.0 + 16.0 + 8.0 + label.size().x + 8.0 + shortcut.size().x + 16.0;
    let total = LOGO_SIZE + 14.0 + title.size().y + 14.0 + BUTTON_H + 14.0 + hint.size().y;
    let mut y = rect.center().y - total / 2.0;
    let cx = rect.center().x;

    draw_logo(ui, pos2(cx - LOGO_SIZE / 2.0, y), p.text_weak.gamma_multiply(0.4));
    y += LOGO_SIZE + 14.0;
    ui.painter().galley(pos2(cx - title.size().x / 2.0, y), title.clone(), Color32::PLACEHOLDER);
    y += title.size().y + 14.0;

    let button = Rect::from_min_size(pos2(cx - button_w / 2.0, y), vec2(button_w, BUTTON_H));
    let response = ui.interact(button, Id::new("empty-open"), Sense::click());
    let fill = if response.hovered() { p.accent.lerp_to_gamma(Color32::WHITE, 0.12) } else { p.accent };
    ui.painter().rect_filled(button, CornerRadius::same(8), fill);
    let mid = button.center().y;
    ui.painter().text(
        pos2(button.min.x + 16.0, mid),
        Align2::LEFT_CENTER,
        icons::FOLDER_OPEN,
        theme::icon_font(16.0),
        accent_text,
    );
    let mut x = button.min.x + 16.0 + 16.0 + 8.0;
    ui.painter().galley(pos2(x, mid - label.size().y / 2.0), label.clone(), Color32::PLACEHOLDER);
    x += label.size().x + 8.0;
    ui.painter().galley(pos2(x, mid - shortcut.size().y / 2.0), shortcut, Color32::PLACEHOLDER);
    y += BUTTON_H + 14.0;
    ui.painter().galley(pos2(cx - hint.size().x / 2.0, y), hint, Color32::PLACEHOLDER);

    ui.allocate_rect(rect, Sense::hover());
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// Outline of the app mosaic: one tall tile beside two stacked ones, on a 16-unit grid.
fn draw_logo(ui: &Ui, origin: Pos2, color: Color32) {
    let unit = LOGO_SIZE / 16.0;
    let stroke = Stroke::new(0.8 * unit, color);
    for (x, y, w, h, r) in [(1.0, 1.0, 8.0, 14.0, 2.0), (10.5, 1.0, 4.5, 6.2, 1.5), (10.5, 8.8, 4.5, 6.2, 1.5)] {
        let rect = Rect::from_min_size(origin + vec2(x, y) * unit, vec2(w, h) * unit);
        ui.painter().rect_stroke(rect, CornerRadius::same((r * unit) as u8), stroke, StrokeKind::Middle);
    }
}
