use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, Context, CornerRadius, CursorIcon, FontId, Id, Margin, Modifiers, Rect,
    ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};

use super::status;
use crate::config::{self, Config};
use crate::layout::{self, BentoRule, Layout, LayoutItem, LayoutKind, LayoutSection};
use crate::library::{Entry, Library, Section, SortOrder};
use crate::selection::Selection;
use crate::thumbs::{
    DiskCache, ThumbJob, ThumbKey, ThumbOutcome, ThumbPipeline, ThumbResult, key_for,
};

const MIN_TEXTURES: usize = 400;
const TEXTURE_SLACK: usize = 64;
const UPLOADS_PER_FRAME: usize = 16;
const HIGHLIGHT: Duration = Duration::from_millis(1500);
const DIMS_RELAYOUT_EVERY: Duration = Duration::from_millis(150);
const PLACEHOLDER: Color32 = Color32::from_gray(44);
const FULL_UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

struct Slot {
    key: ThumbKey,
    handle: TextureHandle,
    used: u64,
}

/// LRU of GPU textures keyed by file path; a slot is only valid for the key it was made with.
struct TextureCache {
    map: HashMap<PathBuf, Slot>,
    tick: u64,
    cap: usize,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self { map: HashMap::new(), tick: 0, cap: MIN_TEXTURES }
    }
}

impl TextureCache {
    fn get(&mut self, path: &Path, key: ThumbKey) -> Option<&TextureHandle> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.map.get_mut(path).filter(|s| s.key == key)?;
        slot.used = tick;
        Some(&slot.handle)
    }

    fn contains(&self, path: &Path, key: ThumbKey) -> bool {
        self.map.get(path).is_some_and(|s| s.key == key)
    }

    /// The cap follows the near-zone tile count so every tile that is drawn or about to be fits.
    fn fit_cap(&mut self, near_tiles: usize) {
        self.cap = MIN_TEXTURES.max(near_tiles + TEXTURE_SLACK);
    }

    /// Returns the key of the texture evicted to stay under the cap, if any.
    fn insert(&mut self, path: PathBuf, key: ThumbKey, handle: TextureHandle) -> Option<ThumbKey> {
        self.tick += 1;
        self.map.insert(path, Slot { key, handle, used: self.tick });
        if self.map.len() <= self.cap {
            return None;
        }
        let oldest = self.map.iter().min_by_key(|(_, s)| s.used).map(|(p, _)| p.clone())?;
        self.map.remove(&oldest).map(|s| s.key)
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct LayoutSig {
    lib: u64,
    dims: u64,
    width: i32,
    size: i32,
    kind: LayoutKind,
    rule: BentoRule,
    sort: SortOrder,
    recursive: bool,
    pins: usize,
    collapsed: usize,
}

struct SectionMeta {
    label: String,
    count: usize,
    collapsed: bool,
    key: String,
}

struct TileItem {
    key: ThumbKey,
}

/// What the gallery reads from the app each frame.
pub struct View<'a> {
    pub library: &'a Library,
    pub config: &'a mut Config,
    pub root: &'a Path,
    pub selection: &'a Selection,
    /// Tile id to bring into view this frame.
    pub scroll_to: Option<usize>,
}

pub struct TileClick {
    pub id: usize,
    pub modifiers: Modifiers,
}

#[derive(Default)]
pub struct Output {
    pub hovered: Option<PathBuf>,
    pub clicked: Option<TileClick>,
    pub double_clicked: Option<usize>,
    pub right_clicked: Option<usize>,
    pub reveal: Option<usize>,
    /// A collapse toggle changed `config.folders`.
    pub config_changed: bool,
}

