use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use image::imageops::FilterType;

use crate::formats;

const MIN_SCALE: f32 = 0.02;
const MAX_SCALE: f32 = 64.0;

/// Maps image pixels to viewport coordinates: `viewport = offset + pixel * scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub scale: f32,
    /// Image origin in viewport coordinates.
    pub offset: Vec2,
}

impl ViewTransform {
    /// Contain-fit, centered; images smaller than the viewport are upscaled too.
    pub fn fit(image: Vec2, viewport: Rect) -> Self {
        let scale = if image.x > 0.0 && image.y > 0.0 {
            (viewport.width() / image.x).min(viewport.height() / image.y).clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        };
        let offset = viewport.min.to_vec2() + (viewport.size() - image * scale) / 2.0;
        Self { scale, offset }
    }

    /// Keeps the image point under `cursor` fixed while scaling by `factor`.
    pub fn zoom_at(&mut self, factor: f32, cursor: Pos2) {
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let ratio = scale / self.scale;
        self.offset = cursor.to_vec2() - (cursor.to_vec2() - self.offset) * ratio;
        self.scale = scale;
    }

    pub fn pan(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    pub fn image_rect(&self, image: Vec2) -> Rect {
        Rect::from_min_size(self.offset.to_pos2(), image * self.scale)
    }
}

struct Loaded {
    full: Arc<RgbaImage>,
    texture: ColorImage,
}

type Message = (u64, PathBuf, Result<Loaded, String>);

/// Full-frame preview of one image of the gallery order.
pub struct PreviewState {
    /// Position in the gallery's visible order.
    pub index: usize,
    /// Path being previewed; also the decode that is awaited until `image` is set.
    pub path: PathBuf,
    pub image: Option<TextureHandle>,
    pub transform: Option<ViewTransform>,
    /// Refit to the viewport every frame until the user zooms or pans.
    pub fitted: bool,
    pub error: Option<String>,
    /// Size the transform is computed against: the original dims once known.
    pub display_size: Option<Vec2>,
    decoded: Option<Arc<RgbaImage>>,
    generation: Arc<AtomicU64>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl PreviewState {
    pub fn open(ctx: &Context, index: usize, path: PathBuf, size_hint: Option<Vec2>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = Self {
            index,
            path: PathBuf::new(),
            image: None,
            transform: None,
            fitted: true,
            error: None,
            display_size: None,
            decoded: None,
            generation: Arc::new(AtomicU64::new(0)),
            tx,
            rx,
        };
        state.show(ctx, index, path, size_hint);
        state
    }

    /// Switches to another image; results of earlier decodes are dropped.
    pub fn show(&mut self, ctx: &Context, index: usize, path: PathBuf, size_hint: Option<Vec2>) {
        self.index = index;
        self.path = path.clone();
        self.image = None;
        self.decoded = None;
        self.error = None;
        self.transform = None;
        self.fitted = true;
        self.display_size = size_hint;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (live, tx, wake) = (self.generation.clone(), self.tx.clone(), ctx.clone());
        let max_side = ctx.input(|i| i.max_texture_side).max(1);
        std::thread::spawn(move || {
            if live.load(Ordering::SeqCst) != generation {
                return;
            }
            let result = formats::decode(&path).map(|img| prepare(img, max_side)).map_err(|e| e.to_string());
            let _ = tx.send((generation, path, result));
            wake.request_repaint();
        });
    }

    pub fn poll(&mut self, ctx: &Context) {
        let live = self.generation.load(Ordering::SeqCst);
        while let Ok((generation, path, result)) = self.rx.try_recv() {
            if generation != live || path != self.path {
                continue;
            }
            match result {
                Ok(loaded) => {
                    let size = Vec2::new(loaded.full.width() as f32, loaded.full.height() as f32);
                    if self.display_size != Some(size) {
                        self.fitted = true;
                    }
                    self.display_size = Some(size);
                    self.image = Some(ctx.load_texture("preview", loaded.texture, TextureOptions::LINEAR));
                    self.decoded = Some(loaded.full);
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// The full original decode of `path`, if it is the one previewed and already loaded.
    pub fn decoded_for(&self, path: &Path) -> Option<Arc<RgbaImage>> {
        (self.path == path).then(|| self.decoded.clone()).flatten()
    }
}

/// Texture-sized copy of `full` (downscaled when a side exceeds `max_side`).
fn prepare(full: RgbaImage, max_side: usize) -> Loaded {
    let (w, h) = (full.width() as usize, full.height() as usize);
    let texture = if w.max(h) > max_side {
        let k = max_side as f32 / w.max(h) as f32;
        let (nw, nh) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let small = image::imageops::resize(&full, nw, nh, FilterType::Triangle);
        ColorImage::from_rgba_unmultiplied([nw as usize, nh as usize], small.as_raw())
    } else {
        ColorImage::from_rgba_unmultiplied([w, h], full.as_raw())
    };
    Loaded { full: Arc::new(full), texture }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2, Rect};

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0))
    }

    #[test]
    fn fit_centers_and_contains() {
        let t = ViewTransform::fit(vec2(400.0, 100.0), viewport());
        let r = t.image_rect(vec2(400.0, 100.0));
        assert!((t.scale - 2.0).abs() < 1e-4, "small images are upscaled: {}", t.scale);
        assert!(viewport().contains_rect(r.expand(-0.01)));
        assert!((r.center() - viewport().center()).length() < 1e-3);
        let tall = ViewTransform::fit(vec2(3000.0, 6000.0), viewport());
        assert!((tall.scale - 0.1).abs() < 1e-4);
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let image = vec2(1000.0, 500.0);
        let mut t = ViewTransform::fit(image, viewport());
        let cursor = pos2(300.0, 250.0);
        let before = (cursor - t.image_rect(image).min) / t.scale;
        t.zoom_at(1.1, cursor);
        t.zoom_at(1.1, cursor);
        let after = (cursor - t.image_rect(image).min) / t.scale;
        assert!((before - after).length() < 1e-3, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_clamped() {
        let mut t = ViewTransform::fit(vec2(100.0, 100.0), viewport());
        t.zoom_at(1e9, pos2(0.0, 0.0));
        assert_eq!(t.scale, 64.0);
        t.zoom_at(1e-9, pos2(0.0, 0.0));
        assert_eq!(t.scale, 0.02);
    }

    #[test]
    fn pan_moves_offset() {
        let mut t = ViewTransform::fit(vec2(100.0, 100.0), viewport());
        let before = t.offset;
        t.pan(vec2(5.0, -7.0));
        assert_eq!(t.offset - before, vec2(5.0, -7.0));
    }
}
