use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::epaint::RectShape;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, Context, CornerRadius, CursorIcon, FontFamily, FontId, Frame, Id,
    Layout as UiLayout, Margin, Modifiers, Painter, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    TextureHandle, TextureId, TextureOptions, Tooltip, Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::theme::{self, Palette, icons};
use super::status;
use crate::formats;
use crate::config::{self, Config};
use crate::layout::{self, BentoRule, Layout, LayoutItem, LayoutKind, LayoutSection};
use crate::library::{Entry, Filter, FsChange, Library, QuickCounts, Section, SortOrder};
use crate::selection::Selection;
use crate::thumbs::{
    DiskCache, ThumbJob, ThumbKey, ThumbOutcome, ThumbPipeline, ThumbResult, key_for,
};

const MIN_TEXTURES: usize = 400;
const TEXTURE_SLACK: usize = 64;
const UPLOADS_PER_FRAME: usize = 16;
const HIGHLIGHT: Duration = Duration::from_millis(1500);
const DIMS_RELAYOUT_EVERY: Duration = Duration::from_millis(150);
const TILE_RADIUS: u8 = 3;
/// Loading placeholder to image cross-fade.
const FADE_SECS: f32 = 0.12;
const TOOLTIP_WIDTH: f32 = 264.0;
const TOOLTIP_IMAGE_H: f32 = 150.0;
const FOCUS_OUTER: Color32 = Color32::from_rgb(242, 242, 245);
const FOCUS_INNER: Color32 = Color32::from_rgb(17, 17, 19);
const CHECK_INK: Color32 = Color32::from_rgb(13, 26, 46);
const ARRIVAL_INK: Color32 = Color32::from_rgb(20, 20, 22);

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
    flat: bool,
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
    pinned: bool,
}

/// What the gallery reads from the app each frame.
pub struct View<'a> {
    pub library: &'a Library,
    pub thumbs: &'a mut Thumbs,
    pub config: &'a mut Config,
    pub sort: SortOrder,
    pub recursive: bool,
    /// One ungrouped list; only meaningful with `recursive`.
    pub flat: bool,
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

static NEXT_GALLERY_ID: AtomicU64 = AtomicU64::new(1);

/// Decode pool, disk cache and GPU textures, shared by every tab's `Gallery`.
///
/// Each visible gallery keeps its own request lists; the pipeline gets them interleaved,
/// so several windows can show a tab at once without one starving the other.
pub struct Thumbs {
    pipeline: ThumbPipeline,
    cache: DiskCache,
    textures: TextureCache,
    backlog: VecDeque<ThumbResult>,
    wanted: Requests,
    probes: Requests,
    near_tiles: HashMap<u64, usize>,
}

/// Jobs requested by each gallery, by gallery id, in the order the galleries first asked.
#[derive(Default)]
struct Requests(Vec<(u64, Vec<ThumbJob>)>);

impl Requests {
    fn set(&mut self, owner: u64, jobs: Vec<ThumbJob>) {
        match self.0.iter_mut().find(|(id, _)| *id == owner) {
            Some((_, slot)) => *slot = jobs,
            None => self.0.push((owner, jobs)),
        }
    }

    /// Drops the lists of galleries `keep` rejects; true when any was dropped.
    fn retain(&mut self, keep: &impl Fn(u64) -> bool) -> bool {
        let before = self.0.len();
        self.0.retain(|(id, _)| keep(*id));
        self.0.len() != before
    }

    fn merged(&self) -> Vec<ThumbJob> {
        interleave(self.0.iter().map(|(_, jobs)| jobs.as_slice()).collect())
    }
}

/// First item of each list, then the second of each, and so on.
fn interleave<T: Clone>(lists: Vec<&[T]>) -> Vec<T> {
    let longest = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    (0..longest).flat_map(|i| lists.iter().filter_map(move |l| l.get(i).cloned())).collect()
}

pub struct Gallery {
    id: u64,
    lib_version: u64,
    dims_version: u64,
    sections_key: Option<(u64, SortOrder, bool, bool)>,
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
    /// Name and quick filters; kept across `reset`.
    filter: Filter,
    /// Chip counts with the library version and start of day they were computed for.
    quick_counts: Option<(u64, SystemTime, QuickCounts)>,
    /// Bumped on every rebuild of `order`.
    order_version: u64,
    arrivals: Arrivals,
}

fn open_cache() -> DiskCache {
    let dir = DiskCache::default_dir();
    DiskCache::open(dir.clone()).unwrap_or_else(|e| {
        log::warn!("cannot open thumbnail cache {}: {e}", dir.display());
        DiskCache::open(std::env::temp_dir().join("image-moz").join("thumbs"))
            .expect("no writable thumbnail cache directory")
    })
}

fn root_name(root: &Path) -> String {
    root.file_name().unwrap_or(root.as_os_str()).to_string_lossy().into_owned()
}

fn collapse_key(rel_dir: &str) -> String {
    if rel_dir.is_empty() { ".".to_string() } else { rel_dir.to_lowercase() }
}

fn fit(rect: Rect, size: Vec2) -> Rect {
    let scale = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * scale)
}

/// UV rectangle that fills `cell` with an image of size `img`, cropping the overflow evenly (CSS `cover`).
fn cover_uv(img: Vec2, cell: Vec2) -> Rect {
    let (image_aspect, cell_aspect) = (img.x / img.y.max(1.0), cell.x / cell.y.max(1.0));
    if image_aspect > cell_aspect {
        let k = cell_aspect / image_aspect;
        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
    } else {
        let k = image_aspect / cell_aspect;
        Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
    }
}