pub struct Gallery {
    pipeline: ThumbPipeline,
    textures: TextureCache,
    backlog: VecDeque<ThumbResult>,
    lib_version: u64,
    dims_version: u64,
    sections_key: Option<(u64, SortOrder, bool)>,
    sections: Vec<Section>,
    sig: Option<LayoutSig>,
    last_layout: Instant,
    layout: Layout,
    order: Vec<PathBuf>,
    items: Vec<TileItem>,
    meta: Vec<SectionMeta>,
    last_wanted: Vec<ThumbKey>,
    probe_dirty: bool,
    scroll_y: f32,
    viewport_h: f32,
    /// First visible tile and its y relative to the viewport top, from the previous frame.
    anchor: Option<(PathBuf, f32)>,
    reset_scroll: bool,
    /// Bumped on every rebuild of `order`.
    order_version: u64,
}

fn open_cache() -> DiskCache {
    let dir = DiskCache::default_dir();
    DiskCache::open(dir.clone()).unwrap_or_else(|e| {
        log::warn!("cannot open thumbnail cache {}: {e}", dir.display());
        DiskCache::open(std::env::temp_dir().join("image-moz").join("thumbs"))
            .expect("no writable thumbnail cache directory")
    })
}

fn collapse_key(rel_dir: &str) -> String {
    if rel_dir.is_empty() { ".".to_string() } else { rel_dir.to_lowercase() }
}

fn fit(rect: Rect, size: Vec2) -> Rect {
    let scale = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * scale)
}

/// Scroll offset that puts the anchor tile back at its previous on-screen y.
fn anchored_offset(new_tile_y: f32, screen_y: f32, content_h: f32, viewport_h: f32) -> f32 {
    (new_tile_y - screen_y).clamp(0.0, (content_h - viewport_h).max(0.0))
}

