//! Orbitable 3D preview drawn on eframe's own wgpu device into an egui texture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use eframe::egui::{Align2, Color32, FontId, Rect, TextureId, Ui, Vec2, pos2};
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use image::RgbaImage;

use super::camera::OrbitCamera;
use super::gpu::{GpuScene, ModelRenderer, RenderTarget, try_read_back};
use super::offscreen::{guard_panics, unpremultiply_edges, with_error_scopes};
use super::{Aabb, Scene, UpAxis};

const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

static UP_OVERRIDES: LazyLock<Mutex<HashMap<PathBuf, UpAxis>>> = LazyLock::new(Default::default);

/// The vertical axis the user picked for `path` earlier in this session.
pub fn up_override(path: &Path) -> Option<UpAxis> {
    UP_OVERRIDES.lock().unwrap_or_else(|e| e.into_inner()).get(path).copied()
}

fn flipped(up: UpAxis) -> UpAxis {
    match up {
        UpAxis::Y => UpAxis::Z,
        UpAxis::Z => UpAxis::Y,
    }
}

/// GPU resources of a view; dropping them releases the egui texture.
struct GpuState {
    rs: RenderState,
    scene: GpuScene,
    target: RenderTarget,
    texture: TextureId,
}

impl GpuState {
    fn create(rs: &RenderState, renderer: &ModelRenderer, scene: &Scene, size: [u32; 2]) -> Self {
        let scene = GpuScene::upload(&rs.device, &rs.queue, renderer, scene);
        let target = RenderTarget::new(&rs.device, size);
        let texture =
            rs.renderer.write().register_native_texture(&rs.device, &target.gamma_view(), wgpu::FilterMode::Linear);
        Self { rs: rs.clone(), scene, target, texture }
    }

    fn resize(&mut self, size: [u32; 2]) {
        if self.target.size() == size {
            return;
        }
        self.target = RenderTarget::new(&self.rs.device, size);
        self.rs.renderer.write().update_egui_texture_from_wgpu_texture(
            &self.rs.device,
            &self.target.gamma_view(),
            wgpu::FilterMode::Linear,
            self.texture,
        );
    }
}

impl Drop for GpuState {
    fn drop(&mut self) {
        self.rs.renderer.write().free_texture(&self.texture);
    }
}

/// A loaded model with its orbit camera; the GPU side is built on the first `paint`.
pub struct ModelView {
    scene: Arc<Scene>,
    path: PathBuf,
    bounds: Aabb,
    home: OrbitCamera,
    camera: OrbitCamera,
    gpu: Option<GpuState>,
    /// Camera and pixel size of the last render attempt, successful or not.
    attempted: Option<(OrbitCamera, [u32; 2])>,
    error: Option<String>,
}

impl ModelView {
    /// `None` when the scene has no geometry. Applies the session up-axis override for `path`.
    pub fn new(scene: Arc<Scene>, path: &Path) -> Option<Self> {
        let bounds = scene.bounds()?;
        let mut home = OrbitCamera::initial(&scene)?;
        if let Some(up) = up_override(path) {
            home.set_up(up);
        }
        Some(Self {
            scene,
            path: path.to_path_buf(),
            bounds,
            home,
            camera: home,
            gpu: None,
            attempted: None,
            error: None,
        })
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.camera
    }

