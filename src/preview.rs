use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Condvar, Mutex};

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
    generation: u64,
    worker: Worker,
    rx: Receiver<Message>,
}

impl PreviewState {
    pub fn open(ctx: &Context, index: usize, path: PathBuf, size_hint: Option<Vec2>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let wake_ctx = ctx.clone();
        let worker = Worker::spawn(
            |path| formats::decode(path).map_err(|e| e.to_string()),
            tx,
            Box::new(move || wake_ctx.request_repaint()),
        );
        let mut state = Self {
            index,
            path: PathBuf::new(),
            image: None,
            transform: None,
            fitted: true,
            error: None,
            display_size: None,
            decoded: None,
            generation: 0,
            worker,
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
        self.generation += 1;
        let max_side = ctx.input(|i| i.max_texture_side).max(1);
        self.worker.submit(Request { generation: self.generation, path, max_side });
    }

    pub fn poll(&mut self, ctx: &Context) {
        let live = self.generation;
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

struct Request {
    generation: u64,
    path: PathBuf,
    max_side: usize,
}

struct Slot {
    request: Option<Request>,
    latest: u64,
    closed: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

/// One long-lived decode thread that only ever works on the newest request.
struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    fn spawn(
        decode: impl Fn(&Path) -> Result<RgbaImage, String> + Send + 'static,
        tx: Sender<Message>,
        notify: Box<dyn Fn() + Send>,
    ) -> Self {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot { request: None, latest: 0, closed: false }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::spawn(move || {
            loop {
                let request = {
                    let mut slot = worker.slot.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if slot.closed {
                            return;
                        }
                        if let Some(r) = slot.request.take() {
                            break r;
                        }
                        slot = worker.wake.wait(slot).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let is_stale = || {
                    let slot = worker.slot.lock().unwrap_or_else(|e| e.into_inner());
                    slot.closed || slot.latest != request.generation
                };
                if is_stale() {
                    continue;
                }
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&request.path)))
                    .unwrap_or_else(|_| Err("decoder crashed".to_string()));
                if is_stale() {
                    continue;
                }
                let result = decoded.map(|img| prepare(img, request.max_side));
                let _ = tx.send((request.generation, request.path, result));
                notify();
            }
        });
        Self { shared }
    }

    fn submit(&self, request: Request) {
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.latest = request.generation;
        slot.request = Some(request);
        self.shared.wake.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.slot.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.shared.wake.notify_one();
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
    fn worker_decodes_only_the_latest_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker = Worker::spawn(
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                Ok(RgbaImage::new(1, 1))
            },
            tx,
            Box::new(|| {}),
        );
        let req = |g: u64| Request { generation: g, path: PathBuf::from(format!("{g}.png")), max_side: 100 };
        worker.submit(req(1));
        std::thread::sleep(Duration::from_millis(50));
        worker.submit(req(2));
        worker.submit(req(3));
        let (generation, _, result) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(generation, 3);
        assert!(result.is_ok());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "superseded results must not be sent");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "request 2 must be skipped");
    }

    #[test]
    fn worker_reports_decoder_panic() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker = Worker::spawn(|_| panic!("boom"), tx, Box::new(|| {}));
        worker.submit(Request { generation: 1, path: PathBuf::from("a.png"), max_side: 10 });
        let (_, _, result) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn pan_moves_offset() {
        let mut t = ViewTransform::fit(vec2(100.0, 100.0), viewport());
        let before = t.offset;
        t.pan(vec2(5.0, -7.0));
        assert_eq!(t.offset - before, vec2(5.0, -7.0));
    }
}
