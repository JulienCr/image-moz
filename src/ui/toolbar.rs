use eframe::egui::{
    Align, Align2, Color32, ComboBox, CornerRadius, FontFamily, FontId, Frame, Id, Layout, Margin, Rect, Response,
    RichText, Sense, Shadow, Slider, Stroke, StrokeKind, TextEdit, TextFormat, Tooltip, Ui, UiBuilder, epaint::Shape, pos2,
    text::LayoutJob, vec2,
};

use super::theme::{self, Palette, icons};
use crate::config::{Config, FolderSettings, MAX_SIZE, MIN_SIZE};
use crate::layout::{BentoRule, LayoutKind};
use crate::library::SortOrder;

pub const HEIGHT: f32 = 44.0;
const CONTROL: f32 = 28.0;

#[derive(Default)]
pub struct ToolbarEvents {
    pub open: bool,
    /// The recursive toggle flipped: the folder must be re-read and re-watched.
    pub recursive_changed: bool,
    /// A global setting changed.
    pub config_changed: bool,
}

/// What the window width allows the toolbar to show.
#[derive(Debug, PartialEq)]
struct Responsive {
    slider_icons: bool,
    filter_width: f32,
    chip_labels: bool,
    bento_prefix: bool,
}

fn responsive(width: f32) -> Responsive {
    Responsive {
        slider_icons: width >= 1100.0,
        filter_width: if width >= 1100.0 { 240.0 } else { 180.0 },
        chip_labels: width >= 960.0,
        bento_prefix: width >= 960.0,
    }
}

fn sort_label(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::DateDesc => "Date ↓",
        SortOrder::DateAsc => "Date ↑",
        SortOrder::NameAsc => "Nom A→Z",
        SortOrder::NameDesc => "Nom Z→A",
    }
}

fn sort_icon(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::DateDesc | SortOrder::NameDesc => icons::SORT_DESCENDING,
        SortOrder::DateAsc | SortOrder::NameAsc => icons::SORT_ASCENDING,
    }
}

fn rule_label(rule: BentoRule) -> &'static str {
    match rule {
        BentoRule::Recent => "Récentes",
        BentoRule::Pinned => "Épinglées",
        BentoRule::Resolution => "Plus grandes",
    }
}

fn medium() -> FontId {
    FontId::new(13.0, FontFamily::Name(theme::MEDIUM.into()))
}

fn separator(ui: &mut Ui, p: &Palette) {
    ui.add_space(12.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
    ui.add_space(12.0);
}

struct Button<'a> {
    icon: &'a str,
    icon_size: f32,
    label: Option<&'a str>,
    selected: bool,
    filled: bool,
}

/// Ghost button: transparent at rest, `hover_bg` on hover; `filled` gives it a resting fill and border.
fn button(ui: &mut Ui, b: Button) -> Response {
    let p = theme::palette(ui.ctx());
    let enabled = ui.is_enabled();
    let color = if b.selected { p.active_text } else if b.filled { p.text } else { p.text_muted };
    let icon = ui.painter().layout_no_wrap(b.icon.to_owned(), theme::icon_font(b.icon_size), color);
    let label = b.label.map(|l| ui.painter().layout_no_wrap(l.to_owned(), medium(), color));
    let pad = if label.is_some() { 10.0 } else { (CONTROL - icon.size().x) / 2.0 };
    let width = pad * 2.0 + icon.size().x + label.as_ref().map_or(0.0, |l| 6.0 + l.size().x);
    let (rect, response) = ui.allocate_exact_size(vec2(width, CONTROL), Sense::click());
    let fill = if b.selected {
        p.active_bg
    } else if response.hovered() && enabled {
        p.hover_bg
    } else if b.filled {
        p.button_fill
    } else {
        Color32::TRANSPARENT
    };
    let radius = CornerRadius::same(6);
    ui.painter().rect_filled(rect, radius, fill);
    if b.filled {
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);
    }
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    let mut x = rect.left() + pad;
    ui.painter().galley(pos2(x, rect.center().y - icon.size().y / 2.0), icon.clone(), fade(color));
    x += icon.size().x + 6.0;
    if let Some(label) = label {
        ui.painter().galley(pos2(x, rect.center().y - label.size().y / 2.0), label, fade(color));
    }
    response
}

