#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

struct EmptyApp;

impl eframe::App for EmptyApp {
    fn ui(&mut self, _ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {}
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    eframe::run_native(
        "image-moz",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(EmptyApp))),
    )
}
