use eframe::egui::{Align, Color32, Context, Frame, Label, Layout, Margin, RichText, Sense, Ui, vec2};

use super::theme;

use crate::formats::{self, MediaKind};
use crate::library::Entry;

/// French size text with a decimal comma: `1,5 Ko`, `12,0 Mo`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{bytes} o");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    // Compare the rounded value so 1023.96 Ko becomes "1,0 Mo", not "1024,0 Ko".
    while (value * 10.0).round() >= 10240.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
}

/// `12 345 triangles` with a narrow no-break space between thousands.
pub fn triangles_text(count: Option<u32>) -> String {
    let Some(n) = count else { return "? triangles".to_string() };
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('\u{202F}');
        }
        grouped.push(c);
    }
    format!("{grouped} triangle{}", if n == 1 { "" } else { "s" })
}

/// Empty for `.blend` files, whose size says nothing about the scene.
pub fn dims_text(entry: &Entry) -> String {
    match formats::kind(&entry.path) {
        Some(MediaKind::Model) => triangles_text(entry.triangles),
        Some(MediaKind::Blend) => String::new(),
        _ => match entry.dims {
            Some((w, h)) => format!("{w} × {h}"),
            None => "? × ?".to_string(),
        },
    }
}

/// Directory relative to the root; the root itself is shown as ".".
pub fn rel_dir_label(entry: &Entry) -> &str {
    if entry.rel_dir.is_empty() { "." } else { &entry.rel_dir }
}

/// Dimensions and size on one line, shared by the tooltip and the status bar.
pub fn meta_text(entry: &Entry) -> String {
    let dims = dims_text(entry);
    if dims.is_empty() {
        format_size(entry.size)
    } else {
        format!("{dims}  ·  {}", format_size(entry.size))
    }
}

pub fn images_text(count: usize) -> String {
    if count == 1 { "1 image".to_string() } else { format!("{count} images") }
}

pub fn selected_text(count: usize) -> String {
    if count > 1 { format!("{count} sélectionnées") } else { format!("{count} sélectionnée") }
}

pub struct StatusInfo<'a> {
    pub entry: Option<&'a Entry>,
    pub recursive: bool,
    pub total: usize,
    /// Images matching the name filter; `None` when no filter is set.
    pub matching: Option<usize>,
    pub selected: usize,
    pub loading: bool,
    /// The folder watcher is running.
    pub live: bool,
    /// Transient notice; replaces the entry summary while set.
    pub message: Option<&'a str>,
}

pub const HEIGHT: f32 = 30.0;
const TEXT_SIZE: f32 = 12.5;
const MONO_SIZE: f32 = 12.0;

pub fn frame(ctx: &Context) -> Frame {
    Frame::NONE.fill(theme::palette(ctx).status_bg).inner_margin(Margin::symmetric(12, 0))
}

pub fn count_text(total: usize, matching: Option<usize>) -> String {
    match matching {
        Some(n) => format!("{n} / {} (filtre)", images_text(total)),
        None => images_text(total),
    }
}

fn text(s: impl Into<String>, color: Color32) -> RichText {
    RichText::new(s.into()).size(TEXT_SIZE).color(color)
}

fn mono(s: impl Into<String>, color: Color32) -> RichText {
    RichText::new(s.into()).monospace().size(MONO_SIZE).color(color)
}

fn live_dot(ui: &mut Ui, live: bool) {
    let p = theme::palette(ui.ctx());
    let color = if live { p.arrival } else { p.text_weak.gamma_multiply(0.5) };
    ui.spacing_mut().item_spacing.x = 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.label(text("En direct", p.text_weak));
}

fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 14.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::palette(ui.ctx()).border);
}

fn entry_info(ui: &mut Ui, entry: &Entry, recursive: bool) {
    let p = theme::palette(ui.ctx());
    ui.add(Label::new(text(&entry.name, p.text).strong()).truncate());
    let dims = dims_text(entry);
    if !dims.is_empty() {
        ui.label(mono(dims, p.text_muted));
    }
    ui.label(mono(format_size(entry.size), p.text_muted));
    if recursive {
        ui.label(text(rel_dir_label(entry), p.text_weak));
    }
    if entry.error {
        ui.label(text(format!("{} illisible", theme::icons::WARNING), p.error));
    }
}

pub fn show(ui: &mut Ui, info: &StatusInfo) {
    let p = theme::palette(ui.ctx());
    ui.set_height(HEIGHT);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        live_dot(ui, info.live);
        divider(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if info.selected > 0 {
                ui.label(text(selected_text(info.selected), p.accent));
            }
            ui.label(text(count_text(info.total, info.matching), p.text_muted));
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if let Some(message) = info.message {
                    ui.add(Label::new(text(message, p.text)).truncate());
                } else if let Some(entry) = info.entry {
                    entry_info(ui, entry, info.recursive);
                } else if info.loading {
                    ui.label(text("Chargement…", p.text_weak));
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn entry(size: u64, dims: Option<(u32, u32)>, rel_dir: &str) -> Entry {
        Entry {
            path: PathBuf::from("a.png"),
            rel_dir: rel_dir.to_string(),
            name: "a.png".to_string(),
            mtime: SystemTime::UNIX_EPOCH,
            size,
            dims,
            triangles: None,
            no_preview: false,
            error: false,
            arrived_at: None,
        }
    }

    #[test]
    fn sizes_use_french_decimal_comma() {
        assert_eq!(format_size(0), "0 o");
        assert_eq!(format_size(1023), "1023 o");
        assert_eq!(format_size(1024), "1,0 Ko");
        assert_eq!(format_size(1536), "1,5 Ko");
        assert_eq!(format_size(5 * 1024 * 1024 + 512 * 1024), "5,5 Mo");
        assert_eq!(format_size(1024 * 1024 - 1), "1,0 Mo");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3,0 Go");
    }

    #[test]
    fn triangle_text() {
        assert_eq!(triangles_text(Some(12345)), "12\u{202F}345 triangles");
        assert_eq!(triangles_text(Some(1)), "1 triangle");
        assert_eq!(triangles_text(None), "? triangles");
    }

    #[test]
    fn blend_meta_has_no_dims() {
        let mut e = entry(2048, None, "");
        e.name = "a.blend".to_string();
        e.path = PathBuf::from("a.blend");
        assert_eq!(meta_text(&e), "2,0 Ko");
    }

    #[test]
    fn model_meta_shows_triangles() {
        let mut e = entry(2048, Some((1024, 1024)), "");
        e.path = PathBuf::from("a.glb");
        e.triangles = Some(12);
        assert_eq!(meta_text(&e), "12 triangles  ·  2,0 Ko");
    }

    #[test]
    fn count_text_marks_the_filter() {
        assert_eq!(count_text(328, None), "328 images");
        assert_eq!(count_text(48, Some(17)), "17 / 48 images (filtre)");
    }

    #[test]
    fn counts_agree_in_number() {
        assert_eq!(images_text(1), "1 image");
        assert_eq!(images_text(12), "12 images");
        assert_eq!(selected_text(0), "0 sélectionnée");
        assert_eq!(selected_text(3), "3 sélectionnées");
    }
}