/// Icon-only segmented control on a `field_bg` rail; returns the index clicked.
fn segmented(ui: &mut Ui, items: &[(&str, &str)], selected: usize) -> Option<usize> {
    let p = theme::palette(ui.ctx());
    let dark = ui.visuals().dark_mode;
    let width = 4.0 + items.len() as f32 * 32.0 + (items.len() - 1) as f32 * 2.0;
    let (rail, _) = ui.allocate_exact_size(vec2(width, CONTROL), Sense::hover());
    ui.painter().rect_filled(rail, CornerRadius::same(8), p.field_bg);
    let mut clicked = None;
    for (i, (icon, tip)) in items.iter().enumerate() {
        let min = rail.min + vec2(2.0 + i as f32 * 34.0, 1.0);
        let rect = Rect::from_min_size(min, vec2(32.0, 26.0));
        let response = ui.interact(rect, Id::new(("layout_segment", i)), Sense::click()).on_hover_text(*tip);
        let radius = CornerRadius::same(6);
        let color = if i == selected {
            let (fill, text) =
                if dark { (p.control_bg, Color32::from_rgb(242, 242, 245)) } else { (Color32::WHITE, p.text) };
            if !dark {
                let shadow = Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(36) };
                ui.painter().add(Shape::from(shadow.as_shape(rect, radius)));
            }
            ui.painter().rect_filled(rect, radius, fill);
            text
        } else {
            if response.hovered() {
                ui.painter().rect_filled(rect, radius, p.hover_bg);
            }
            p.text_weak
        };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, *icon, theme::icon_font(16.0), color);
        if response.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

fn caret(ui: &Ui, rect: Rect, color: Color32) {
    let center = rect.center() + vec2(2.0, 0.0);
    ui.painter().text(center, Align2::CENTER_CENTER, icons::CARET_DOWN, theme::icon_font(10.0), color);
}

fn combo<R>(
    ui: &mut Ui,
    id: &str,
    text: impl Into<eframe::egui::WidgetText>,
    width: f32,
    items: impl FnOnce(&mut Ui) -> R,
) {
    let color = theme::palette(ui.ctx()).text_weak;
    ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(width)
        .icon(move |ui, rect, _, _| caret(ui, rect, color))
        .show_ui(ui, items);
}

fn small_square(ui: &mut Ui, side: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    ui.painter().rect_stroke(rect.shrink(1.0), CornerRadius::same(2), Stroke::new(1.5, color), StrokeKind::Inside);
}

/// Filter field with a magnifier and a round clear button; returns whether it holds the focus.
fn filter_field(ui: &mut Ui, filter: &mut String, width: f32) {
    let p = theme::palette(ui.ctx());
    let dark = ui.visuals().dark_mode;
    let id = Id::new("name_filter");
    let focused = ui.memory(|m| m.has_focus(id));
    let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL), Sense::hover());
    let active = focused || !filter.is_empty();
    let radius = CornerRadius::same(6);
    let fill = if active && !dark { Color32::WHITE } else { p.field_bg };
    if focused && !dark {
        let halo = Stroke::new(3.0, p.accent.gamma_multiply(0.15));
        ui.painter().rect_stroke(rect, radius, halo, StrokeKind::Outside);
    }
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if active { p.accent } else { p.border }),
        StrokeKind::Inside,
    );
    let inner = Rect::from_min_max(rect.min + vec2(10.0, 0.0), rect.max - vec2(if filter.is_empty() { 10.0 } else { 6.0 }, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(icons::MAGNIFYING_GLASS).font(theme::icon_font(14.0)).color(p.text_weak));
        let reserved = if filter.is_empty() { 0.0 } else { 18.0 + 8.0 };
        ui.add(
            TextEdit::singleline(filter)
                .id(id)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .hint_text("Filtrer par nom…")
                .desired_width(ui.available_width() - reserved),
        );
        if !filter.is_empty() {
            let (clear, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
            let fill = if dark { Color32::from_rgb(52, 52, 59) } else { Color32::from_rgb(212, 212, 216) };
            ui.painter().circle_filled(clear.center(), 9.0, fill);
            ui.painter().text(clear.center(), Align2::CENTER_CENTER, icons::X, theme::icon_font(8.0), p.text);
            if response.on_hover_text("Effacer le filtre").clicked() {
                filter.clear();
            }
        }
    });
}

fn size_slider(ui: &mut Ui, config: &mut Config, p: &Palette) -> bool {
    ui.spacing_mut().slider_width = 120.0;
    ui.spacing_mut().slider_rail_height = 4.0;
    ui.visuals_mut().selection.bg_fill = p.accent;
    let slider = Slider::new(&mut config.target_size, MIN_SIZE..=MAX_SIZE)
        .step_by(1.0)
        .show_value(false)
        .trailing_fill(true)
        .handle_shape(eframe::egui::style::HandleShape::Circle);
    let response = ui.add(slider);
    let text = format!("Taille cible : {} px", config.target_size);
    if response.dragged() {
        Tooltip::always_open(ui.ctx().clone(), response.layer_id, response.id, eframe::egui::PopupAnchor::Pointer)
            .show(|ui| ui.label(text));
    } else {
        response.clone().on_hover_text(text);
    }
    response.changed()
}

