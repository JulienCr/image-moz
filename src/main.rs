#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui_wgpu;

use image_moz::app::{ImageMozApp, Startup};
use image_moz::config::Config;
use image_moz::instance::{self, Acquired, Names, Primary};
use image_moz::platform;
use image_moz::tab::absolute;

const TITLE: &str = "Image Moz";

/// First CLI argument as an absolute path; never canonicalized.
fn cli_path() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(|arg| absolute(&PathBuf::from(arg)))
}

/// `None` when this launch handed its argument to a running instance and should exit.
fn claim_instance(cli: Option<&std::path::Path>) -> Option<Option<Primary>> {
    let names = Names::for_current_user();
    match instance::acquire(&names) {
        Ok(Acquired::First(primary)) => Some(Some(primary)),
        Ok(Acquired::Existing) => match instance::forward(&names, cli) {
            Ok(()) => None,
            Err(e) => {
                log::warn!("cannot reach the running instance, starting standalone: {e}");
                Some(None)
            }
        },
        Err(e) => {
            log::warn!("single-instance check failed, starting standalone: {e}");
            Some(None)
        }
    }
}

/// Keeps egui's device limits but lifts the buffer and texture caps to the adapter's, so the
/// orbit preview can upload meshes the offscreen thumbnail device already handles.
fn wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut config = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut config.wgpu_setup {
        let egui_descriptor = create.device_descriptor.clone();
        create.device_descriptor = Arc::new(move |adapter| {
            let mut descriptor = egui_descriptor(adapter);
            let (limits, supported) = (&mut descriptor.required_limits, adapter.limits());
            limits.max_buffer_size = supported.max_buffer_size;
            limits.max_texture_dimension_2d = limits.max_texture_dimension_2d.max(supported.max_texture_dimension_2d);
            descriptor
        });
    }
    config
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    platform::enable_dpi_awareness();
    let cli = cli_path();
    let Some(instance) = claim_instance(cli.as_deref()) else { return Ok(()) };
    let target = cli.as_deref().and_then(image_moz::tab::OpenTarget::from_path);
    let startup = Startup::from_config(Config::load(&Config::default_path()));
    let options = eframe::NativeOptions {
        viewport: startup.root_viewport(),
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(ImageMozApp::new(cc, startup, target, instance)))),
    )
}
