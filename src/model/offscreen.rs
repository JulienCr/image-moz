//! Dedicated wgpu device that renders model thumbnails off the UI thread.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::wgpu;
use image::RgbaImage;

use super::Scene;
use super::camera::OrbitCamera;
use super::gpu::{GpuScene, ModelRenderer, RenderTarget, try_read_back};

pub const MODEL_RENDER_SIZE: u32 = 1024;

/// How long a failed device build is remembered before `shared()` tries again.
const REBUILD_COOLDOWN: Duration = Duration::from_secs(5);

pub struct Offscreen {
    adapter_name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: ModelRenderer,
    lost: Arc<Mutex<Option<String>>>,
}

impl Offscreen {
    pub fn new(force_fallback: bool) -> Result<Self, String> {
        // Concurrent device creation intermittently hangs on DX12; tests build several.
        static CREATION: Mutex<()> = Mutex::new(());
        let _creating = CREATION.lock().unwrap_or_else(|e| e.into_inner());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: force_fallback,
            ..Default::default()
        }))
        .map_err(|e| format!("no usable GPU adapter: {e}"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("model thumbnails"),
            // The defaults cap buffers at 256 MiB, too small for big meshes.
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| format!("cannot create a GPU device on {}: {e}", info.name))?;

        // The default handler panics; errors are reported through scopes in `render`.
        device.on_uncaptured_error(Arc::new(|e| log::error!("uncaptured wgpu error: {e}")));
        let lost = Arc::new(Mutex::new(None));
        let lost_slot = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            log::error!("model GPU device lost ({reason:?}): {message}");
            *lost_slot.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("{reason:?}: {message}"));
        });
        log::info!(
            "model thumbnails render on {} ({:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );
        let renderer = build_renderer(&device)?;
        Ok(Self {
            adapter_name: info.name,
            device,
            queue,
            renderer,
            lost,
        })
    }

    /// The process-wide renderer: hardware adapter if there is one, otherwise
    /// the software one. A failed build or a lost device is rebuilt on a later call.
    pub fn shared() -> Result<Arc<Offscreen>, String> {
        static SHARED: Mutex<SharedSlot> = Mutex::new(SharedSlot::new(REBUILD_COOLDOWN));
        SHARED.lock().unwrap_or_else(|e| e.into_inner()).get(|| {
            Offscreen::new(false).or_else(|hardware| {
                log::warn!("{hardware}; trying the software adapter");
                Offscreen::new(true)
            })
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Renders `scene` into a square image with a transparent background.
    pub fn render(&self, scene: &Scene, size: u32) -> Result<RgbaImage, String> {
        self.check_lost()?;
        let scopes = [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ]
        .map(|filter| self.device.push_error_scope(filter));
        let result = guard_panics(|| self.render_in_scopes(scene, size));
        // Scopes must pop innermost first.
        let errors: Vec<String> = scopes
            .into_iter()
            .rev()
            .filter_map(|scope| pollster::block_on(scope.pop()))
            .map(|e| e.to_string())
            .collect();
        self.check_lost()?;
        if !errors.is_empty() {
            return Err(format!("GPU error: {}", errors.join("; ")));
        }
        result
    }

    fn render_in_scopes(&self, scene: &Scene, size: u32) -> Result<RgbaImage, String> {
        let limit = self.device.limits().max_texture_dimension_2d;
        if size == 0 || size > limit {
            return Err(format!("render size {size} outside 1..={limit}"));
        }
        let bounds = scene.bounds().ok_or("model has no geometry")?;
        let camera = OrbitCamera::initial(scene).ok_or("model has no geometry")?;
        let gpu = GpuScene::upload(&self.device, &self.queue, &self.renderer, scene);
        let target = RenderTarget::new(&self.device, [size, size]);
        self.renderer
            .render(&self.device, &self.queue, &target, &gpu, &camera, &bounds);
        let mut image = try_read_back(&self.device, &self.queue, target.texture())?;
        unpremultiply_edges(&mut image);
        Ok(image)
    }

    fn check_lost(&self) -> Result<(), String> {
        match &*self.lost.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(why) => Err(format!("GPU device lost: {why}")),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub fn destroy_device(&self) {
        self.device.destroy();
        // The lost callback only runs on the next poll after a destroy.
        let _ = self.device.poll(wgpu::PollType::Poll);
    }
}

/// Builds the renderer under a validation scope, since wgpu would otherwise
/// only log a pipeline or shader failure here.
fn build_renderer(device: &wgpu::Device) -> Result<ModelRenderer, String> {
    capture_validation(device, || ModelRenderer::new(device))
        .map_err(|e| format!("cannot build the model renderer: {e}"))
}

fn capture_validation<T>(device: &wgpu::Device, work: impl FnOnce() -> T) -> Result<T, String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = work();
    match pollster::block_on(scope.pop()) {
        Some(e) => Err(e.to_string()),
        None => Ok(value),
    }
}

/// `Device::poll` panics on some device failures in wgpu 29; report those as errors.
fn guard_panics<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        Err(format!("GPU renderer panicked: {what}"))
    })
}