/// `folder` holds the active tab's settings; its controls are disabled when no tab is open.
pub fn show(
    ui: &mut Ui,
    config: &mut Config,
    folder: &mut FolderSettings,
    filter: &mut String,
    has_tab: bool,
) -> ToolbarEvents {
    let mut events = ToolbarEvents::default();
    let p = theme::palette(ui.ctx());
    let narrow = responsive(ui.ctx().viewport_rect().width());
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
        let open = Button { icon: icons::FOLDER_OPEN, icon_size: 16.0, label: Some("Ouvrir"), selected: false, filled: true };
        if button(ui, open).on_hover_text("Ouvrir un dossier").clicked() {
            events.open = true;
        }
        separator(ui, p);

        ui.add_enabled_ui(has_tab, |ui| {
            let label = |text| narrow.chip_labels.then_some(text);
            let recursive = Button {
                icon: icons::TREE_STRUCTURE,
                icon_size: 14.0,
                label: label("Récursif"),
                selected: folder.recursive,
                filled: false,
            };
            if button(ui, recursive).on_hover_text("Récursif").clicked() {
                folder.recursive = !folder.recursive;
                events.recursive_changed = true;
            }
            ui.add_enabled_ui(folder.recursive, |ui| {
                let flat =
                    Button { icon: icons::LIST, icon_size: 14.0, label: label("À plat"), selected: folder.flat, filled: false };
                if button(ui, flat).on_hover_text("À plat : une seule liste, sans regroupement par dossier").clicked() {
                    folder.flat = !folder.flat;
                }
            });
        });
        separator(ui, p);

        let kinds = [LayoutKind::Grid, LayoutKind::Justified, LayoutKind::Bento];
        let items = [
            (icons::GRID_FOUR, "Grille (1)"),
            (icons::ROWS, "Justifié (2)"),
            (icons::LAYOUT, "Bento (3)"),
        ];
        let current = kinds.iter().position(|k| *k == config.layout).unwrap_or(0);
        if let Some(i) = segmented(ui, &items, current)
            && kinds[i] != config.layout
        {
            config.layout = kinds[i];
            events.config_changed = true;
        }
        ui.add_space(8.0);

        ui.add_enabled_ui(has_tab, |ui| {
            let mut job = LayoutJob::default();
            let color = p.text_muted;
            job.append(sort_icon(folder.sort), 0.0, TextFormat::simple(theme::icon_font(14.0), color));
            job.append(sort_label(folder.sort), 6.0, TextFormat::simple(medium(), color));
            combo(ui, "sort_order", job, 100.0, |ui| {
                for sort in [SortOrder::DateDesc, SortOrder::DateAsc, SortOrder::NameAsc, SortOrder::NameDesc] {
                    ui.selectable_value(&mut folder.sort, sort, sort_label(sort));
                }
            });
        });
        if config.layout == LayoutKind::Bento {
            let mut job = LayoutJob::default();
            let regular = FontId::proportional(13.0);
            if narrow.bento_prefix {
                job.append("Grandes : ", 0.0, TextFormat::simple(regular, p.text_weak));
            }
            job.append(rule_label(config.bento_rule), 0.0, TextFormat::simple(medium(), p.text));
            let width = if narrow.bento_prefix { 150.0 } else { 90.0 };
            combo(ui, "bento_rule", job, width, |ui| {
                for rule in [BentoRule::Recent, BentoRule::Pinned, BentoRule::Resolution] {
                    events.config_changed |= ui.selectable_value(&mut config.bento_rule, rule, rule_label(rule)).changed();
                }
            });
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(has_tab, |ui| filter_field(ui, filter, narrow.filter_width));
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Right to left: the large icon is added first, so the order below is reversed.
                    if narrow.slider_icons {
                        small_square(ui, 16.0, p.text_weak);
                        ui.add_space(8.0);
                    }
                    events.config_changed |= size_slider(ui, config, p);
                    if narrow.slider_icons {
                        ui.add_space(8.0);
                        small_square(ui, 12.0, p.text_weak);
                    }
                });
            });
        });
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_follow_the_spec() {
        let wide = responsive(1280.0);
        assert!(wide.slider_icons && wide.chip_labels && wide.bento_prefix);
        assert_eq!(wide.filter_width, 240.0);

        let medium = responsive(1000.0);
        assert!(!medium.slider_icons && medium.chip_labels);
        assert_eq!(medium.filter_width, 180.0);

        let small = responsive(900.0);
        assert!(!small.chip_labels && !small.bento_prefix);
    }

    #[test]
    fn breakpoints_are_inclusive_at_the_threshold() {
        assert!(responsive(1100.0).slider_icons);
        assert!(!responsive(1099.0).slider_icons);
        assert!(responsive(960.0).chip_labels);
        assert!(!responsive(959.0).chip_labels);
    }
}
