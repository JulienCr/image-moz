use std::collections::{HashMap, HashSet, VecDeque};
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
use crate::library::{Entry, FsChange, Library, Section, SortOrder};
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
    handle: TextureHandle,
    used: u64,
}

/// LRU of GPU textures keyed by `ThumbKey`, which is case-insensitive on the path like the library.
struct TextureCache {
    map: HashMap<ThumbKey, Slot>,
    tick: u64,
    cap: usize,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self { map: HashMap::new(), tick: 0, cap: MIN_TEXTURES }
    }
}

impl TextureCache {
    fn get(&mut self, key: ThumbKey) -> Option<&TextureHandle> {
        self.tick += 1;
        let slot = self.map.get_mut(&key)?;
        slot.used = self.tick;
        Some(&slot.handle)
    }

    fn contains(&self, key: ThumbKey) -> bool {
        self.map.contains_key(&key)
    }

    fn remove(&mut self, key: ThumbKey) {
        self.map.remove(&key);
    }

    /// The cap follows the near-zone tile count so every tile that is drawn or about to be fits.
    fn fit_cap(&mut self, near_tiles: usize) {
        self.cap = MIN_TEXTURES.max(near_tiles + TEXTURE_SLACK);
    }

    /// Returns the key of the texture evicted to stay under the cap, if any.
    fn insert(&mut self, key: ThumbKey, handle: TextureHandle) -> Option<ThumbKey> {
        self.tick += 1;
        self.map.insert(key, Slot { handle, used: self.tick });
        if self.map.len() <= self.cap {
            return None;
        }
        let oldest = self.map.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| *k)?;
        self.map.remove(&oldest).map(|_| oldest)
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
    pub thumbs: &'a mut Thumbs,
    pub config: &'a mut Config,
    pub sort: SortOrder,
    pub recursive: bool,
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

/// Decode pool, disk cache and GPU textures, shared by every tab's `Gallery`.
pub struct Thumbs {
    pipeline: ThumbPipeline,
    textures: TextureCache,
    backlog: VecDeque<ThumbResult>,
}

pub struct Gallery {
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
    /// Keys of the last request sent to the pipeline; `None` forces the next one through.
    last_wanted: Option<Vec<ThumbKey>>,
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

impl Thumbs {
    pub fn new(ctx: &Context) -> Self {
        Self::with_cache(ctx, open_cache())
    }

    pub fn with_cache(ctx: &Context, cache: DiskCache) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        Self {
            pipeline: ThumbPipeline::new(cache, wake),
            textures: TextureCache::default(),
            backlog: VecDeque::new(),
        }
    }

    /// Drops every queued request, e.g. when no tab is left to want them.
    pub fn cancel_requests(&self) {
        self.pipeline.set_wanted(Vec::new());
        self.pipeline.set_probe(Vec::new());
    }

