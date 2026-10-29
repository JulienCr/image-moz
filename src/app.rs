use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{self, Context, Frame, Key, Margin};
use image::RgbaImage;

use crate::config::{Config, MAX_SIZE, MIN_SIZE};
use crate::formats::{self, DecodeError};
use crate::layout::{self, Dir, Layout, LayoutKind};
use crate::library::{self, Entry, FsChange, Library};
use crate::platform;
use crate::preview::PreviewState;
use crate::selection::Selection;
use crate::ui::gallery::{self, Gallery};
use crate::ui::preview_view;
use crate::ui::status::{self, StatusInfo};
use crate::ui::toolbar;
use crate::watcher::FolderWatcher;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const SIZE_STEP: f32 = 16.0;
const TOAST_DURATION: Duration = Duration::from_secs(2);

type ScanResult = io::Result<Vec<Entry>>;

struct PendingCopy {
    files: Vec<PathBuf>,
    decoded: Receiver<Result<RgbaImage, DecodeError>>,
}

enum KeyAction {
    Move(Dir, bool),
    CopyFiles,
    CopyPaths,
    Pin,
    Open,
}

enum PreviewAction {
    Close,
    Step(isize),
    Refit,
    CopyFiles,
    CopyPaths,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub struct ImageMozApp {
    config: Config,
    config_path: PathBuf,
    config_dirty: bool,
    last_save: Option<Instant>,
    root: Option<PathBuf>,
    library: Library,
    watcher: Option<FolderWatcher>,
    changes: Receiver<FsChange>,
    scan: Option<Receiver<ScanResult>>,
    scan_is_initial: bool,
    rescan_wanted: bool,
    /// Changes received while a scan runs; replayed on top of its result.
    deferred: Vec<FsChange>,
    folder_missing: bool,
    gallery: Gallery,
    pub selection: Selection,
    hovered: Option<PathBuf>,
    title: String,
    open_requested: bool,
    scroll_to: Option<usize>,
    toast: Option<(String, Instant)>,
    pending_copy: Option<PendingCopy>,
    /// Order the selection positions refer to, with the gallery version it came from.
    order_snapshot: (u64, Vec<PathBuf>),
    preview: Option<PreviewState>,
}

/// The bundled fonts lack arrows (↓ ↑ →); Segoe UI Symbol covers them on Windows.
fn install_symbol_font(ctx: &Context) {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let path = PathBuf::from(windir).join("Fonts").join("seguisym.ttf");
    let Ok(bytes) = std::fs::read(&path) else {
        log::warn!("symbol font not found at {}", path.display());
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("symbols".into(), Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("symbols".into());
    }
    ctx.set_fonts(fonts);
}

fn window_title(root: Option<&Path>) -> String {
    match root {
        Some(root) => format!("image-moz — {}", root.display()),
        None => "image-moz".to_string(),
    }
}

impl ImageMozApp {
    pub fn new(cc: &eframe::CreationContext, cli_folder: Option<PathBuf>) -> Self {
        // Ctrl+= / Ctrl+- resize the thumbnails here, not the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        install_symbol_font(&cc.egui_ctx);
        let config_path = Config::default_path();
        let config = Config::load(&config_path);
        let folder = cli_folder.or_else(|| config.last_folder.clone());
        let (_, changes) = crossbeam_channel::unbounded();
        let mut app = Self {
            library: Library::new(PathBuf::new(), config.recursive),
            config,
            config_path,
            config_dirty: false,
            last_save: None,
            root: None,
            watcher: None,
            changes,
            scan: None,
            scan_is_initial: false,
            rescan_wanted: false,
            deferred: Vec::new(),
            folder_missing: false,
            gallery: Gallery::new(&cc.egui_ctx),
            selection: Selection::default(),
            hovered: None,
            title: String::new(),
            open_requested: false,
            scroll_to: None,
            toast: None,
            pending_copy: None,
            order_snapshot: (0, Vec::new()),
            preview: None,
        };
        if let Some(folder) = folder {
            app.open_folder(&cc.egui_ctx, folder);
        }
        app
    }

    /// Tile paths in display order; selection positions index into this slice.
    pub fn visible_order(&self) -> &[PathBuf] {
        self.gallery.order()
    }

    pub fn layout(&self) -> &Layout {
        self.gallery.layout()
    }

    fn wake(ctx: &Context) -> Arc<dyn Fn() + Send + Sync> {
        let ctx = ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    fn touch_config(&mut self) {
        self.config_dirty = true;
    }

    fn save_config(&mut self) {
        self.config_dirty = false;
        self.last_save = Some(Instant::now());
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("cannot save config {}: {e}", self.config_path.display());
        }
    }

    /// Saves at most once per `SAVE_INTERVAL`; the exit path saves unconditionally.
    fn flush_config(&mut self, ctx: &Context) {
        if !self.config_dirty {
            return;
        }
        let wait = self
            .last_save
            .map_or(Duration::ZERO, |t| SAVE_INTERVAL.saturating_sub(t.elapsed()));
        if wait.is_zero() {
            self.save_config();
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    /// (Re)opens `folder` with the current recursive setting: new library, watcher and scan.
    fn open_folder(&mut self, ctx: &Context, folder: PathBuf) {
        let recursive = self.config.recursive;
        self.library = Library::new(folder.clone(), recursive);
        self.selection.clear();
        self.order_snapshot.1.clear();
        self.pending_copy = None;
        self.preview = None;
        self.hovered = None;
        self.deferred.clear();
        self.rescan_wanted = false;
        self.folder_missing = false;
        self.gallery.reset();

        let (tx, rx) = crossbeam_channel::unbounded();
        self.changes = rx;
        self.watcher = None;
        match FolderWatcher::start(&folder, recursive, tx, Self::wake(ctx)) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(e) => log::warn!("cannot watch {}: {e}", folder.display()),
        }
        self.scan_is_initial = true;
        self.start_scan(ctx, &folder);

        if self.config.last_folder.as_deref() != Some(folder.as_path()) {
            self.config.last_folder = Some(folder.clone());
            self.touch_config();
        }
        self.root = Some(folder);
    }

    fn start_scan(&mut self, ctx: &Context, folder: &Path) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (folder, recursive, wake) = (folder.to_path_buf(), self.config.recursive, Self::wake(ctx));
        std::thread::spawn(move || {
            let _ = tx.send(library::scan(&folder, recursive));
            wake();
        });
        self.scan = Some(rx);
    }

    /// Swaps in a scan result, keeping dims, error and highlight of unchanged files.
    fn apply_scan(&mut self, result: ScanResult) {
        match result {
            Err(e) => {
                log::warn!("scan failed: {e}");
                if self.scan_is_initial || e.kind() == io::ErrorKind::NotFound {
                    self.library.replace_all(Vec::new());
                    self.folder_missing = true;
                }
            }
            Ok(mut entries) => {
                self.folder_missing = false;
                let now = Instant::now();
                for entry in &mut entries {
                    match self.library.get(&entry.path) {
                        Some(old) if old.mtime == entry.mtime && old.size == entry.size => {
                            entry.dims = old.dims;
                            entry.error = old.error;
                            entry.arrived_at = old.arrived_at;
                        }
                        Some(_) => {}
                        None if !self.scan_is_initial => entry.arrived_at = Some(now),
                        None => {}
                    }
                }
                self.library.replace_all(entries);
            }
        }
        log::debug!("scan applied: {} entries", self.library.len());
        self.scan_is_initial = false;
        self.gallery.library_changed();
        for change in std::mem::take(&mut self.deferred) {
            self.apply_change(&change);
        }
    }

    fn apply_change(&mut self, change: &FsChange) {
        log::debug!("fs change: {change:?}");
        if let FsChange::Upsert(path) = change
            && let Some(old) = self.library.get(path)
        {
            self.gallery.forget_entry(old);
        }
        self.library.apply(change, Instant::now());
        self.gallery.library_changed();
    }

    /// Collects scan results and watcher events; at most one scan runs at a time.
    fn pump(&mut self, ctx: &Context) {
        if let Some(scan) = &self.scan {
            match scan.try_recv() {
                Ok(result) => {
                    self.scan = None;
                    self.apply_scan(result);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    log::error!("scan thread died without a result");
                    self.scan = None;
                }
            }
        }
        let changes: Vec<FsChange> = self.changes.try_iter().collect();
        for change in changes {
            match change {
                FsChange::Rescan => self.rescan_wanted = true,
                change if self.scan.is_some() => self.deferred.push(change),
                change => self.apply_change(&change),
            }
        }
        if self.rescan_wanted
            && self.scan.is_none()
            && let Some(root) = self.root.clone()
        {
            self.rescan_wanted = false;
            self.start_scan(ctx, &root);
        }
    }

    fn handle_input(&mut self, ctx: &Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(path) = dropped {
            let folder = if path.is_dir() { Some(path) } else { path.parent().map(Path::to_path_buf) };
            if let Some(folder) = folder {
                self.open_folder(ctx, folder);
            }
        }

        let typing = ctx.egui_wants_keyboard_input();
        let browsing = self.preview.is_none();
        let (layout, rescan, zoom) = ctx.input(|i| {
            let plain = !i.modifiers.any();
            let layout = if typing || !plain || !browsing {
                None
            } else if i.key_pressed(Key::Num1) {
                Some(LayoutKind::Grid)
            } else if i.key_pressed(Key::Num2) {
                Some(LayoutKind::Justified)
            } else if i.key_pressed(Key::Num3) {
                Some(LayoutKind::Bento)
            } else {
                None
            };
            let mut zoom = 0;
            if browsing && i.modifiers.command {
                if i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus) {
                    zoom += 1;
                }
                if i.key_pressed(Key::Minus) {
                    zoom -= 1;
                }
            }
            // Raw events, not zoom_delta(): egui smooths the wheel over several frames.
            for event in &i.events {
                if let egui::Event::MouseWheel { delta, modifiers, .. } = event
                    && browsing
                    && modifiers.command
                    && delta.y != 0.0
                {
                    zoom += delta.y.signum() as i32;
                }
            }
            (layout, i.key_pressed(Key::F5), zoom)
        });
        if let Some(kind) = layout
            && kind != self.config.layout
        {
            self.config.layout = kind;
            self.touch_config();
        }
        if zoom != 0 {
            let size = (self.config.target_size + zoom as f32 * SIZE_STEP).clamp(MIN_SIZE, MAX_SIZE);
            if size != self.config.target_size {
                self.config.target_size = size;
                self.touch_config();
            }
        }
        if rescan {
            self.rescan_wanted = true;
        }
        self.handle_gallery_keys(ctx);
    }

    fn handle_gallery_keys(&mut self, ctx: &Context) {
        if self.preview.is_some() {
            self.handle_preview_keys(ctx);
            return;
        }
        if ctx.egui_wants_keyboard_input() || self.library.is_empty() || self.gallery.order().is_empty() {
            return;
        }
        let action = ctx.input_mut(|i| {
            let m = i.modifiers;
            if m.command {
                // egui-winit turns Ctrl+C into Event::Copy and emits no key event for it.
                let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if m.shift || !copy {
                    return None;
                }
                return Some(if m.alt { KeyAction::CopyPaths } else { KeyAction::CopyFiles });
            }
            if m.alt {
                return None;
            }
            for (key, dir) in [
                (Key::ArrowLeft, Dir::Left),
                (Key::ArrowRight, Dir::Right),
                (Key::ArrowUp, Dir::Up),
                (Key::ArrowDown, Dir::Down),
            ] {
                if i.key_pressed(key) {
                    i.consume_key(m, key);
                    return Some(KeyAction::Move(dir, m.shift));
                }
            }
            if !m.shift && i.consume_key(m, Key::Space) {
                return Some(KeyAction::Open);
            }
            (!m.shift && i.key_pressed(Key::P)).then_some(KeyAction::Pin)
        });
        match action {
            Some(KeyAction::Move(dir, extend)) => self.move_focus(dir, extend),
            Some(KeyAction::CopyFiles) => self.copy_files(ctx),
            Some(KeyAction::CopyPaths) => self.copy_paths(ctx),
            Some(KeyAction::Pin) => self.toggle_focus_pin(),
            Some(KeyAction::Open) => self.open_preview(ctx),
            None => {}
        }
    }

    fn open_preview(&mut self, ctx: &Context) {
        let Some(id) = self.focus_id() else { return };
        self.show_preview(ctx, id);
    }

    /// Opens the preview on tile `id`, or retargets an open one.
    fn show_preview(&mut self, ctx: &Context, id: usize) {
        let Some(path) = self.gallery.order().get(id).cloned() else { return };
        let hint = self
            .library
            .get(&path)
            .and_then(|e| e.dims)
            .map(|(w, h)| egui::vec2(w as f32, h as f32));
        match &mut self.preview {
            Some(preview) => preview.show(ctx, id, path, hint),
            None => self.preview = Some(PreviewState::open(ctx, id, path, hint)),
        }
        self.selection.move_to(id, false);
        self.scroll_to = Some(id);
    }

    fn handle_preview_keys(&mut self, ctx: &Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let action = ctx.input_mut(|i| {
            let m = i.modifiers;
            if m.command {
                let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if m.shift || !copy {
                    return None;
                }
                return Some(if m.alt { PreviewAction::CopyPaths } else { PreviewAction::CopyFiles });
            }
            if m.any() {
                return None;
            }
            let none = egui::Modifiers::NONE;
            if i.consume_key(none, Key::Escape) || i.consume_key(none, Key::Space) {
                Some(PreviewAction::Close)
            } else if i.consume_key(none, Key::ArrowLeft) {
                Some(PreviewAction::Step(-1))
            } else if i.consume_key(none, Key::ArrowRight) {
                Some(PreviewAction::Step(1))
            } else if i.key_pressed(Key::Num0) {
                Some(PreviewAction::Refit)
            } else {
                None
            }
        });
        match action {
            Some(PreviewAction::Close) => self.preview = None,
            Some(PreviewAction::Step(delta)) => self.step_preview(ctx, delta),
            Some(PreviewAction::Refit) => {
                if let Some(preview) = &mut self.preview {
                    preview.fitted = true;
                }
            }
            Some(PreviewAction::CopyFiles) => self.copy_files(ctx),
            Some(PreviewAction::CopyPaths) => self.copy_paths(ctx),
            None => {}
        }
    }

    fn step_preview(&mut self, ctx: &Context, delta: isize) {
        let Some(preview) = &self.preview else { return };
        let target = preview.index.checked_add_signed(delta).filter(|&i| i < self.gallery.order().len());
        if let Some(target) = target {
            self.show_preview(ctx, target);
        }
    }

    /// Keeps the preview on its file when the gallery order changes underneath it.
    fn sync_preview(&mut self) {
        let Some(preview) = &mut self.preview else { return };
        if self.library.get(&preview.path).is_none() {
            self.preview = None;
            return;
        }
        let order = self.gallery.order();
        if order.get(preview.index) == Some(&preview.path) {
            return;
        }
        match order.iter().position(|p| *p == preview.path) {
            Some(i) => preview.index = i,
            None => self.preview = None,
        }
    }

    fn focus_id(&self) -> Option<usize> {
        self.selection.focus().filter(|&i| i < self.gallery.order().len())
    }

    fn move_focus(&mut self, dir: Dir, extend: bool) {
        let target = match self.focus_id() {
            Some(from) => layout::neighbor(self.gallery.layout(), from, dir),
            None => Some(0),
        };
        if let Some(target) = target {
            self.selection.move_to(target, extend);
            self.scroll_to = Some(target);
        }
    }

    fn toggle_focus_pin(&mut self) {
        let Some(root) = self.root.clone() else { return };
        let Some(path) = self.focus_id().map(|i| self.gallery.order()[i].clone()) else { return };
        if self.library.get(&path).is_some() && path.exists() {
            self.config.toggle_pin(&root, &path);
            self.touch_config();
        }
    }

    /// Absolute paths of the selection, or of the focused tile when nothing is selected.
    fn selected_paths(&self) -> Vec<PathBuf> {
        if let Some(preview) = &self.preview {
            return vec![absolute(&preview.path)];
        }
        let order = self.gallery.order();
        let mut paths: Vec<PathBuf> =
            self.selection.selected().filter_map(|i| order.get(i)).map(|p| absolute(p)).collect();
        if paths.is_empty()
            && let Some(i) = self.focus_id()
        {
            paths.push(absolute(&order[i]));
        }
        paths
    }

    fn notify(&mut self, ctx: &Context, text: String) {
        self.toast = Some((text, Instant::now()));
        ctx.request_repaint_after(TOAST_DURATION);
    }

    fn copy_files(&mut self, ctx: &Context) {
        let files = self.selected_paths();
        if files.is_empty() {
            return;
        }
        let focus = match &self.preview {
            Some(preview) => {
                if let Some(img) = preview.decoded_for(&preview.path) {
                    self.finish_copy(ctx, files, Some((*img).clone()));
                    return;
                }
                Some(preview.path.clone())
            }
            None => self.focus_id().map(|i| self.gallery.order()[i].clone()),
        };
        let Some(focus) = focus else {
            self.finish_copy(ctx, files, None);
            return;
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wake = Self::wake(ctx);
        std::thread::spawn(move || {
            let _ = tx.send(formats::decode(&focus));
            wake();
        });
        self.pending_copy = Some(PendingCopy { files, decoded: rx });
        self.notify(ctx, "Copie en cours…".to_string());
    }

    fn poll_copy(&mut self, ctx: &Context) {
        let Some(pending) = &self.pending_copy else { return };
        let result = match pending.decoded.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(DecodeError::Unsupported),
        };
        let Some(PendingCopy { files, .. }) = self.pending_copy.take() else { return };
        if let Err(e) = &result {
            log::warn!("cannot decode image for the clipboard: {e}");
        }
        self.finish_copy(ctx, files, result.ok());
    }

    fn finish_copy(&mut self, ctx: &Context, files: Vec<PathBuf>, bitmap: Option<RgbaImage>) {
        let text = match platform::copy_files_and_bitmap(&files, bitmap.as_ref()) {
            Ok(()) => format!("Copié : {} fichier(s)", files.len()),
            Err(e) => format!("Copie impossible : {e}"),
        };
        self.notify(ctx, text);
    }

    fn copy_paths(&mut self, ctx: &Context) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n");
        let message = match platform::copy_text(&text) {
            Ok(()) => "Chemins copiés".to_string(),
            Err(e) => format!("Copie impossible : {e}"),
        };
        self.notify(ctx, message);
    }