    /// The last GPU failure, shown instead of the model until a later render succeeds.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn reset(&mut self) {
        self.camera = self.home;
    }

    /// Flips Y-up and Z-up, for this file until the app exits.
    pub fn toggle_up(&mut self) {
        let up = flipped(self.camera.up);
        self.camera.set_up(up);
        self.home.set_up(up);
        UP_OVERRIDES.lock().unwrap_or_else(|e| e.into_inner()).insert(self.path.clone(), up);
    }

    pub fn orbit(&mut self, d: Vec2) {
        self.camera.orbit(glam::Vec2::new(d.x, d.y));
    }

    pub fn pan(&mut self, d: Vec2, viewport: Vec2) {
        self.camera.pan(glam::Vec2::new(d.x, d.y), glam::Vec2::new(viewport.x, viewport.y));
    }

    pub fn dolly(&mut self, notches: f32) {
        self.camera.dolly(notches);
    }

    /// Renders when the camera or pixel size changed since the last attempt, then draws over `rect`.
    pub fn paint(&mut self, ui: &mut Ui, rect: Rect, rs: &RenderState, renderer: &ModelRenderer) {
        let limit = rs.device.limits().max_texture_dimension_2d;
        let size = target_size(rect, ui.pixels_per_point(), limit);
        if self.attempted != Some((self.camera, size)) {
            self.attempted = Some((self.camera, size));
            self.error = self.render(rs, renderer, size).err();
            if let Some(e) = &self.error {
                log::warn!("model preview render failed: {e}");
            }
        }
        let painter = ui.painter_at(rect);
        match (&self.error, &self.gpu) {
            (Some(e), _) => {
                let text = format!("Rendu 3D impossible : {e}");
                let galley = painter.layout(text, FontId::proportional(14.0), Color32::GRAY, rect.width() * 0.8);
                painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::GRAY);
            }
            (None, Some(gpu)) => {
                painter.image(gpu.texture, rect, FULL_UV, Color32::WHITE);
            }
            (None, None) => {
                painter.text(rect.center(), Align2::CENTER_CENTER, "Chargement…", FontId::proportional(14.0), Color32::GRAY);
            }
        }
    }

    fn render(&mut self, rs: &RenderState, renderer: &ModelRenderer, size: [u32; 2]) -> Result<(), String> {
        log::debug!("model render {}x{}", size[0], size[1]);
        let mut slot = self.gpu.take();
        let (scene, bounds, camera) = (&self.scene, &self.bounds, &self.camera);
        let (result, errors) = with_error_scopes(&rs.device, || {
            guard_panics(|| {
                let gpu = slot.get_or_insert_with(|| GpuState::create(rs, renderer, scene, size));
                gpu.resize(size);
                renderer.render(&rs.device, &rs.queue, &gpu.target, &gpu.scene, camera, bounds);
                Ok(())
            })
        });
        if !errors.is_empty() {
            return Err(format!("GPU error: {}", errors.join("; ")));
        }
        result?;
        self.gpu = slot;
        Ok(())
    }

    /// The current view as a straight-alpha bitmap; `None` before the first render or after a failure.
    pub fn snapshot(&self) -> Option<RgbaImage> {
        let gpu = self.gpu.as_ref().filter(|_| self.error.is_none())?;
        let (device, queue) = (&gpu.rs.device, &gpu.rs.queue);
        let (result, errors) =
            with_error_scopes(device, || guard_panics(|| try_read_back(device, queue, gpu.target.texture())));
        let outcome = if errors.is_empty() { result } else { Err(errors.join("; ")) };
        match outcome {
            Ok(mut image) => {
                unpremultiply_edges(&mut image);
                Some(image)
            }
            Err(e) => {
                log::warn!("model snapshot failed: {e}");
                None
            }
        }
    }
}

fn target_size(rect: Rect, pixels_per_point: f32, limit: u32) -> [u32; 2] {
    let px = |points: f32| ((points * pixels_per_point).round() as u32).clamp(1, limit.max(1));
    [px(rect.width()), px(rect.height())]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use eframe::egui::vec2;

    use super::*;
    use crate::model::camera::OrbitCamera;
    use crate::model::{UpAxis, fixture, load};

    fn cube() -> Arc<crate::model::Scene> {
        Arc::new(load(&fixture("cube.glb")).unwrap())
    }

    fn flipped(up: UpAxis) -> UpAxis {
        match up {
            UpAxis::Y => UpAxis::Z,
            UpAxis::Z => UpAxis::Y,
        }
    }

    #[test]
    fn target_size_is_in_physical_pixels_within_limits() {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(300.4, 200.0));
        assert_eq!(target_size(rect, 1.5, 8192), [451, 300]);
        assert_eq!(target_size(rect, 1.5, 256), [256, 256]);
        assert_eq!(target_size(Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, 0.0)), 2.0, 8192), [1, 1]);
    }

    #[test]
    fn up_override_survives_a_new_view() {
        let (scene, path) = (cube(), PathBuf::from("view-test/up-override/cube.glb"));
        let mut first = ModelView::new(scene.clone(), &path).unwrap();
        let before = first.camera().up;
        assert_eq!(up_override(&path), None);
        first.toggle_up();
        assert_eq!(first.camera().up, flipped(before));

        let second = ModelView::new(scene.clone(), &path).unwrap();
        assert_eq!(second.camera().up, flipped(before));
        assert_eq!(up_override(&path), Some(flipped(before)));

        let other = ModelView::new(scene, &PathBuf::from("view-test/up-override/other.glb")).unwrap();
        assert_eq!(other.camera().up, before);
    }

    #[test]
    fn reset_restores_initial_camera() {
        let (scene, path) = (cube(), PathBuf::from("view-test/reset/cube.glb"));
        let initial = OrbitCamera::initial(&scene).unwrap();
        let mut view = ModelView::new(scene, &path).unwrap();
        assert_eq!(*view.camera(), initial);
        view.orbit(vec2(40.0, -15.0));
        view.pan(vec2(10.0, 5.0), vec2(800.0, 600.0));
        view.dolly(3.0);
        assert_ne!(*view.camera(), initial);
        view.reset();
        assert_eq!(*view.camera(), initial);
    }

    #[test]
    fn reset_keeps_the_toggled_axis() {
        let (scene, path) = (cube(), PathBuf::from("view-test/reset-axis/cube.glb"));
        let mut view = ModelView::new(scene, &path).unwrap();
        let before = view.camera().up;
        view.toggle_up();
        view.orbit(vec2(40.0, 0.0));
        view.reset();
        assert_eq!(view.camera().up, flipped(before));
    }
}