/// Opacity of the new-arrival ring `age` after the file appeared; `None` once it has faded out.
fn arrival_alpha(age: Duration) -> Option<f32> {
    (age < HIGHLIGHT).then(|| 1.0 - egui::emath::easing::cubic_out(age.as_secs_f32() / HIGHLIGHT.as_secs_f32()))
}

/// `tile` widened upwards so that scrolling it into view leaves it clear of a sticky header.
fn reveal_rect(tile: Rect, top_inset: f32) -> Rect {
    Rect::from_min_max(pos2(tile.min.x, tile.min.y - top_inset), tile.max)
}

/// New files per section key, shown on the section header until the user has looked at it and moved on.
#[derive(Default)]
struct Arrivals {
    counts: HashMap<String, usize>,
    in_view: HashSet<String>,
}

impl Arrivals {
    fn note(&mut self, key: String) {
        *self.counts.entry(key).or_default() += 1;
    }

    fn count(&self, key: &str) -> usize {
        self.counts.get(key).copied().unwrap_or(0)
    }

    /// Sections on screen last frame but not now have been seen: their count restarts from zero.
    fn settle(&mut self, in_view: HashSet<String>) {
        for key in self.in_view.difference(&in_view) {
            self.counts.remove(key);
        }
        self.in_view = in_view;
    }

    fn clear(&mut self) {
        self.counts.clear();
        self.in_view.clear();
    }
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::SEMIBOLD.into()))
}

fn nouvelles_text(count: usize) -> String {
    if count == 1 { "+1 nouvelle".to_string() } else { format!("+{count} nouvelles") }
}

struct Pill<'a> {
    text: &'a str,
    font: FontId,
    fill: Color32,
    ink: Color32,
    height: f32,
}

/// A fully rounded label whose right edge is at `right`; returns its rectangle.
fn paint_pill(painter: &Painter, right: f32, top: f32, pill: &Pill) -> Rect {
    const PAD: f32 = 6.0;
    let galley = painter.layout_no_wrap(pill.text.to_string(), pill.font.clone(), pill.ink);
    let size = vec2(galley.size().x + 2.0 * PAD, pill.height);
    let rect = Rect::from_min_size(pos2(right - size.x, top), size);
    painter.rect_filled(rect, CornerRadius::same((pill.height / 2.0) as u8), pill.fill);
    painter.galley(pos2(rect.min.x + PAD, rect.center().y - galley.size().y / 2.0), galley, pill.ink);
    rect
}

