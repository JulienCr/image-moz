use eframe::egui::{self, Align, Label, Layout, Ui};

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

pub fn summary(entry: &Entry, recursive: bool) -> String {
    let mut text = format!("{}  ·  {}", entry.name, meta_text(entry));
    if recursive {
        text.push_str(&format!("  ·  {}", rel_dir_label(entry)));
    }
    if entry.error {
        text.push_str("  ·  ⚠ illisible");
    }
    text
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
    /// Transient notice; replaces the entry summary while set.
    pub message: Option<&'a str>,
}

pub fn show(ui: &mut Ui, info: &StatusInfo) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(selected_text(info.selected));
            ui.separator();
            match info.matching {
                Some(n) => ui.label(format!("{n} / {}", images_text(info.total))),
                None => ui.label(images_text(info.total)),
            };
            ui.separator();
            if let Some(message) = info.message {
                ui.add(Label::new(message).truncate());
            } else if let Some(entry) = info.entry {
                ui.add(Label::new(summary(entry, info.recursive)).truncate());
            } else if info.loading {
                ui.label(egui::RichText::new("Chargement…").weak());
            }
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
    fn summary_shows_rel_dir_only_when_recursive() {
        let e = entry(2048, Some((640, 480)), "");
        assert_eq!(summary(&e, false), "a.png  ·  640 × 480  ·  2,0 Ko");
        assert_eq!(summary(&e, true), "a.png  ·  640 × 480  ·  2,0 Ko  ·  .");
        let e = entry(10, None, "sub/deep");
        assert_eq!(summary(&e, true), "a.png  ·  ? × ?  ·  10 o  ·  sub/deep");
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
    fn counts_agree_in_number() {
        assert_eq!(images_text(1), "1 image");
        assert_eq!(images_text(12), "12 images");
        assert_eq!(selected_text(0), "0 sélectionnée");
        assert_eq!(selected_text(3), "3 sélectionnées");
    }
}