    /// Re-targets selection positions when the gallery order was rebuilt.
    fn remap_selection(&mut self, ctx: &Context) {
        let version = self.gallery.order_version();
        if version == self.order_snapshot.0 {
            return;
        }
        let new_order = self.gallery.order();
        if new_order != self.order_snapshot.1.as_slice() {
            let positions: HashMap<&PathBuf, usize> =
                new_order.iter().enumerate().map(|(i, p)| (p, i)).collect();
            let old = &self.order_snapshot.1;
            self.selection.remap(|i| old.get(i).and_then(|p| positions.get(p).copied()));
            ctx.request_repaint();
        }
        self.order_snapshot = (version, self.gallery.order().to_vec());
    }

    fn pick_folder(&mut self, ctx: &Context) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(root) = self.root.as_ref().filter(|r| r.is_dir()) {
            dialog = dialog.set_directory(root);
        }
        if let Some(folder) = dialog.pick_folder() {
            self.open_folder(ctx, folder);
        }
    }

    fn sync_title(&mut self, ctx: &Context) {
        let title = window_title(self.root.as_deref());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.root.clone() else {
            self.message(ui, "Aucun dossier ouvert");
            return;
        };
        if self.folder_missing {
            self.message(ui, &format!("Dossier introuvable : {}", root.display()));
            return;
        }
        if self.library.is_empty() {
            self.selection.clear();
            self.order_snapshot.1.clear();
            let text = if self.scan.is_some() { "Chargement…" } else { "Aucune image dans ce dossier" };
            self.message(ui, text);
            return;
        }
        let view = gallery::View {
            library: &self.library,
            config: &mut self.config,
            root: &root,
            selection: &self.selection,
            scroll_to: self.scroll_to.take(),
        };
        let out = self.gallery.show(ui, view);
        if out.hovered != self.hovered {
            self.hovered = out.hovered.clone();
            ui.ctx().request_repaint();
        }
        if out.config_changed {
            self.touch_config();
        }
        self.remap_selection(ui.ctx());
        self.apply_tile_events(&out);
    }

    fn apply_tile_events(&mut self, out: &gallery::Output) {
        if let Some(c) = &out.clicked {
            if c.modifiers.command {
                self.selection.ctrl_click(c.id);
            } else if c.modifiers.shift {
                self.selection.shift_click(c.id);
            } else {
                self.selection.click(c.id);
            }
        }
        if let Some(id) = out.double_clicked {
            self.selection.click(id);
            if let Some(path) = self.gallery.order().get(id) {
                platform::open_default(&absolute(path));
            }
        }
        if let Some(id) = out.right_clicked
            && !self.selection.contains(id)
        {
            self.selection.click(id);
        }
        if let Some(id) = out.reveal
            && let Some(path) = self.gallery.order().get(id)
        {
            platform::reveal_in_explorer(&absolute(path));
        }
    }

    fn message(&mut self, ui: &mut egui::Ui, text: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 3.0).max(0.0));
            ui.heading(text);
            if self.root.is_none() || self.folder_missing {
                ui.add_space(12.0);
                if ui.button("Ouvrir").clicked() {
                    self.open_requested = true;
                }
            }
        });
    }
}