    /// Applies finished jobs to every tab that lists the file; texture uploads are spread over frames.
    pub fn poll(&mut self, ctx: &Context, tabs: &mut [(&mut Library, &mut Gallery)]) {
        self.backlog.extend(self.pipeline.try_iter());
        let mut uploads = 0;
        while let Some(front) = self.backlog.front() {
            let is_ready = matches!(front.outcome, ThumbOutcome::Ready(_));
            if is_ready && uploads >= UPLOADS_PER_FRAME {
                break;
            }
            let Some(result) = self.backlog.pop_front() else { break };
            let mut claimed = false;
            for (library, gallery) in tabs.iter_mut() {
                let current = library.get_mut(&result.path);
                let Some(entry) = current.filter(|e| key_for(&e.path, e.mtime, e.size) == result.key) else {
                    continue;
                };
                claimed = true;
                let dims = match &result.outcome {
                    ThumbOutcome::Ready(thumb) => {
                        entry.error = false;
                        Some(thumb.orig_dims)
                    }
                    ThumbOutcome::Dims(dims) => Some(*dims),
                    ThumbOutcome::Failed => {
                        entry.error = true;
                        None
                    }
                };
                if let Some(dims) = dims.filter(|d| entry.dims != Some(*d)) {
                    entry.dims = Some(dims);
                    gallery.dims_version += 1;
                }
            }
            if !claimed {
                self.pipeline.forget(result.key);
                continue;
            }
            if let ThumbOutcome::Ready(thumb) = result.outcome {
                uploads += 1;
                let size = [thumb.image.width() as usize, thumb.image.height() as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, thumb.image.as_raw());
                let handle = ctx.load_texture("thumb", image, TextureOptions::LINEAR);
                if let Some(old) = self.textures.insert(result.key, handle) {
                    self.pipeline.forget(old);
                }
            }
        }
        if !self.backlog.is_empty() {
            ctx.request_repaint();
        }
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new()
    }
}

impl Gallery {
    pub fn new() -> Self {
        Self {
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
            last_wanted: None,
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
    pub fn thumbnail(&self, thumbs: &mut Thumbs, id: usize) -> Option<TextureHandle> {
        let key = self.items.get(id)?.key;
        thumbs.textures.get(key).cloned()
    }

    /// The tab became the visible one: its thumbnail requests must replace the previous tab's.
    pub fn activate(&mut self) {
        self.last_wanted = None;
        self.probe_dirty = true;
    }

    pub fn order_version(&self) -> u64 {
        self.order_version
    }

    /// Entries were added, removed or modified: sections and layout must be rebuilt.
    pub fn library_changed(&mut self) {
        self.lib_version += 1;
        self.probe_dirty = true;
    }

    /// The folder is re-read: rebuild from scratch. Textures stay, they are keyed by file state.
    pub fn reset(&mut self, thumbs: &mut Thumbs) {
        thumbs.pipeline.set_wanted(Vec::new());
        thumbs.pipeline.set_probe(Vec::new());
        thumbs.pipeline.forget_all();
        self.last_wanted = None;
        self.sections_key = None;
        self.sig = None;
        self.anchor = None;
        self.scroll_y = 0.0;
        self.reset_scroll = true;
        self.library_changed();
    }

    /// The entry is about to be modified, removed or re-read: let its thumb and dims be redone.
    pub fn forget_entry(&mut self, thumbs: &mut Thumbs, entry: &Entry) {
        let key = key_for(&entry.path, entry.mtime, entry.size);
        thumbs.pipeline.forget(key);
        thumbs.textures.remove(key);
        self.last_wanted = None;
    }

    /// Applies a watcher change, releasing the pipeline state of the entry it replaces or drops.
    pub fn apply_change(&mut self, thumbs: &mut Thumbs, library: &mut Library, change: &FsChange, now: Instant) {
        if let FsChange::Upsert(path) | FsChange::Remove(path) = change
            && let Some(old) = library.get(path)
        {
            self.forget_entry(thumbs, old);
        }
        library.apply(change, now);
        self.library_changed();
    }

    /// Swaps in a rescan result; entries it no longer contains (or contains changed) are forgotten.
    pub fn replace_entries(&mut self, thumbs: &mut Thumbs, library: &mut Library, entries: Vec<Entry>) {
        let kept: HashSet<ThumbKey> = entries.iter().map(|e| key_for(&e.path, e.mtime, e.size)).collect();
        for old in library.iter() {
            if !kept.contains(&key_for(&old.path, old.mtime, old.size)) {
                self.forget_entry(thumbs, old);
            }
        }
        library.replace_all(entries);
        self.library_changed();
    }

    /// User refresh: failed tiles get another attempt.
    pub fn retry_failed(&mut self, thumbs: &mut Thumbs, library: &mut Library) {
        library.clear_errors();
        thumbs.pipeline.forget_all();
        self.last_wanted = None;
        self.library_changed();
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
            sort: view.sort,
            recursive: view.recursive,
            pins: state.map_or(0, |s| s.pins.len()),
            collapsed: state.map_or(0, |s| s.collapsed.len()),
        }
    }

    fn rebuild(&mut self, view: &View, sig: &LayoutSig) {
        let cfg = &*view.config;
        let sections_key = (sig.lib, sig.sort, sig.recursive);
        if self.sections_key != Some(sections_key) {
            self.sections = view.library.sections(view.sort);
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
            let is_collapsed = view.recursive && collapsed.is_some_and(|c| c.contains(&key));
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
                header: view.recursive.then(|| label.clone()),
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

    fn update_probes(&mut self, thumbs: &Thumbs, library: &Library) {
        let mut jobs = Vec::new();
        for path in self.sections.iter().flat_map(|s| &s.items) {
            if let Some(e) = library.get(path)
                && e.dims.is_none()
                && !e.error
            {
                jobs.push(ThumbJob { path: path.clone(), key: key_for(path, e.mtime, e.size) });
            }
        }
        thumbs.pipeline.set_probe(jobs);
        self.probe_dirty = false;
    }

    pub fn show(&mut self, ui: &mut Ui, mut view: View) -> Output {
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
            self.update_probes(view.thumbs, view.library);
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
            self.draw(ui, viewport, &mut view, width, &mut out, &mut toggled);
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
        view: &mut View,
        width: f32,
        out: &mut Output,
        toggled: &mut Option<String>,
    ) {
        let (content, _) =
            ui.allocate_exact_size(vec2(width, self.layout.height.max(1.0)), Sense::hover());
        let origin = content.min.to_vec2();
        let near = viewport.expand2(vec2(0.0, viewport.height()));
        let recursive = view.recursive;
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
            let loaded = view.thumbs.textures.contains(key);
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
                    view.thumbs.textures.get(key);
                }
                continue;
            }
            if first.is_none_or(|(y, x, _)| (rel.min.y, rel.min.x) < (y, x)) {
                first = Some((rel.min.y, rel.min.x, id));
            }

            let response = ui.interact(rect, Id::new(("tile", id)), Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(2), PLACEHOLDER);
            if let Some(texture) = view.thumbs.textures.get(key) {
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
        view.thumbs.textures.fit_cap(near_tiles);
        self.request_thumbs(view.thumbs, near_jobs);
    }

    fn request_thumbs(&mut self, thumbs: &Thumbs, jobs: Vec<ThumbJob>) {
        let keys: Vec<ThumbKey> = jobs.iter().map(|j| j.key).collect();
        if self.last_wanted.as_ref() != Some(&keys) {
            self.last_wanted = Some(keys);
            thumbs.pipeline.set_wanted(jobs);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

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
            assert_eq!(cache.insert(ThumbKey(i as u64), make(i)), None);
        }
        assert!(cache.get(ThumbKey(0)).is_some());
        let evicted = cache.insert(ThumbKey(999), make(999));
        assert_eq!(evicted, Some(ThumbKey(1)));
        assert!(cache.contains(ThumbKey(0)));
        assert!(!cache.contains(ThumbKey(1)));
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
        cache.insert(ThumbKey(1), handle);
        assert!(cache.get(ThumbKey(2)).is_none());
        assert!(!cache.contains(ThumbKey(2)));
    }

    #[test]
    fn texture_survives_a_case_only_rename() {
        let ctx = Context::default();
        let mut cache = TextureCache::default();
        let mtime = SystemTime::now();
        let handle = ctx.load_texture("t", ColorImage::example(), TextureOptions::LINEAR);
        cache.insert(key_for(Path::new("C:/pics/a.png"), mtime, 10), handle);
        assert!(cache.contains(key_for(Path::new("C:/pics/A.PNG"), mtime, 10)));
    }

    fn test_thumbs(dir: &Path) -> (Context, Thumbs, Gallery) {
        let ctx = Context::default();
        let cache = DiskCache::open(dir.join("cache")).unwrap();
        (ctx.clone(), Thumbs::with_cache(&ctx, cache), Gallery::new())
    }

    fn library_of(dir: &Path) -> Library {
        let mut library = Library::new(dir.to_path_buf(), false);
        library.replace_all(crate::library::scan(dir, false).unwrap());
        library
    }

    fn probe(thumbs: &Thumbs, gallery: &mut Gallery, library: &Library) {
        gallery.sections = library.sections(SortOrder::default());
        gallery.library_changed();
        gallery.update_probes(thumbs, library);
    }

    /// Polls until `done` holds for the entry at `path`, or 5 s elapse.
    fn settles(
        ctx: &Context,
        thumbs: &mut Thumbs,
        gallery: &mut Gallery,
        library: &mut Library,
        path: &Path,
        done: impl Fn(&Entry) -> bool,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            thumbs.poll(ctx, &mut [(&mut *library, &mut *gallery)]);
            if library.get(path).is_some_and(&done) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn has_dims(entry: &Entry) -> bool {
        entry.dims == Some((40, 20))
    }

    fn write_image(path: &Path) {
        image::RgbaImage::from_pixel(40, 20, image::Rgba([1, 2, 3, 255])).save(path).unwrap();
    }

    #[test]
    fn removed_then_restored_file_is_probed_again() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));

        let now = Instant::now();
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Remove(img.clone()), now);
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Upsert(img.clone()), now);
        assert_eq!(library.get(&img).unwrap().dims, None);
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn file_missing_from_a_rescan_is_probed_again_when_it_returns() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));

        gallery.replace_entries(&mut thumbs, &mut library, Vec::new());
        gallery.replace_entries(&mut thumbs, &mut library, crate::library::scan(dir.path(), false).unwrap());
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn discarded_result_does_not_pin_its_key_as_done() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&thumbs, &mut gallery, &library);
        while thumbs.backlog.is_empty() {
            thumbs.backlog.extend(thumbs.pipeline.try_iter());
            std::thread::sleep(Duration::from_millis(10));
        }

        library.apply(&FsChange::Remove(img.clone()), Instant::now());
        thumbs.poll(&ctx, &mut [(&mut library, &mut gallery)]);
        library.apply(&FsChange::Upsert(img.clone()), Instant::now());
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn retry_failed_gives_broken_files_another_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"not an image").unwrap();
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &bad, |e| e.error));

        gallery.retry_failed(&mut thumbs, &mut library);
        assert!(!library.get(&bad).unwrap().error);
        probe(&thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &bad, |e| e.error));
    }

    #[test]
    fn result_goes_to_the_tab_that_lists_the_file() {
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let img = dir_a.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery_a) = test_thumbs(dir_a.path());
        let (mut lib_a, mut lib_b) = (library_of(dir_a.path()), library_of(dir_b.path()));
        let mut gallery_b = Gallery::new();
        probe(&thumbs, &mut gallery_a, &lib_a);

        let start = Instant::now();
        while lib_a.get(&img).unwrap().dims.is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "result never routed to tab A");
            thumbs.poll(&ctx, &mut [(&mut lib_b, &mut gallery_b), (&mut lib_a, &mut gallery_a)]);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(lib_b.is_empty());
    }

    #[test]
    fn result_for_a_file_no_tab_lists_is_dropped_and_requestable_again() {
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let img = dir_a.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery_a) = test_thumbs(dir_a.path());
        let (mut lib_a, mut lib_b) = (library_of(dir_a.path()), library_of(dir_b.path()));
        let mut gallery_b = Gallery::new();
        probe(&thumbs, &mut gallery_a, &lib_a);
        while thumbs.backlog.is_empty() {
            thumbs.backlog.extend(thumbs.pipeline.try_iter());
            std::thread::sleep(Duration::from_millis(10));
        }

        thumbs.poll(&ctx, &mut [(&mut lib_b, &mut gallery_b)]);
        assert!(thumbs.backlog.is_empty());
        assert_eq!(lib_a.get(&img).unwrap().dims, None);
        probe(&thumbs, &mut gallery_a, &lib_a);
        assert!(settles(&ctx, &mut thumbs, &mut gallery_a, &mut lib_a, &img, has_dims));
    }

    #[test]
    fn collapse_key_names_root_dot() {
        assert_eq!(collapse_key(""), ".");
        assert_eq!(collapse_key("Sub/Deep"), "sub/deep");
    }
}
