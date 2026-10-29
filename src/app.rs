use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{self, Context, Frame, Key, Margin};

use crate::config::{Config, MAX_SIZE, MIN_SIZE};
use crate::layout::{Layout, LayoutKind};
use crate::library::{self, Entry, FsChange, Library};
use crate::selection::Selection;
use crate::ui::gallery::{self, Gallery};
use crate::ui::status::{self, StatusInfo};
use crate::ui::toolbar;
use crate::watcher::FolderWatcher;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const SIZE_STEP: f32 = 16.0;

type ScanResult = io::Result<Vec<Entry>>;

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
        let (layout, rescan, zoom) = ctx.input(|i| {
            let plain = !i.modifiers.any();
            let layout = if typing || !plain {
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
            if i.modifiers.command {
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
            let text = if self.scan.is_some() { "Chargement…" } else { "Aucune image dans ce dossier" };
            self.message(ui, text);
            return;
        }
        let view = gallery::View {
            library: &self.library,
            config: &mut self.config,
            root: &root,
            selection: &self.selection,
        };
        let out = self.gallery.show(ui, view);
        if out.hovered != self.hovered {
            self.hovered = out.hovered;
            ui.ctx().request_repaint();
        }
        if out.config_changed {
            self.touch_config();
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
        self.sync_title(&ctx);

        let events = egui::Panel::top("toolbar")
            .show(ui, |ui| toolbar::show(ui, &mut self.config))
            .inner;
        let entry = self
            .hovered
            .as_deref()
            .or_else(|| self.selection.focus().and_then(|i| self.gallery.order().get(i)).map(PathBuf::as_path))
            .and_then(|p| self.library.get(p));
        let info = StatusInfo {
            entry,
            recursive: self.config.recursive,
            total: self.library.len(),
            selected: self.selection.len(),
            loading: self.scan.is_some(),
        };
        egui::Panel::bottom("status").show(ui, |ui| status::show(ui, &info));
        let frame = Frame::NONE.fill(ctx.global_style().visuals.panel_fill).inner_margin(Margin::same(6));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| self.central(ui));

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