/// Format label at the bottom-left corner of a tile.
fn draw_badge(painter: &Painter, tile: Rect, label: &str, p: &Palette) {
    let format = TextFormat {
        font_id: semibold(10.0),
        color: Color32::WHITE,
        extra_letter_spacing: 0.4,
        ..Default::default()
    };
    let galley = painter.layout_job(LayoutJob::single_section(label.to_string(), format));
    let rect = Rect::from_min_size(
        pos2(tile.left() + 6.0, tile.bottom() - 6.0 - 18.0),
        vec2(galley.size().x + 12.0, 18.0),
    );
    painter.rect_filled(rect, CornerRadius::same(4), p.badge_bg);
    painter.galley(pos2(rect.min.x + 6.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
}

/// Pin pill at the top-right corner of a tile.
fn draw_pin(painter: &Painter, tile: Rect, p: &Palette) {
    let center = pos2(tile.right() - 6.0 - 11.0, tile.top() + 6.0 + 11.0);
    painter.circle_filled(center, 11.0, p.badge_bg);
    painter.text(center, Align2::CENTER_CENTER, icons::PUSH_PIN, theme::icon_font(12.0), Color32::WHITE);
}

/// A glyph over a short word, centred; the word is dropped when the tile is too small for it.
fn draw_note(painter: &Painter, rect: Rect, glyph: &str, glyph_color: Color32, word: &str, word_color: Color32) {
    let tiny = rect.width().min(rect.height()) < 64.0;
    let size = if tiny { 20.0 } else { 28.0 };
    let icon = painter.layout_no_wrap(glyph.to_string(), theme::icon_font(size), glyph_color);
    let text = painter.layout_no_wrap(word.to_string(), FontId::proportional(11.5), word_color);
    let with_word = !tiny && text.size().x + 8.0 <= rect.width();
    let icon_h = icon.size().y;
    let total = icon_h + if with_word { 8.0 + text.size().y } else { 0.0 };
    let top = rect.center().y - total / 2.0;
    painter.galley(pos2(rect.center().x - icon.size().x / 2.0, top), icon, glyph_color);
    if with_word {
        painter.galley(pos2(rect.center().x - text.size().x / 2.0, top + icon_h + 8.0), text, word_color);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Placeholder {
    Loading,
    Unreadable,
    NoPreview,
}

struct TileLook {
    texture: Option<(TextureId, Vec2)>,
    /// Image opacity, 0 to 1, during the fade-in.
    fade: f32,
    placeholder: Placeholder,
    badge: Option<&'static str>,
    hovered: bool,
    selected: bool,
    focused: bool,
    pinned: bool,
    /// Ring opacity of a file that just arrived.
    arrival: Option<f32>,
}

fn draw_tile(painter: &Painter, rect: Rect, look: &TileLook, p: &Palette) {
    let radius = CornerRadius::same(TILE_RADIUS);
    painter.rect_filled(rect, radius, p.tile_placeholder);
    match (look.texture, look.placeholder) {
        (Some((texture, size)), _) => {
            let tint = Color32::WHITE.gamma_multiply(look.fade);
            painter.add(RectShape::filled(rect, radius, tint).with_texture(texture, cover_uv(size, rect.size())));
        }
        (None, Placeholder::Unreadable) => draw_note(painter, rect, icons::WARNING, p.error, "Illisible", p.text_weak),
        (None, Placeholder::NoPreview) => {
            let dim = p.text_weak.gamma_multiply(0.7);
            draw_note(painter, rect, icons::CUBE, dim, "pas d'aperçu", p.text_weak);
        }
        (None, Placeholder::Loading) => {}
    }
    if look.selected {
        painter.rect_filled(rect, radius, p.accent.gamma_multiply(0.10));
    } else if look.hovered {
        painter.rect_filled(rect, radius, Color32::from_white_alpha(15));
        painter.rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_white_alpha(89)), StrokeKind::Inside);
    }
    if let Some(label) = look.badge {
        draw_badge(painter, rect, label, p);
    }
    if look.pinned {
        draw_pin(painter, rect, p);
    }
    let inner = CornerRadius::same(1);
    match (look.selected, look.focused) {
        (true, false) => {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, p.accent), StrokeKind::Inside);
            let line = Stroke::new(1.0, Color32::from_black_alpha(153));
            painter.rect_stroke(rect.shrink(2.0), inner, line, StrokeKind::Inside);
        }
        (false, true) => {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, FOCUS_OUTER), StrokeKind::Inside);
            painter.rect_stroke(rect.shrink(2.0), inner, Stroke::new(2.0, FOCUS_INNER), StrokeKind::Inside);
        }
        (true, true) => {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, FOCUS_OUTER), StrokeKind::Inside);
            painter.rect_stroke(rect.shrink(2.0), inner, Stroke::new(2.0, p.accent), StrokeKind::Inside);
        }
        (false, false) => {}
    }
    if look.selected {
        let inset = if look.focused { 8.0 } else { 6.0 };
        let center = rect.left_top() + vec2(inset + 10.0, inset + 10.0);
        painter.circle_filled(center, 10.0, p.accent);
        painter.text(center, Align2::CENTER_CENTER, icons::CHECK, theme::icon_font(12.0), CHECK_INK);
    }
    if let Some(alpha) = look.arrival {
        painter.rect_stroke(rect, radius, Stroke::new(4.0, p.arrival.gamma_multiply(alpha)), StrokeKind::Inside);
        if rect.width() >= 100.0 {
            let right = rect.right() - 8.0 - if look.pinned { 26.0 } else { 0.0 };
            let pill = Pill {
                text: "NOUVEAU",
                font: semibold(10.5),
                fill: p.arrival.gamma_multiply(alpha),
                ink: ARRIVAL_INK.gamma_multiply(alpha),
                height: 18.0,
            };
            paint_pill(painter, right, rect.top() + 8.0, &pill);
        }
    }
}

fn draw_header(ui: &Ui, rect: Rect, hovered: bool, meta: &SectionMeta, new_count: usize, p: &Palette) {
    let painter = ui.painter();
    let fill = if hovered { p.status_bg.lerp_to_gamma(p.hover_bg, 0.5) } else { p.status_bg };
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
    let mid = rect.center().y;
    let mut x = rect.left() + 12.0;
    let mut put = |text: String, font: FontId, color: Color32| {
        let galley = painter.layout_no_wrap(text, font, color);
        let width = galley.size().x;
        painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, color);
        x += width + 8.0;
    };
    let caret = if meta.collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
    put(caret.to_string(), theme::icon_font(10.0), if hovered { p.text } else { p.text_weak });
    put(meta.label.clone(), FontId::new(12.5, FontFamily::Monospace), p.text);
    let mut count = status::images_text(meta.count);
    if meta.collapsed {
        count.push_str(" · repliée");
    }
    put(count, FontId::proportional(13.0), p.text_weak);
    if new_count > 0 {
        let (fill, ink) = (p.new_pill_bg, p.new_pill_text);
        let text = nouvelles_text(new_count);
        let width = painter.layout_no_wrap(text.clone(), semibold(11.0), ink).size().x + 12.0;
        let pill = Pill { text: &text, font: semibold(11.0), fill, ink, height: 18.0 };
        paint_pill(painter, x + width, mid - 9.0, &pill);
    }
}

/// `« chat » · Aujourd'hui · PNG, JPG`: the active criteria, for the empty state.
fn filter_description(filter: &Filter) -> String {
    let mut parts = Vec::new();
    if !filter.text.trim().is_empty() {
        parts.push(format!("« {} »", filter.text.trim()));
    }
    if filter.since.is_some() {
        parts.push("Aujourd'hui".to_owned());
    }
    if !filter.types.is_empty() {
        parts.push(filter.types.iter().map(String::as_str).collect::<Vec<_>>().join(", "));
    }
    parts.join(" · ")
}