impl eframe::App for ImageMozApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_input(&ctx);
        self.pump(&ctx);
        self.gallery.poll(&ctx, &mut self.library);
        self.poll_copy(&ctx);
        self.sync_title(&ctx);

        self.sync_preview();
        let previewing = self.preview.is_some();
        let events = if previewing {
            toolbar::ToolbarEvents::default()
        } else {
            egui::Panel::top("toolbar").show(ui, |ui| toolbar::show(ui, &mut self.config)).inner
        };
        let entry = self
            .preview
            .as_ref()
            .map(|p| p.path.as_path())
            .or(self.hovered.as_deref())
            .or_else(|| self.selection.focus().and_then(|i| self.gallery.order().get(i)).map(PathBuf::as_path))
            .and_then(|p| self.library.get(p));
        if let Some((_, at)) = &self.toast {
            let age = at.elapsed();
            if age >= TOAST_DURATION && self.pending_copy.is_none() {
                self.toast = None;
            } else {
                ctx.request_repaint_after(TOAST_DURATION.saturating_sub(age).max(Duration::from_millis(50)));
            }
        }
        let info = StatusInfo {
            entry,
            recursive: self.config.recursive,
            total: self.library.len(),
            selected: self.selection.len(),
            loading: self.scan.is_some(),
            message: self.toast.as_ref().map(|(t, _)| t.as_str()),
        };
        egui::Panel::bottom("status").show(ui, |ui| status::show(ui, &info));
        if previewing {
            let thumb = self.preview.as_ref().and_then(|p| self.gallery.thumbnail(p.index));
            let frame = Frame::NONE.fill(preview_view::BACKGROUND);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                if let Some(preview) = &mut self.preview {
                    preview.poll(&ctx);
                    preview_view::show(ui, preview, thumb.as_ref());
                }
            });
        } else {
            let frame = Frame::NONE.fill(ctx.global_style().visuals.panel_fill).inner_margin(Margin::same(6));
            egui::CentralPanel::default().frame(frame).show(ui, |ui| self.central(ui));
        }

        if events.config_changed {
            self.touch_config();
        }
        if events.recursive_changed
            && let Some(root) = self.root.clone()
        {
            self.open_folder(&ctx, root);
        }
        if events.open || std::mem::take(&mut self.open_requested) {
            self.pick_folder(&ctx);
        }
        self.flush_config(&ctx);
    }

    fn on_exit(&mut self) {
        if self.config_dirty {
            self.save_config();
        }
    }
}
