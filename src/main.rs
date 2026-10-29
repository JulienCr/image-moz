#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use image_moz::app::ImageMozApp;

/// First CLI argument as an absolute path; never canonicalized (no `\\?\` prefix).
fn cli_folder() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::args_os().nth(1)?);
    Some(std::path::absolute(&path).unwrap_or(path))
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    let cli_folder = cli_folder();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("image-moz")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "image-moz",
        options,
        Box::new(move |cc| Ok(Box::new(ImageMozApp::new(cc, cli_folder)))),
    )
}