impl Gallery {
    pub fn new(ctx: &Context) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        let pipeline = ThumbPipeline::new(open_cache(), wake);
        Self {
            pipeline,
            textures: TextureCache::default(),
            backlog: VecDeque::new(),
            lib_version: 0,
            dims_version: 0,
            sections_key: None,
            sections: Vec::new(),
            sig: None,
            last_layout: Instant::now(),
            layout: Layout::default(),
            order: Vec::new(),
            items: Vec::new(),
            meta: Vec::new(),
            last_wanted: Vec::new(),
            probe_dirty: true,
            scroll_y: 0.0,
            viewport_h: 0.0,
            anchor: None,
            reset_scroll: false,
            order_version: 0,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Paths of the laid-out tiles in display order; a tile id is an index into it.
    pub fn order(&self) -> &[PathBuf] {
        &self.order
    }

    /// The cached thumbnail texture of tile `id`, if already uploaded.
    pub fn thumbnail(&mut self, id: usize) -> Option<TextureHandle> {
        let key = self.items.get(id)?.key;
        self.textures.get(self.order.get(id)?, key).cloned()
    }

    pub fn order_version(&self) -> u64 {
        self.order_version
    }

    /// Entries were added, removed or modified: sections and layout must be rebuilt.
    pub fn library_changed(&mut self) {
        self.lib_version += 1;
        self.probe_dirty = true;
    }

    /// A different folder: drop every texture and forget what the pipeline already produced.
    pub fn reset(&mut self) {
        self.pipeline.set_wanted(Vec::new());
        self.pipeline.set_probe(Vec::new());
        self.pipeline.forget_all();
        self.last_wanted.clear();
        self.textures.clear();
        self.backlog.clear();
        self.sections_key = None;
        self.sig = None;
        self.anchor = None;
        self.scroll_y = 0.0;
        self.reset_scroll = true;
        self.library_changed();
    }

    /// The entry is about to be modified on disk or re-read: let its thumb and dims be redone.
    pub fn forget_entry(&mut self, entry: &Entry) {
        self.pipeline.forget(key_for(&entry.path, entry.mtime, entry.size));
        self.last_wanted.clear();
    }

    /// Applies finished thumbnail jobs; texture uploads are spread over several frames.
    pub fn poll(&mut self, ctx: &Context, library: &mut Library) {
        self.backlog.extend(self.pipeline.try_iter());
        let mut uploads = 0;
        while let Some(front) = self.backlog.front() {
            let is_ready = matches!(front.outcome, ThumbOutcome::Ready(_));
            if is_ready && uploads >= UPLOADS_PER_FRAME {
                break;
            }
            let Some(result) = self.backlog.pop_front() else { break };
            let Some(entry) = library.get_mut(&result.path) else { continue };
            if key_for(&entry.path, entry.mtime, entry.size) != result.key {
                continue;
            }
            match result.outcome {
                ThumbOutcome::Ready(thumb) => {
                    uploads += 1;
                    entry.error = false;
                    if entry.dims != Some(thumb.orig_dims) {
                        entry.dims = Some(thumb.orig_dims);
                        self.dims_version += 1;
                    }
                    let size = [thumb.image.width() as usize, thumb.image.height() as usize];
                    let image = ColorImage::from_rgba_unmultiplied(size, thumb.image.as_raw());
                    let handle = ctx.load_texture("thumb", image, TextureOptions::LINEAR);
                    if let Some(old) = self.textures.insert(result.path, result.key, handle) {
                        self.pipeline.forget(old);
                    }
                }
                ThumbOutcome::Dims(dims) => {
                    if entry.dims != Some(dims) {
                        entry.dims = Some(dims);
                        self.dims_version += 1;
                    }
                }
                ThumbOutcome::Failed => entry.error = true,
            }
        }
        if !self.backlog.is_empty() {
            ctx.request_repaint();
        }
    }

    fn signature(&self, view: &View, width: f32) -> LayoutSig {
        let cfg = &*view.config;
        let state = cfg.folder(view.root);
        LayoutSig {
            lib: self.lib_version,
            dims: if cfg.layout == LayoutKind::Grid { 0 } else { self.dims_version },
            width: width as i32,
            size: cfg.target_size.round() as i32,
            kind: cfg.layout,
            rule: cfg.bento_rule,
            sort: cfg.sort,
            recursive: cfg.recursive,
            pins: state.map_or(0, |s| s.pins.len()),
            collapsed: state.map_or(0, |s| s.collapsed.len()),
        }
    }

    fn rebuild(&mut self, view: &View, sig: &LayoutSig) {
        let cfg = &*view.config;
        let sections_key = (sig.lib, sig.sort, sig.recursive);
        if self.sections_key != Some(sections_key) {
            self.sections = view.library.sections(cfg.sort);
            self.sections_key = Some(sections_key);
        }
        let state = cfg.folder(view.root);
        let pins = state.map(|s| &s.pins).filter(|p| !p.is_empty());
        let collapsed = state.map(|s| &s.collapsed);
        self.items.clear();
        self.order.clear();
        self.meta.clear();
        let mut layout_sections = Vec::with_capacity(self.sections.len());
        for section in &self.sections {
            let key = collapse_key(&section.rel_dir);
            let is_collapsed = cfg.recursive && collapsed.is_some_and(|c| c.contains(&key));
            let mut items = Vec::new();
            if !is_collapsed {
                for path in &section.items {
                    let Some(entry) = view.library.get(path) else { continue };
                    let (aspect, pixels) = match entry.dims {
                        Some((w, h)) if h > 0 => (w as f32 / h as f32, u64::from(w) * u64::from(h)),
                        _ => (1.0, 0),
                    };
                    let pinned = pins.is_some_and(|p| {
                        config::rel_key(view.root, path).is_some_and(|rel| p.contains(&rel))
                    });
                    items.push(LayoutItem {
                        id: self.items.len(),
                        aspect,
                        mtime: entry.mtime,
                        pixels,
                        pinned,
                    });
                    self.items.push(TileItem { key: key_for(path, entry.mtime, entry.size) });
                    self.order.push(path.clone());
                }
            }
            let label = if section.rel_dir.is_empty() { ".".to_string() } else { section.rel_dir.clone() };
            layout_sections.push(LayoutSection {
                header: cfg.recursive.then(|| label.clone()),
                collapsed: is_collapsed,
                items,
            });
            self.meta.push(SectionMeta {
                label,
                count: section.items.len(),
                collapsed: is_collapsed,
                key,
            });
        }
        self.layout = layout::compute(
            sig.kind,
            sig.rule,
            &layout_sections,
            sig.width as f32,
            sig.size as f32,
        );
        self.last_layout = Instant::now();
        self.order_version += 1;
    }

    fn update_probes(&mut self, library: &Library) {
        let mut jobs = Vec::new();
        for path in self.sections.iter().flat_map(|s| &s.items) {
            if let Some(e) = library.get(path)
                && e.dims.is_none()
                && !e.error
            {
                jobs.push(ThumbJob { path: path.clone(), key: key_for(path, e.mtime, e.size) });
            }
        }
        self.pipeline.set_probe(jobs);
        self.probe_dirty = false;
    }

    pub fn show(&mut self, ui: &mut Ui, view: View) -> Output {
        let width = ui.available_width().floor().max(1.0);
        let sig = self.signature(&view, width);
        let mut forced_offset = self.reset_scroll.then_some(0.0);
        self.reset_scroll = false;

        let mut relayout = self.sig != Some(sig);
        if relayout
            && let Some(old) = self.sig
            && (LayoutSig { dims: sig.dims, ..old }) == sig
            && self.last_layout.elapsed() < DIMS_RELAYOUT_EVERY
        {
            ui.ctx().request_repaint_after(DIMS_RELAYOUT_EVERY);
            relayout = false;
        }
        if relayout {
            self.rebuild(&view, &sig);
            self.sig = Some(sig);
            if forced_offset.is_none() && self.scroll_y > 0.5 {
                forced_offset = self.anchored_offset_after_relayout();
            }
        }
        if self.probe_dirty {
            self.update_probes(view.library);
        }

        let mut area = ScrollArea::vertical()
            .id_salt("gallery_scroll")
            .auto_shrink([false, false])
            .content_margin(Margin::ZERO);
        if let Some(offset) = forced_offset {
            area = area.vertical_scroll_offset(offset);
        }
        let mut out = Output::default();
        let mut toggled: Option<String> = None;
        let scroll = area.show_viewport(ui, |ui, viewport| {
            if let Some(tile) = view.scroll_to.and_then(|id| self.layout.tiles.iter().find(|t| t.id == id)) {
                let origin = ui.max_rect().min.to_vec2();
                ui.scroll_to_rect(tile.rect.translate(origin), None);
            }
            self.draw(ui, viewport, &view, width, &mut out, &mut toggled);
        });
        self.scroll_y = scroll.state.offset.y;
        self.viewport_h = scroll.inner_rect.height();

        if let Some(key) = toggled {
            let collapsed = &mut view.config.folder_mut(view.root).collapsed;
            if !collapsed.remove(&key) {
                collapsed.insert(key);
            }
            out.config_changed = true;
        }
        out
    }

    fn anchored_offset_after_relayout(&self) -> Option<f32> {
        let (path, screen_y) = self.anchor.as_ref()?;
        let id = self.order.iter().position(|p| p == path)?;
        let tile = self.layout.tiles.iter().find(|t| t.id == id)?;
        Some(anchored_offset(tile.rect.min.y, *screen_y, self.layout.height, self.viewport_h))
    }

    fn draw(
        &mut self,
        ui: &mut Ui,
        viewport: Rect,
        view: &View,
        width: f32,
        out: &mut Output,
        toggled: &mut Option<String>,
    ) {
        let (content, _) =
            ui.allocate_exact_size(vec2(width, self.layout.height.max(1.0)), Sense::hover());
        let origin = content.min.to_vec2();
        let near = viewport.expand2(vec2(0.0, viewport.height()));
        let recursive = view.config.recursive;
        let now = Instant::now();
        let mut fading = false;
        let mut first: Option<(f32, f32, usize)> = None;
        let mut near_tiles = 0;
        let mut near_jobs: Vec<ThumbJob> = Vec::new();
        let mut far_jobs: Vec<(f32, ThumbJob)> = Vec::new();

        for header in &self.layout.headers {
            if !header.rect.intersects(viewport) {
                continue;
            }
            let rect = header.rect.translate(origin);
            let meta = &self.meta[header.section];
            let response = ui
                .interact(rect, Id::new(("section_header", header.section)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            let visuals = ui.visuals();
            let fill = if response.hovered() {
                visuals.widgets.hovered.weak_bg_fill
            } else {
                visuals.widgets.inactive.weak_bg_fill
            };
            let text_color = visuals.strong_text_color();
            let weak = visuals.weak_text_color();
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(3), fill);
            let arrow = if meta.collapsed { "▶" } else { "▼" };
            let title = painter.text(
                rect.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{arrow}  {}", meta.label),
                FontId::proportional(14.0),
                text_color,
            );
            painter.text(
                pos2(title.right() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                status::images_text(meta.count),
                FontId::proportional(12.0),
                weak,
            );
            if response.clicked() {
                *toggled = Some(meta.key.clone());
            }
        }

        for tile in &self.layout.tiles {
            let rect = tile.rect.translate(origin);
            let rel = tile.rect;
            if !rel.intersects(near) {
                continue;
            }
            near_tiles += 1;
            let id = tile.id;
            let path = &self.order[id];
            let key = self.items[id].key;
            let entry = view.library.get(path);
            let in_view = rel.intersects(viewport);
            let loaded = self.textures.contains(path, key);
            if !loaded && !entry.is_some_and(|e| e.error) {
                let job = ThumbJob { path: path.clone(), key };
                if in_view {
                    near_jobs.push(job);
                } else {
                    let dist = (rel.center().y - viewport.center().y).abs();
                    far_jobs.push((dist, job));
                }
            }
            if !in_view {
                if loaded {
                    self.textures.get(path, key);
                }
                continue;
            }
            if first.is_none_or(|(y, x, _)| (rel.min.y, rel.min.x) < (y, x)) {
                first = Some((rel.min.y, rel.min.x, id));
            }

            let response = ui.interact(rect, Id::new(("tile", id)), Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(2), PLACEHOLDER);
            if let Some(texture) = self.textures.get(path, key) {
                painter.image(texture.id(), fit(rect, texture.size_vec2()), FULL_UV, Color32::WHITE);
            } else if entry.is_some_and(|e| e.error) {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "⚠",
                    FontId::proportional(28.0),
                    Color32::from_rgb(230, 160, 40),
                );
            }
            let accent = Color32::from_rgb(70, 140, 255);
            if view.selection.contains(id) {
                painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(2.0, accent), StrokeKind::Inside);
            }
            if view.selection.focus() == Some(id) {
                painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(4.0, accent), StrokeKind::Inside);
            }
            if let Some(arrived) = entry.and_then(|e| e.arrived_at) {
                let age = now.saturating_duration_since(arrived);
                if age < HIGHLIGHT {
                    let fade = 1.0 - age.as_secs_f32() / HIGHLIGHT.as_secs_f32();
                    let alpha = (fade * 255.0) as u8;
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(2),
                        Stroke::new(4.0, Color32::from_rgba_unmultiplied(255, 200, 40, alpha)),
                        StrokeKind::Inside,
                    );
                    fading = true;
                }
            }
            if response.hovered() {
                out.hovered = Some(path.clone());
            }
            if response.double_clicked() {
                out.double_clicked = Some(id);
            } else if response.clicked() {
                out.clicked = Some(TileClick { id, modifiers: ui.input(|i| i.modifiers) });
            }
            if response.secondary_clicked() {
                out.right_clicked = Some(id);
            }
            response.context_menu(|ui| {
                if ui.button("Afficher dans l'Explorateur").clicked() {
                    out.reveal = Some(id);
                    ui.close();
                }
            });
            if let Some(entry) = entry {
                response.on_hover_ui(|ui| {
                    ui.label(egui::RichText::new(&entry.name).strong());
                    ui.label(status::meta_text(entry));
                    if recursive {
                        ui.label(status::rel_dir_label(entry));
                    }
                });
            }
        }

