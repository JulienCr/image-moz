#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use image_moz::app::ImageMozApp;
use image_moz::tab::{OpenTarget, absolute};

const TITLE: &str = "Image Moz";

/// First CLI argument as an absolute path; never canonicalized.
fn cli_target() -> Option<OpenTarget> {
    let arg = std::env::args_os().nth(1)?;
    OpenTarget::from_path(&absolute(&PathBuf::from(arg)))
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    let target = cli_target();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(ImageMozApp::new(cc, target)))),
    )
}