/// Holds the shared `Offscreen` and decides when to build a new one.
struct SharedSlot {
    current: Option<Arc<Offscreen>>,
    failed: Option<(Instant, String)>,
    cooldown: Duration,
}

impl SharedSlot {
    const fn new(cooldown: Duration) -> Self {
        Self {
            current: None,
            failed: None,
            cooldown,
        }
    }

    fn get(
        &mut self,
        build: impl FnOnce() -> Result<Offscreen, String>,
    ) -> Result<Arc<Offscreen>, String> {
        if let Some(off) = &self.current {
            match off.check_lost() {
                Ok(()) => return Ok(off.clone()),
                Err(why) => log::warn!("{why}; rebuilding the model renderer"),
            }
            self.current = None;
        }
        if let Some((at, error)) = &self.failed
            && at.elapsed() < self.cooldown
        {
            return Err(error.clone());
        }
        match build() {
            Ok(off) => {
                self.failed = None;
                Ok(self.current.insert(Arc::new(off)).clone())
            }
            Err(error) => {
                self.failed = Some((Instant::now(), error.clone()));
                Err(error)
            }
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The MSAA resolve averages model samples with transparent ones, so edge
/// pixels come out premultiplied in linear light; undo that for straight alpha.
fn unpremultiply_edges(image: &mut RgbaImage) {
    for px in image.pixels_mut().filter(|p| p.0[3] > 0 && p.0[3] < 255) {
        let alpha = px.0[3] as f32 / 255.0;
        for c in &mut px.0[..3] {
            let linear = (srgb_to_linear(*c as f32 / 255.0) / alpha).min(1.0);
            *c = (linear_to_srgb(linear) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{fixture, load};

    fn scene(name: &str) -> Scene {
        load(&fixture(name)).unwrap()
    }

    fn assert_red_center(img: &RgbaImage) {
        let (w, h) = img.dimensions();
        let c = img.get_pixel(w / 2, h / 2).0;
        assert!(c[3] == 255 && c[0] > 100 && c[1] < 30 && c[2] < 30, "{c:?}");
    }

    #[test]
    fn renders_red_cube_on_transparent_background() {
        let img = Offscreen::shared()
            .unwrap()
            .render(&scene("cube_nocam.glb"), 128)
            .unwrap();
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_red_center(&img);
    }

    #[test]
    fn texture_reaches_the_image() {
        let img = Offscreen::shared()
            .unwrap()
            .render(&scene("textured.glb"), 128)
            .unwrap();
        let c = img.get_pixel(64, 64).0;
        assert!(c[3] == 255 && c[1] > 100 && c[0] < 30 && c[2] < 30, "{c:?}");
    }

    #[test]
    fn fallback_adapter_renders() {
        let off = match Offscreen::new(true) {
            Ok(off) => off,
            Err(e) => return eprintln!("skipped: no fallback adapter ({e})"),
        };
        let img = off.render(&scene("cube_nocam.glb"), 128).unwrap();
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_red_center(&img);
    }

    #[test]
    fn concurrent_renders_are_all_correct() {
        let off = Offscreen::shared().unwrap();
        let off = &*off;
        let (cube, textured) = (scene("cube_nocam.glb"), scene("textured.glb"));
        std::thread::scope(|s| {
            for t in 0..4 {
                let (cube, textured) = (&cube, &textured);
                s.spawn(move || {
                    for _ in 0..3 {
                        // Alternating scenes would show any state leaking between renders.
                        let red = t % 2 == 0;
                        let img = off.render(if red { cube } else { textured }, 64).unwrap();
                        let c = img.get_pixel(32, 32).0;
                        let ok = if red {
                            c[0] > 100 && c[1] < 30 && c[2] < 30
                        } else {
                            c[1] > 100 && c[0] < 30 && c[2] < 30
                        };
                        assert!(ok && c[3] == 255, "thread {t}: {c:?}");
                    }
                });
            }
        });
    }

    #[test]
    fn file_camera_changes_the_view() {
        let off = Offscreen::shared().unwrap();
        let with = off.render(&scene("cube.glb"), 128).unwrap();
        let without = off.render(&scene("cube_nocam.glb"), 128).unwrap();
        assert_ne!(with.as_raw(), without.as_raw());
    }

    #[test]
    fn file_camera_keeps_the_whole_model_in_a_square_thumbnail() {
        let img = Offscreen::shared()
            .unwrap()
            .render(&scene("cube.glb"), 128)
            .unwrap();
        let last = 127;
        let border_hits = (0..128)
            .flat_map(|i| [(i, 0), (i, last), (0, i), (last, i)])
            .filter(|&(x, y)| img.get_pixel(x, y).0[3] > 0)
            .count();
        assert_eq!(border_hits, 0, "model touches the image border");
    }

    #[test]
    fn unaligned_row_size_keeps_the_image_intact() {
        let img = Offscreen::shared()
            .unwrap()
            .render(&scene("cube_nocam.glb"), 100)
            .unwrap();
        assert_eq!(img.dimensions(), (100, 100));
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_red_center(&img);
    }

    #[test]
    fn silhouette_edges_keep_the_model_color() {
        let img = Offscreen::shared()
            .unwrap()
            .render(&scene("cube_nocam.glb"), 256)
            .unwrap();
        let edge: Vec<_> = img
            .pixels()
            .filter(|p| p.0[3] > 0 && p.0[3] < 255)
            .collect();
        assert!(!edge.is_empty(), "expected antialiased edge pixels");
        for p in edge {
            assert!(p.0[0] > 100 && p.0[1] < 30 && p.0[2] < 30, "{:?}", p.0);
        }
    }

    #[test]
    fn oversized_texture_is_an_error_not_a_panic() {
        let mut s = scene("cube_nocam.glb");
        s.textures.push(RgbaImage::new(100_000, 1));
        s.materials[0].texture = Some(s.textures.len() - 1);
        let err = Offscreen::shared().unwrap().render(&s, 64);
        assert!(err.is_err());
    }

    #[test]
    fn lost_device_is_an_error_not_a_panic() {
        let off = Offscreen::new(false)
            .or_else(|_| Offscreen::new(true))
            .unwrap();
        off.destroy_device();
        assert!(off.render(&scene("cube_nocam.glb"), 64).is_err());
    }

    #[test]
    fn a_panic_during_render_becomes_an_error() {
        let r: Result<(), String> = guard_panics(|| panic!("poll blew up"));
        assert!(r.unwrap_err().contains("poll blew up"));
        assert_eq!(guard_panics(|| Ok(3)), Ok(3));
    }

    #[test]
    fn validation_errors_during_setup_are_captured() {
        let off = any_offscreen().unwrap();
        assert!(build_renderer(&off.device).is_ok());
        let bad_texture = || {
            off.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 0,
                    height: 0,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        assert!(capture_validation(&off.device, bad_texture).is_err());
    }

    fn any_offscreen() -> Result<Offscreen, String> {
        Offscreen::new(false).or_else(|_| Offscreen::new(true))
    }

    #[test]
    fn slot_replaces_a_lost_device() {
        let mut slot = SharedSlot::new(Duration::ZERO);
        let first = slot.get(any_offscreen).unwrap();
        assert!(Arc::ptr_eq(&first, &slot.get(any_offscreen).unwrap()));
        first.destroy_device();
        let second = slot.get(any_offscreen).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_red_center(&second.render(&scene("cube_nocam.glb"), 64).unwrap());
    }

    #[test]
    fn slot_retries_a_failed_build_after_the_cooldown() {
        let mut slot = SharedSlot::new(Duration::from_secs(3600));
        assert_eq!(slot.get(|| Err("no gpu".into())).err().unwrap(), "no gpu");
        // Within the cooldown the builder is not even called.
        let again = slot.get(|| panic!("must not rebuild yet"));
        assert_eq!(again.err().unwrap(), "no gpu");

        slot.cooldown = Duration::ZERO;
        assert!(slot.get(any_offscreen).is_ok());
    }
}