fn tooltip_ui(ui: &mut Ui, entry: &Entry, texture: Option<(TextureId, Vec2)>, recursive: bool, p: &Palette) {
    Frame::NONE.inner_margin(Margin::same(2)).show(ui, |ui| {
        let width = TOOLTIP_WIDTH - ui.spacing().menu_margin.sum().x - 4.0;
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 3.0;
        if let Some((id, size)) = texture {
            let (area, _) = ui.allocate_exact_size(vec2(width, TOOLTIP_IMAGE_H), Sense::hover());
            let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            ui.painter().rect_filled(area, CornerRadius::same(4), p.gallery_bg);
            ui.painter().image(id, fit(area, size), full, Color32::WHITE);
            ui.add_space(3.0);
        }
        ui.add(egui::Label::new(RichText::new(&entry.name).font(semibold(13.0)).color(p.text)).truncate());
        ui.label(RichText::new(status::meta_text(entry)).font(FontId::monospace(11.5)).color(p.text_muted));
        for (label, date) in status::date_parts(entry) {
            let mut job = LayoutJob::default();
            let font = FontId::monospace(11.5);
            job.append(&status::capitalized(label), 0.0, TextFormat::simple(font.clone(), p.text_weak));
            job.append(&format!(" {date}"), 0.0, TextFormat::simple(font, p.text_muted));
            ui.label(job);
        }
        if recursive && !entry.rel_dir.is_empty() {
            ui.label(RichText::new(&entry.rel_dir).size(12.0).color(p.text_weak));
        }
    });
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
            pipeline: ThumbPipeline::new(cache.clone(), wake),
            cache,
            textures: TextureCache::default(),
            backlog: VecDeque::new(),
            wanted: Requests::default(),
            probes: Requests::default(),
            near_tiles: HashMap::new(),
        }
    }

    /// Drops the texture and the disk entry of `key` so its next request renders from the source.
    fn invalidate(&mut self, key: ThumbKey) {
        self.textures.remove(key);
        self.cache.remove(key);
    }

    /// Drops every queued request, e.g. when no tab is left to want them.
    pub fn cancel_requests(&mut self) {
        self.retain_galleries(|_| false);
    }

    /// Forgets the requests of galleries that are no longer on screen.
    pub fn retain_galleries(&mut self, keep: impl Fn(u64) -> bool) {
        if self.wanted.retain(&keep) {
            self.pipeline.set_wanted(self.wanted.merged());
        }
        if self.probes.retain(&keep) {
            self.pipeline.set_probe(self.probes.merged());
        }
        self.near_tiles.retain(|id, _| keep(*id));
        self.fit_cap();
    }

    fn set_wanted(&mut self, owner: u64, jobs: Vec<ThumbJob>) {
        self.wanted.set(owner, jobs);
        self.pipeline.set_wanted(self.wanted.merged());
    }

    fn set_probe(&mut self, owner: u64, jobs: Vec<ThumbJob>) {
        self.probes.set(owner, jobs);
        self.pipeline.set_probe(self.probes.merged());
    }

    /// Every visible gallery keeps its near zone cached, so the cap is the sum over galleries.
    fn note_near_tiles(&mut self, owner: u64, count: usize) {
        self.near_tiles.insert(owner, count);
        self.fit_cap();
    }

    fn fit_cap(&mut self) {
        self.textures.fit_cap(self.near_tiles.values().sum());
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
                        entry.no_preview = false;
                        entry.triangles = thumb.triangles;
                        Some(thumb.orig_dims)
                    }
                    ThumbOutcome::Dims(dims) => Some(*dims),
                    ThumbOutcome::Failed => {
                        entry.error = true;
                        None
                    }
                    ThumbOutcome::NoPreview => {
                        entry.no_preview = true;
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
            id: NEXT_GALLERY_ID.fetch_add(1, Ordering::Relaxed),
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
            filter: Filter::default(),
            quick_counts: None,
            order_version: 0,
            arrivals: Arrivals::default(),
        }
    }

    /// Names this gallery's requests in the shared `Thumbs`; stable for the tab's life.
    pub fn id(&self) -> u64 {
        self.id
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

    pub fn scroll_to_top(&mut self) {
        self.reset_scroll = true;
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    /// Narrows the gallery to `filter` and returns the view to the top.
    pub fn set_filter(&mut self, filter: Filter) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        self.library_changed();
        self.scroll_to_top();
    }

    pub fn clear_filter(&mut self) {
        self.set_filter(Filter::default());
    }

    /// Chip counts over the whole library, recomputed when it changes or the day rolls over.
    pub fn quick_counts(&mut self, library: &Library, since: SystemTime) -> &QuickCounts {
        let stale = self.quick_counts.as_ref().is_none_or(|(v, s, _)| *v != self.lib_version || *s != since);
        if stale {
            self.quick_counts = Some((self.lib_version, since, library.quick_counts(since)));
        }
        self.quick_counts.as_ref().map(|(_, _, c)| c).expect("set above")
    }

    /// Images matching the filter, across collapsed sections too; `None` when no filter is set.
    pub fn match_count(&self) -> Option<usize> {
        if !self.filter.is_active() {
            return None;
        }
        Some(self.sections.iter().map(|s| s.items.len()).sum())
    }

    /// Entries were added, removed or modified: sections and layout must be rebuilt.
    pub fn library_changed(&mut self) {
        self.lib_version += 1;
        self.probe_dirty = true;
    }

    /// The folder is re-read: rebuild from scratch. Textures stay, they are keyed by file state.
    pub fn reset(&mut self, thumbs: &mut Thumbs) {
        thumbs.set_wanted(self.id, Vec::new());
        thumbs.set_probe(self.id, Vec::new());
        thumbs.pipeline.forget_all();
        self.last_wanted = None;
        self.sections_key = None;
        self.sig = None;
        self.anchor = None;
        self.scroll_y = 0.0;
        self.reset_scroll = true;
        self.arrivals.clear();
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
        let added = library.apply(change, now);
        if let (true, FsChange::Upsert(path)) = (added, change)
            && let Some(entry) = library.get(path)
        {
            self.arrivals.note(collapse_key(&entry.rel_dir));
        }
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
        for entry in entries.iter().filter(|e| e.arrived_at.is_some() && library.get(&e.path).is_none()) {
            self.arrivals.note(collapse_key(&entry.rel_dir));
        }
        library.replace_all(entries);
        self.library_changed();
    }

    /// User refresh: failed tiles get another attempt, and model thumbnails are rendered anew
    /// because their annex files (.mtl, .bin, textures) change without touching the model's key.
    pub fn retry_failed(&mut self, thumbs: &mut Thumbs, library: &mut Library) {
        library.clear_errors();
        for entry in library.iter().filter(|e| formats::kind(&e.path) == Some(formats::MediaKind::Model)) {
            thumbs.invalidate(key_for(&entry.path, entry.mtime, entry.size));
        }
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
            flat: view.flat,
            pins: state.map_or(0, |s| s.pins.len()),
            collapsed: state.map_or(0, |s| s.collapsed.len()),
        }
    }

    fn rebuild(&mut self, view: &View, sig: &LayoutSig) {
        let cfg = &*view.config;
        let sections_key = (sig.lib, sig.sort, sig.recursive, sig.flat);
        if self.sections_key != Some(sections_key) {
            self.sections = view.library.sections(view.sort, &self.filter, view.flat);
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
            let is_collapsed = view.recursive && !view.flat && collapsed.is_some_and(|c| c.contains(&key));
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
                    self.items.push(TileItem { key: key_for(path, entry.mtime, entry.size), pinned });
                    self.order.push(path.clone());
                }
            }
            let label = if section.rel_dir.is_empty() { root_name(view.root) } else { section.rel_dir.clone() };
            layout_sections.push(LayoutSection {
                header: (view.recursive && !view.flat).then(|| label.clone()),
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

    fn update_probes(&mut self, thumbs: &mut Thumbs, library: &Library) {
        let mut jobs = Vec::new();
        for path in self.sections.iter().flat_map(|s| &s.items) {
            if let Some(e) = library.get(path)
                && e.dims.is_none()
                && !e.error
                && !e.no_preview
            {
                jobs.push(ThumbJob { path: path.clone(), key: key_for(path, e.mtime, e.size) });
            }
        }
        thumbs.set_probe(self.id, jobs);
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
            .id_salt(("gallery_scroll", self.id))
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
                let inset = if self.layout.headers.is_empty() { 0.0 } else { layout::HEADER_H + layout::GAP };
                ui.scroll_to_rect(reveal_rect(tile.rect, inset).translate(origin), None);
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
        if self.match_count() == Some(0) {
            self.empty_filter(ui, scroll.inner_rect, view.library.len());
        }
        out
    }

    fn empty_filter(&mut self, ui: &mut Ui, area: Rect, total: usize) {
        let p = theme::palette(ui.ctx());
        let mut clear = false;
        let builder = UiBuilder::new().max_rect(area).layout(UiLayout::top_down(Align::Center));
        ui.scope_builder(builder, |ui| {
            let label = |ui: &mut Ui, text: RichText| ui.add(egui::Label::new(text).selectable(false));
            ui.spacing_mut().item_spacing.y = 12.0;
            ui.add_space(((area.height() - 150.0) / 2.0).max(0.0));
            label(ui, RichText::new(icons::MAGNIFYING_GLASS).font(theme::icon_font(36.0)).color(p.text_weak.gamma_multiply(0.6)));
            label(ui, RichText::new("Aucun fichier ne correspond au filtre").font(semibold(15.0)).color(p.text));
            let line = format!("{} · 0 sur {}", filter_description(&self.filter), status::images_text(total));
            label(ui, RichText::new(line).size(12.5).color(p.text_weak));
            let clear_label = if self.filter.has_quick() { "Effacer les filtres" } else { "Effacer le filtre" };
            let button = egui::Button::new(RichText::new(clear_label).color(p.text))
                .fill(p.button_fill)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(6))
                .min_size(vec2(0.0, 30.0));
            clear = ui.add(button).clicked();
        });
        if clear {
            self.clear_filter();
            ui.ctx().request_repaint();
        }
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
        let p = theme::palette(ui.ctx());
        let now = Instant::now();
        let mut arriving = false;
        let mut first: Option<(f32, f32, usize)> = None;
        let mut near_tiles = 0;
        let mut near_jobs: Vec<ThumbJob> = Vec::new();
        let mut far_jobs: Vec<(f32, ThumbJob)> = Vec::new();

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
            if !loaded && !entry.is_some_and(|e| e.error || e.no_preview) {
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
            let texture = view.thumbs.textures.get(key).map(|t| (t.id(), t.size_vec2()));
            let placeholder = match entry {
                Some(e) if e.error => Placeholder::Unreadable,
                Some(e) if e.no_preview => Placeholder::NoPreview,
                _ => Placeholder::Loading,
            };
            let arrival = entry
                .and_then(|e| e.arrived_at)
                .and_then(|at| arrival_alpha(now.saturating_duration_since(at)));
            arriving |= arrival.is_some();
            let look = TileLook {
                texture,
                fade: ui.ctx().animate_bool_with_time(Id::new(("tile_fade", key)), texture.is_some(), FADE_SECS),
                placeholder,
                badge: formats::badge(path),
                hovered: response.hovered(),
                selected: view.selection.contains(id),
                focused: view.selection.focus() == Some(id),
                pinned: self.items[id].pinned,
                arrival,
            };
            draw_tile(ui.painter(), rect, &look, p);

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
                Tooltip::for_enabled(&response)
                    .at_pointer()
                    .gap(12.0)
                    .show(|ui| tooltip_ui(ui, entry, texture, recursive, p));
            }
        }

        // Headers go last so they sit on top of the tiles, in drawing and in hit order.
        let headers = &self.layout.headers;
        let mut seen = HashSet::new();
        for (i, header) in headers.iter().enumerate() {
            let next_y = headers.get(i + 1).map(|h| h.rect.min.y);
            let meta = &self.meta[header.section];
            let end = next_y.unwrap_or(self.layout.height);
            if Rect::from_min_max(pos2(0.0, header.rect.min.y), pos2(width, end)).intersects(viewport) {
                seen.insert(meta.key.clone());
            }
            let y = layout::sticky_y(header.rect.min.y, next_y, viewport.min.y);
            let rel = Rect::from_min_size(pos2(header.rect.min.x, y), header.rect.size());
            if !rel.intersects(viewport) {
                continue;
            }
            let rect = rel.translate(origin);
            let id = Id::new(("section_header", self.id, header.section));
            let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            draw_header(ui, rect, response.hovered(), meta, self.arrivals.count(&meta.key), p);
            if response.clicked() {
                *toggled = Some(meta.key.clone());
            }
        }
        if headers.is_empty() {
            self.arrivals.clear();
        } else {
            self.arrivals.settle(seen);
        }

        self.anchor = first.map(|(y, _, id)| (self.order[id].clone(), y - viewport.min.y));
        if arriving {
            ui.ctx().request_repaint();
        }
        far_jobs.sort_by(|a, b| a.0.total_cmp(&b.0));
        near_jobs.extend(far_jobs.into_iter().map(|(_, job)| job));
        view.thumbs.note_near_tiles(self.id, near_tiles);
        self.request_thumbs(view.thumbs, near_jobs);
    }

    fn request_thumbs(&mut self, thumbs: &mut Thumbs, jobs: Vec<ThumbJob>) {
        let keys: Vec<ThumbKey> = jobs.iter().map(|j| j.key).collect();
        if self.last_wanted.as_ref() != Some(&keys) {
            self.last_wanted = Some(keys);
            thumbs.set_wanted(self.id, jobs);
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
    fn cover_crops_the_longer_side_and_centres_it() {
        let wide = cover_uv(vec2(200.0, 100.0), vec2(100.0, 100.0));
        assert_eq!((wide.min, wide.max), (pos2(0.25, 0.0), pos2(0.75, 1.0)));
        let tall = cover_uv(vec2(100.0, 200.0), vec2(100.0, 100.0));
        assert_eq!((tall.min, tall.max), (pos2(0.0, 0.25), pos2(1.0, 0.75)));
        let exact = cover_uv(vec2(512.0, 256.0), vec2(300.0, 150.0));
        assert_eq!((exact.min, exact.max), (pos2(0.0, 0.0), pos2(1.0, 1.0)));
    }

    #[test]
    fn arrival_ring_fades_out_over_the_highlight() {
        assert_eq!(arrival_alpha(Duration::ZERO), Some(1.0));
        let mid = arrival_alpha(HIGHLIGHT / 2).unwrap();
        assert!(mid > 0.0 && mid < 1.0);
        assert!(arrival_alpha(HIGHLIGHT - Duration::from_millis(1)).unwrap() < 0.01);
        assert_eq!(arrival_alpha(HIGHLIGHT), None);
    }

    #[test]
    fn revealed_tile_clears_the_sticky_header() {
        let tile = Rect::from_min_size(pos2(10.0, 500.0), vec2(100.0, 100.0));
        let wanted = reveal_rect(tile, 36.0);
        assert_eq!((wanted.min, wanted.max), (pos2(10.0, 464.0), tile.max));
        assert_eq!(reveal_rect(tile, 0.0), tile);
    }

    #[test]
    fn arrivals_stay_until_the_section_was_seen_and_left() {
        let keys = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<HashSet<_>>();
        let mut arrivals = Arrivals::default();
        arrivals.note("a".into());
        arrivals.note("a".into());
        arrivals.note("b".into());
        arrivals.settle(keys(&["a"]));
        assert_eq!((arrivals.count("a"), arrivals.count("b")), (2, 1));
        arrivals.note("a".into());
        arrivals.settle(keys(&["a", "b"]));
        assert_eq!((arrivals.count("a"), arrivals.count("b")), (3, 1));
        arrivals.settle(keys(&["b"]));
        assert_eq!((arrivals.count("a"), arrivals.count("b")), (0, 1));
        arrivals.settle(keys(&[]));
        assert_eq!(arrivals.count("b"), 0);
    }

    #[test]
    fn new_files_are_counted_by_section() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Sub");
        std::fs::create_dir_all(&sub).unwrap();
        write_image(&dir.path().join("old.png"));
        let (_ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = Library::new(dir.path().to_path_buf(), true);
        library.replace_all(crate::library::scan(dir.path(), true).unwrap());

        let fresh = sub.join("new.png");
        write_image(&fresh);
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Upsert(fresh.clone()), Instant::now());
        assert_eq!((gallery.arrivals.count("sub"), gallery.arrivals.count(".")), (1, 0));
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Upsert(fresh), Instant::now());
        assert_eq!(gallery.arrivals.count("sub"), 1);

        let late = dir.path().join("late.png");
        write_image(&late);
        let mut scanned = crate::library::scan(dir.path(), true).unwrap();
        for entry in scanned.iter_mut().filter(|e| e.path == late) {
            entry.arrived_at = Some(Instant::now());
        }
        gallery.replace_entries(&mut thumbs, &mut library, scanned);
        assert_eq!(gallery.arrivals.count("."), 1);
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

    fn job(n: u64) -> ThumbJob {
        ThumbJob { path: PathBuf::from(format!("{n}.png")), key: ThumbKey(n) }
    }

    fn keys(jobs: &[ThumbJob]) -> Vec<u64> {
        jobs.iter().map(|j| j.key.0).collect()
    }

    #[test]
    fn galleries_are_served_in_turn() {
        assert_eq!(interleave(vec![&[1, 2, 3][..], &[10][..], &[][..], &[20, 21][..]]), [1, 10, 20, 2, 21, 3]);
        assert_eq!(interleave::<u8>(Vec::new()), Vec::<u8>::new());
    }

    #[test]
    fn each_gallery_keeps_its_own_requests_until_it_is_hidden() {
        let mut requests = Requests::default();
        requests.set(1, vec![job(1), job(2)]);
        requests.set(2, vec![job(10)]);
        assert_eq!(keys(&requests.merged()), [1, 10, 2]);
        requests.set(1, vec![job(3)]);
        assert_eq!(keys(&requests.merged()), [3, 10]);
        assert!(!requests.retain(&|_| true));
        assert!(requests.retain(&|id| id == 2));
        assert_eq!(keys(&requests.merged()), [10]);
    }

    #[test]
    fn texture_cap_covers_every_visible_gallery() {
        let dir = tempfile::tempdir().unwrap();
        let (_ctx, mut thumbs, _gallery) = test_thumbs(dir.path());
        thumbs.note_near_tiles(1, 500);
        thumbs.note_near_tiles(2, 600);
        assert_eq!(thumbs.textures.cap, 1100 + TEXTURE_SLACK);
        thumbs.note_near_tiles(2, 100);
        assert_eq!(thumbs.textures.cap, 600 + TEXTURE_SLACK);
        thumbs.retain_galleries(|id| id == 2);
        assert_eq!(thumbs.textures.cap, MIN_TEXTURES);
    }

    #[test]
    fn two_visible_galleries_both_get_their_thumbnails() {
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (img_a, img_b) = (dir_a.path().join("a.png"), dir_b.path().join("b.png"));
        write_image(&img_a);
        write_image(&img_b);
        let (ctx, mut thumbs, mut gallery_a) = test_thumbs(dir_a.path());
        let mut gallery_b = Gallery::new();
        let (mut lib_a, mut lib_b) = (library_of(dir_a.path()), library_of(dir_b.path()));
        probe(&mut thumbs, &mut gallery_a, &lib_a);
        probe(&mut thumbs, &mut gallery_b, &lib_b);
        let start = Instant::now();
        while lib_a.get(&img_a).unwrap().dims.is_none() || lib_b.get(&img_b).unwrap().dims.is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "a gallery was starved");
            thumbs.poll(&ctx, &mut [(&mut lib_a, &mut gallery_a), (&mut lib_b, &mut gallery_b)]);
            std::thread::sleep(Duration::from_millis(10));
        }
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
        theme::install_fonts(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let cache = DiskCache::open(dir.join("cache")).unwrap();
        (ctx.clone(), Thumbs::with_cache(&ctx, cache), Gallery::new())
    }

    fn library_of(dir: &Path) -> Library {
        let mut library = Library::new(dir.to_path_buf(), false);
        library.replace_all(crate::library::scan(dir, false).unwrap());
        library
    }

    fn probe(thumbs: &mut Thumbs, gallery: &mut Gallery, library: &Library) {
        gallery.sections = library.sections(SortOrder::default(), &Filter::default(), false);
        gallery.library_changed();
        gallery.update_probes(thumbs, library);
    }

    fn show_once(
        ctx: &Context,
        thumbs: &mut Thumbs,
        gallery: &mut Gallery,
        library: &Library,
        dir: &Path,
        flat: bool,
    ) {
        let mut config = Config::default();
        let selection = Selection::default();
        let raw = eframe::egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            let view = View {
                library,
                thumbs: &mut *thumbs,
                config: &mut config,
                sort: SortOrder::NameAsc,
                recursive: true,
                flat,
                root: dir,
                selection: &selection,
                scroll_to: None,
            };
            gallery.show(ui, view);
        });
    }

    #[test]
    fn filter_description_lists_every_active_criterion() {
        let filter = Filter {
            text: " chat ".to_owned(),
            since: Some(SystemTime::UNIX_EPOCH),
            types: ["PNG".to_owned(), "JPG".to_owned()].into(),
        };
        assert_eq!(filter_description(&filter), "« chat » · Aujourd'hui · JPG, PNG");
        assert_eq!(filter_description(&Filter::named("x")), "« x »");
    }

    #[test]
    fn filter_and_flat_rebuild_the_layout_on_next_show() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["cat.png", "dog.png", "sub/cat2.png"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            write_image(&path);
        }
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = Library::new(dir.path().to_path_buf(), true);
        library.replace_all(crate::library::scan(dir.path(), true).unwrap());

        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);
        assert_eq!(gallery.order().len(), 3);
        assert_eq!(gallery.layout().headers.len(), 2);
        assert_eq!(gallery.match_count(), None);

        gallery.set_filter(Filter::named("CAT"));
        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);
        assert_eq!(gallery.order().len(), 2);
        assert_eq!(gallery.match_count(), Some(2));

        gallery.clear_filter();
        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);
        assert_eq!(gallery.order().len(), 3);

        let quick = |types: &[&str]| Filter { types: types.iter().map(|t| (*t).to_owned()).collect(), ..Filter::default() };
        gallery.set_filter(quick(&["JPG"]));
        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);
        assert_eq!((gallery.order().len(), gallery.match_count()), (0, Some(0)));
        gallery.set_filter(quick(&["PNG"]));
        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);
        assert_eq!((gallery.order().len(), gallery.match_count()), (3, Some(3)));
        gallery.clear_filter();
        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), false);

        show_once(&ctx, &mut thumbs, &mut gallery, &library, dir.path(), true);
        assert_eq!(gallery.order().len(), 3);
        assert!(gallery.layout().headers.is_empty());
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
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));

        let now = Instant::now();
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Remove(img.clone()), now);
        gallery.apply_change(&mut thumbs, &mut library, &FsChange::Upsert(img.clone()), now);
        assert_eq!(library.get(&img).unwrap().dims, None);
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn file_missing_from_a_rescan_is_probed_again_when_it_returns() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));

        gallery.replace_entries(&mut thumbs, &mut library, Vec::new());
        gallery.replace_entries(&mut thumbs, &mut library, crate::library::scan(dir.path(), false).unwrap());
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn discarded_result_does_not_pin_its_key_as_done() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&mut thumbs, &mut gallery, &library);
        while thumbs.backlog.is_empty() {
            thumbs.backlog.extend(thumbs.pipeline.try_iter());
            std::thread::sleep(Duration::from_millis(10));
        }

        library.apply(&FsChange::Remove(img.clone()), Instant::now());
        thumbs.poll(&ctx, &mut [(&mut library, &mut gallery)]);
        library.apply(&FsChange::Upsert(img.clone()), Instant::now());
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
    }

    #[test]
    fn retry_failed_gives_broken_files_another_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"not an image").unwrap();
        let (ctx, mut thumbs, mut gallery) = test_thumbs(dir.path());
        let mut library = library_of(dir.path());
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &bad, |e| e.error));

        gallery.retry_failed(&mut thumbs, &mut library);
        assert!(!library.get(&bad).unwrap().error);
        probe(&mut thumbs, &mut gallery, &library);
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &bad, |e| e.error));
    }

    #[test]
    fn refresh_renders_model_thumbnails_again_but_keeps_images() {
        let dir = tempfile::tempdir().unwrap();
        let (model, img) = (dir.path().join("m.obj"), dir.path().join("a.png"));
        std::fs::write(&model, b"not a model").unwrap();
        write_image(&img);
        let ctx = Context::default();
        let cache = DiskCache::open(dir.path().join("cache")).unwrap();
        let mut thumbs = Thumbs::with_cache(&ctx, cache.clone());
        let mut gallery = Gallery::new();
        let mut library = library_of(dir.path());
        let jobs = [&model, &img].map(|p| {
            let e = library.get(p).unwrap();
            ThumbJob { path: e.path.clone(), key: key_for(&e.path, e.mtime, e.size) }
        });
        let keys = jobs.clone().map(|j| j.key);
        let stale = crate::thumbs::Thumb { image: image::RgbaImage::new(2, 2), orig_dims: (40, 20), triangles: Some(1) };
        for key in keys {
            cache.put(key, &stale).unwrap();
        }
        thumbs.set_wanted(gallery.id, jobs.to_vec());
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &model, has_dims));
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &img, has_dims));
        assert!(thumbs.textures.contains(keys[0]) && thumbs.textures.contains(keys[1]));

        gallery.retry_failed(&mut thumbs, &mut library);
        assert!(!thumbs.textures.contains(keys[0]) && cache.get(keys[0]).is_none());
        assert!(thumbs.textures.contains(keys[1]) && cache.get(keys[1]).is_some());

        thumbs.set_wanted(gallery.id, jobs.to_vec());
        assert!(settles(&ctx, &mut thumbs, &mut gallery, &mut library, &model, |e| e.error));
    }

    #[test]
    fn result_goes_to_the_tab_that_lists_the_file() {
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let img = dir_a.path().join("a.png");
        write_image(&img);
        let (ctx, mut thumbs, mut gallery_a) = test_thumbs(dir_a.path());
        let (mut lib_a, mut lib_b) = (library_of(dir_a.path()), library_of(dir_b.path()));
        let mut gallery_b = Gallery::new();
        probe(&mut thumbs, &mut gallery_a, &lib_a);

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
        probe(&mut thumbs, &mut gallery_a, &lib_a);
        while thumbs.backlog.is_empty() {
            thumbs.backlog.extend(thumbs.pipeline.try_iter());
            std::thread::sleep(Duration::from_millis(10));
        }

        thumbs.poll(&ctx, &mut [(&mut lib_b, &mut gallery_b)]);
        assert!(thumbs.backlog.is_empty());
        assert_eq!(lib_a.get(&img).unwrap().dims, None);
        probe(&mut thumbs, &mut gallery_a, &lib_a);
        assert!(settles(&ctx, &mut thumbs, &mut gallery_a, &mut lib_a, &img, has_dims));
    }

    #[test]
    fn collapse_key_names_root_dot() {
        assert_eq!(collapse_key(""), ".");
        assert_eq!(collapse_key("Sub/Deep"), "sub/deep");
    }
}