        self.anchor = first.map(|(y, _, id)| (self.order[id].clone(), y - viewport.min.y));
        if fading {
            ui.ctx().request_repaint();
        }
        far_jobs.sort_by(|a, b| a.0.total_cmp(&b.0));
        near_jobs.extend(far_jobs.into_iter().map(|(_, job)| job));
        self.textures.fit_cap(near_tiles);
        self.request_thumbs(near_jobs);
    }

    fn request_thumbs(&mut self, jobs: Vec<ThumbJob>) {
        let keys: Vec<ThumbKey> = jobs.iter().map(|j| j.key).collect();
        if keys != self.last_wanted {
            self.last_wanted = keys;
            self.pipeline.set_wanted(jobs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_contains_without_cropping() {
        let tile = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 100.0));
        let wide = fit(tile, vec2(200.0, 100.0));
        assert_eq!(wide.size(), vec2(100.0, 50.0));
        assert_eq!(wide.center(), tile.center());
        let tall = fit(tile, vec2(50.0, 200.0));
        assert_eq!(tall.size(), vec2(25.0, 100.0));
    }

    #[test]
    fn anchor_keeps_tile_on_screen_position() {
        assert_eq!(anchored_offset(500.0, 20.0, 5000.0, 600.0), 480.0);
        assert_eq!(anchored_offset(500.0, -30.0, 5000.0, 600.0), 530.0);
        assert_eq!(anchored_offset(10.0, 50.0, 5000.0, 600.0), 0.0);
        assert_eq!(anchored_offset(9000.0, 0.0, 5000.0, 600.0), 4400.0);
        assert_eq!(anchored_offset(100.0, 0.0, 300.0, 600.0), 0.0);
    }

    #[test]
    fn texture_cache_evicts_least_recently_used() {
        let ctx = Context::default();
        let mut cache = TextureCache::default();
        let make = |i: usize| {
            ctx.load_texture(format!("t{i}"), ColorImage::example(), TextureOptions::LINEAR)
        };
        for i in 0..MIN_TEXTURES {
            assert_eq!(cache.insert(PathBuf::from(format!("{i}")), ThumbKey(i as u64), make(i)), None);
        }
        assert!(cache.get(Path::new("0"), ThumbKey(0)).is_some());
        let evicted = cache.insert(PathBuf::from("new"), ThumbKey(999), make(999));
        assert_eq!(evicted, Some(ThumbKey(1)));
        assert!(cache.contains(Path::new("0"), ThumbKey(0)));
        assert!(!cache.contains(Path::new("1"), ThumbKey(1)));
    }

    #[test]
    fn cap_follows_near_zone_tile_count() {
        let mut cache = TextureCache::default();
        cache.fit_cap(10);
        assert_eq!(cache.cap, MIN_TEXTURES);
        cache.fit_cap(900);
        assert_eq!(cache.cap, 900 + TEXTURE_SLACK);
    }

    #[test]
    fn texture_is_only_valid_for_its_key() {
        let ctx = Context::default();
        let mut cache = TextureCache::default();
        let handle = ctx.load_texture("t", ColorImage::example(), TextureOptions::LINEAR);
        cache.insert(PathBuf::from("a"), ThumbKey(1), handle);
        assert!(cache.get(Path::new("a"), ThumbKey(2)).is_none());
        assert!(!cache.contains(Path::new("a"), ThumbKey(2)));
    }

    #[test]
    fn collapse_key_names_root_dot() {
        assert_eq!(collapse_key(""), ".");
        assert_eq!(collapse_key("Sub/Deep"), "sub/deep");
    }
}
