use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{self, Context, Frame, Key, Margin, Modifiers};
use image::RgbaImage;

use crate::config::{Config, FolderSettings, MAX_SIZE, MIN_SIZE};
use crate::formats::{self, DecodeError};
use crate::instance::Primary;
use crate::layout::{Dir, LayoutKind};
use crate::platform;
use crate::tab::{FolderIssue, OpenTarget, Tab, WindowState, absolute, restore_plan, wake};
use crate::ui::gallery::{self, Thumbs};
use crate::ui::preview_view;
use crate::ui::status::{self, StatusInfo};
use crate::ui::tabbar::{self, TabBarEvents};
use crate::ui::toolbar;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const SIZE_STEP: f32 = 16.0;
const TOAST_DURATION: Duration = Duration::from_secs(2);
const APP_NAME: &str = "Image Moz";

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

#[derive(Debug, PartialEq, Eq)]
enum TabShortcut {
    New,
    CloseActive,
    Cycle(isize),
}

/// A key press that is not an OS auto-repeat.
fn fresh_press(input: &egui::InputState, key: Key) -> bool {
    input
        .events
        .iter()
        .any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, repeat: false, .. } if *k == key))
}

fn tab_shortcut(i: &mut egui::InputState) -> Option<TabShortcut> {
    let cmd = Modifiers::COMMAND;
    let (fresh_t, fresh_w) = (fresh_press(i, Key::T), fresh_press(i, Key::W));
    // consume_key ignores an extra Shift, so the Shift variant has to be tried first.
    if i.consume_key(cmd | Modifiers::SHIFT, Key::Tab) {
        Some(TabShortcut::Cycle(-1))
    } else if i.consume_key(cmd, Key::Tab) {
        Some(TabShortcut::Cycle(1))
    } else if i.consume_key(cmd, Key::T) && fresh_t {
        Some(TabShortcut::New)
    } else if i.consume_key(cmd, Key::W) && fresh_w {
        Some(TabShortcut::CloseActive)
    } else {
        None
    }
}

pub struct ImageMozApp {
    config: Config,
    config_path: PathBuf,
    config_dirty: bool,
    last_save: Option<Instant>,
    thumbs: Thumbs,
    window: WindowState,
    title: String,
    open_requested: bool,
    toast: Option<(String, Instant)>,
    pending_copy: Option<PendingCopy>,
    instance: Option<Primary>,
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
    fonts.font_data.insert("symbols".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("symbols".into());
    }
    ctx.set_fonts(fonts);
}

fn window_title(root: Option<&Path>) -> String {
    match root {
        Some(root) => format!("{APP_NAME} — {}", root.display()),
        None => APP_NAME.to_string(),
    }
}

impl ImageMozApp {
    pub fn new(cc: &eframe::CreationContext, cli: Option<OpenTarget>, instance: Option<Primary>) -> Self {
        // Ctrl+= / Ctrl+- resize the thumbnails here, not the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        install_symbol_font(&cc.egui_ctx);
        if let Some(instance) = &instance {
            instance.set_wake(wake(&cc.egui_ctx));
        }
        let config_path = Config::default_path();
        let mut app = Self {
            config: Config::load(&config_path),
            config_path,
            config_dirty: false,
            last_save: None,
            thumbs: Thumbs::new(&cc.egui_ctx),
            window: WindowState::default(),
            title: String::new(),
            open_requested: false,
            toast: None,
            pending_copy: None,
            instance,
        };
        app.restore_session(&cc.egui_ctx);
        if let Some(target) = cli {
            app.open_target(&cc.egui_ctx, target);
        }
        app
    }

    fn restore_session(&mut self, ctx: &Context) {
        let stored = self.config.session.windows.first().cloned().unwrap_or_default();
        let (folders, active) = restore_plan(&stored, Path::is_dir);
        for folder in folders {
            self.window.open_folder(ctx, &self.config, folder);
        }
        self.window.activate(active);
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

    fn sync_session(&mut self) {
        let session = self.window.session();
        if self.config.session.windows.first() != Some(&session) {
            self.config.session.windows = vec![session];
            self.touch_config();
        }
    }

    /// Opens the folder in a new tab, or shows the one that already has it.
    fn open_target(&mut self, ctx: &Context, target: OpenTarget) {
        let index = self.window.open_folder(ctx, &self.config, target.folder);
        if let Some(path) = target.select {
            self.window.tabs[index].select_when_listed(path);
        }
        ctx.request_repaint();
    }

    fn open_path(&mut self, ctx: &Context, path: &Path) {
        if let Some(target) = OpenTarget::from_path(path) {
            self.open_target(ctx, target);
        }
    }

    fn close_tab(&mut self, ctx: &Context, index: usize) {
        self.window.close(index);
        if self.window.tabs.is_empty() {
            self.thumbs.cancel_requests();
        }
        ctx.request_repaint();
    }

    /// Opens what another launch forwarded and brings the window forward.
    fn handle_forwarded(&mut self, ctx: &Context) {
        let Some(instance) = &self.instance else { return };
        let forwarded = instance.try_iter();
        for path in forwarded {
            log::debug!("forwarded from another launch: {path:?}");
            if let Some(path) = path {
                self.open_path(ctx, &path);
            }
            if ctx.input(|i| i.viewport().minimized) == Some(true) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Collects scan results and watcher events of every tab, then thumbnail results.
    fn pump(&mut self, ctx: &Context) {
        for tab in &mut self.window.tabs {
            tab.pump(ctx, &mut self.thumbs);
        }
        let mut owners: Vec<_> = self.window.tabs.iter_mut().map(|t| (&mut t.library, &mut t.gallery)).collect();
        self.thumbs.poll(ctx, &mut owners);
    }

    fn handle_input(&mut self, ctx: &Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            self.open_path(ctx, &path);
        }

        match ctx.input_mut(tab_shortcut) {
            Some(TabShortcut::New) => self.open_requested = true,
            Some(TabShortcut::CloseActive) => self.close_tab(ctx, self.window.active),
            Some(TabShortcut::Cycle(delta)) => {
                self.window.cycle(delta);
                ctx.request_repaint();
            }
            None => {}
        }

        let typing = ctx.egui_wants_keyboard_input();
        let browsing = self.window.active_tab().is_some_and(|t| t.preview.is_none());
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
        if rescan && let Some(tab) = self.window.active_tab_mut() {
            tab.refresh(&mut self.thumbs);
        }
        self.handle_gallery_keys(ctx);
    }

    fn handle_gallery_keys(&mut self, ctx: &Context) {
        let Some(tab) = self.window.active_tab() else { return };
        if tab.preview.is_some() {
            self.handle_preview_keys(ctx);
            return;
        }
        if ctx.egui_wants_keyboard_input() || tab.library.is_empty() || tab.gallery.order().is_empty() {
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
            let fresh_space = fresh_press(i, Key::Space);
            if !m.shift && i.consume_key(m, Key::Space) && fresh_space {
                return Some(KeyAction::Open);
            }
            (!m.shift && i.key_pressed(Key::P)).then_some(KeyAction::Pin)
        });
        match action {
            Some(KeyAction::Move(dir, extend)) => {
                if let Some(tab) = self.window.active_tab_mut() {
                    tab.move_focus(dir, extend);
                }
            }
            Some(KeyAction::CopyFiles) => self.copy_files(ctx),
            Some(KeyAction::CopyPaths) => self.copy_paths(ctx),
            Some(KeyAction::Pin) => self.toggle_focus_pin(),
            Some(KeyAction::Open) => {
                if let Some(tab) = self.window.active_tab_mut() {
                    tab.open_preview(ctx);
                }
            }
            None => {}
        }
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
            let fresh_space = fresh_press(i, Key::Space);
            let space = i.consume_key(none, Key::Space) && fresh_space;
            if i.consume_key(none, Key::Escape) || space {
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
            Some(PreviewAction::CopyFiles) => self.copy_files(ctx),
            Some(PreviewAction::CopyPaths) => self.copy_paths(ctx),
            Some(action) => {
                let Some(tab) = self.window.active_tab_mut() else { return };
                match action {
                    PreviewAction::Close => tab.preview = None,
                    PreviewAction::Step(delta) => tab.step_preview(ctx, delta),
                    PreviewAction::Refit => {
                        if let Some(preview) = &mut tab.preview {
                            preview.fitted = true;
                        }
                    }
                    PreviewAction::CopyFiles | PreviewAction::CopyPaths => {}
                }
            }
            None => {}
        }
    }

    fn toggle_focus_pin(&mut self) {
        let Some(tab) = self.window.active_tab() else { return };
        let Some(path) = tab.focus_id().map(|i| tab.gallery.order()[i].clone()) else { return };
        if tab.library.get(&path).is_some() && path.exists() {
            self.config.toggle_pin(&tab.root, &path);
            self.touch_config();
        }
    }

    fn notify(&mut self, ctx: &Context, text: String) {
        self.toast = Some((text, Instant::now()));
        ctx.request_repaint_after(TOAST_DURATION);
    }

    fn copy_files(&mut self, ctx: &Context) {
        let Some(tab) = self.window.active_tab() else { return };
        let files = tab.selected_paths();
        if files.is_empty() {
            return;
        }
        let focus = match &tab.preview {
            Some(preview) => {
                if let Some(img) = preview.decoded_for(&preview.path) {
                    self.finish_copy(ctx, files, Some((*img).clone()));
                    return;
                }
                Some(preview.path.clone())
            }
            None => tab.focus_id().map(|i| tab.gallery.order()[i].clone()),
        };
        let Some(focus) = focus else {
            self.finish_copy(ctx, files, None);
            return;
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wake = wake(ctx);
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
        let paths = self.window.active_tab().map(Tab::selected_paths).unwrap_or_default();
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

    fn pick_folder(&mut self, ctx: &Context) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(tab) = self.window.active_tab().filter(|t| t.root.is_dir()) {
            dialog = dialog.set_directory(&tab.root);
        }
        if let Some(folder) = dialog.pick_folder() {
            self.open_target(ctx, OpenTarget { folder, select: None });
        }
    }

    fn sync_title(&mut self, ctx: &Context) {
        let title = window_title(self.window.active_tab().map(|t| t.root.as_path()));
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Applies toolbar edits of the folder settings to the active tab and the config.
    fn apply_folder_settings(&mut self, ctx: &Context, settings: FolderSettings) {
        let Some(tab) = self.window.active_tab_mut() else { return };
        if settings.recursive != tab.recursive {
            tab.reopen(ctx, &mut self.thumbs, settings);
        }
        tab.sort = settings.sort;
        if self.config.set_folder_settings(&tab.root, settings) {
            self.config_dirty = true;
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let Some(tab) = self.window.tabs.get_mut(self.window.active) else {
            self.open_requested |= message(ui, "Aucun dossier ouvert", true);
            return;
        };
        if let Some(issue) = tab.folder_issue {
            let what = match issue {
                FolderIssue::Missing => "Dossier introuvable",
                FolderIssue::Unreadable => "Dossier inaccessible",
            };
            self.open_requested |= message(ui, &format!("{what} : {}", tab.root.display()), true);
            return;
        }
        if tab.library.is_empty() {
            tab.selection.clear();
            tab.order_snapshot.1.clear();
            let text = if tab.is_scanning() { "Chargement…" } else { "Aucune image dans ce dossier" };
            message(ui, text, false);
            return;
        }
        let view = gallery::View {
            library: &tab.library,
            thumbs: &mut self.thumbs,
            config: &mut self.config,
            sort: tab.sort,
            recursive: tab.recursive,
            root: &tab.root,
            selection: &tab.selection,
            scroll_to: tab.scroll_to.take(),
        };
        let out = tab.gallery.show(ui, view);
        if out.hovered != tab.hovered {
            tab.hovered = out.hovered.clone();
            ui.ctx().request_repaint();
        }
        tab.remap_selection(ui.ctx());
        tab.resolve_pending_select(ui.ctx());
        apply_tile_events(tab, &out);
        if out.config_changed {
            self.config_dirty = true;
        }
    }
}

fn apply_tile_events(tab: &mut Tab, out: &gallery::Output) {
    if let Some(c) = &out.clicked {
        if c.modifiers.command {
            tab.selection.ctrl_click(c.id);
        } else if c.modifiers.shift {
            tab.selection.shift_click(c.id);
        } else {
            tab.selection.click(c.id);
        }
    }
    if let Some(id) = out.double_clicked {
        tab.selection.click(id);
        if let Some(path) = tab.gallery.order().get(id) {
            platform::open_default(&absolute(path));
        }
    }
    if let Some(id) = out.right_clicked
        && !tab.selection.contains(id)
    {
        tab.selection.click(id);
    }
    if let Some(id) = out.reveal
        && let Some(path) = tab.gallery.order().get(id)
    {
        platform::reveal_in_explorer(&absolute(path));
    }
}

/// A centered heading; returns true when its "Ouvrir" button was clicked.
fn message(ui: &mut egui::Ui, text: &str, with_open: bool) -> bool {
    let mut open = false;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 3.0).max(0.0));
        ui.heading(text);
        if with_open {
            ui.add_space(12.0);
            open = ui.button("Ouvrir").clicked();
        }
    });
    open
}

impl eframe::App for ImageMozApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_forwarded(&ctx);
        self.handle_input(&ctx);
        self.pump(&ctx);
        self.poll_copy(&ctx);
        self.sync_title(&ctx);
        if let Some(tab) = self.window.active_tab_mut() {
            tab.sync_preview();
        }

        let tab_events = if self.window.tabs.is_empty() {
            TabBarEvents::default()
        } else {
            egui::Panel::top("tabs")
                .show(ui, |ui| tabbar::show(ui, &self.window.tabs, self.window.active))
                .inner
        };
        let previewing = self.window.active_tab().is_some_and(|t| t.preview.is_some());
        let mut settings = self
            .window
            .active_tab()
            .map_or_else(FolderSettings::default, |t| FolderSettings { recursive: t.recursive, sort: t.sort });
        let has_tab = self.window.active_tab().is_some();
        let events = if previewing {
            toolbar::ToolbarEvents::default()
        } else {
            egui::Panel::top("toolbar")
                .show(ui, |ui| toolbar::show(ui, &mut self.config, &mut settings, has_tab))
                .inner
        };
        if let Some((_, at)) = &self.toast {
            let age = at.elapsed();
            if age >= TOAST_DURATION && self.pending_copy.is_none() {
                self.toast = None;
            } else {
                ctx.request_repaint_after(TOAST_DURATION.saturating_sub(age).max(Duration::from_millis(50)));
            }
        }
        let tab = self.window.active_tab();
        let entry = tab.and_then(|tab| {
            tab.preview
                .as_ref()
                .map(|p| p.path.as_path())
                .or(tab.hovered.as_deref())
                .or_else(|| tab.selection.focus().and_then(|i| tab.gallery.order().get(i)).map(PathBuf::as_path))
                .and_then(|p| tab.library.get(p))
        });
        let info = StatusInfo {
            entry,
            recursive: tab.is_some_and(|t| t.recursive),
            total: tab.map_or(0, |t| t.library.len()),
            selected: tab.map_or(0, |t| t.selection.len()),
            loading: tab.is_some_and(Tab::is_scanning),
            message: self.toast.as_ref().map(|(t, _)| t.as_str()),
        };
        egui::Panel::bottom("status").show(ui, |ui| status::show(ui, &info));
        if previewing {
            if let Some(tab) = self.window.tabs.get_mut(self.window.active) {
                let thumb = tab.preview.as_ref().and_then(|p| tab.gallery.thumbnail(&mut self.thumbs, p.index));
                let frame = Frame::NONE.fill(preview_view::BACKGROUND);
                egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                    if let Some(preview) = &mut tab.preview {
                        preview.poll(&ctx);
                        preview_view::show(ui, preview, thumb.as_ref());
                    }
                });
            }
        } else {
            let frame = Frame::NONE.fill(ctx.global_style().visuals.panel_fill).inner_margin(Margin::same(6));
            egui::CentralPanel::default().frame(frame).show(ui, |ui| self.central(ui));
        }

        if events.config_changed {
            self.touch_config();
        }
        // Before the tab events: the toolbar edited the tab that was active when it was drawn.
        if has_tab {
            self.apply_folder_settings(&ctx, settings);
        }
        if let Some(index) = tab_events.activate {
            self.window.activate(index);
            ctx.request_repaint();
        }
        if let Some(index) = tab_events.close {
            self.close_tab(&ctx, index);
        }
        if events.open || std::mem::take(&mut self.open_requested) {
            self.pick_folder(&ctx);
        }
        self.sync_session();
        self.flush_config(&ctx);
    }

    fn on_exit(&mut self) {
        self.sync_session();
        if self.config_dirty {
            self.save_config();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space_down() -> egui::Event {
        egui::Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// egui recomputes `repeat` from the keys already held, so it takes two passes to get one.
    fn fresh_space_per_pass(passes: usize) -> Vec<bool> {
        let ctx = Context::default();
        (0..passes)
            .map(|_| {
                let raw = egui::RawInput { events: vec![space_down()], ..Default::default() };
                let mut fresh = false;
                let _ = ctx.run_ui(raw, |ui| fresh = ui.input(|i| fresh_press(i, Key::Space)));
                fresh
            })
            .collect()
    }

    #[test]
    fn auto_repeat_is_not_a_fresh_press() {
        assert_eq!(fresh_space_per_pass(3), [true, false, false]);
    }

    /// What the shortcut parser makes of one key press with Ctrl (and optionally Shift) held.
    fn shortcut_for(key: Key, shift: bool, plain: bool) -> Option<TabShortcut> {
        let modifiers = if plain {
            Modifiers::NONE
        } else {
            Modifiers { ctrl: true, command: true, shift, ..Modifiers::NONE }
        };
        let event = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let raw = egui::RawInput { events: vec![event], modifiers, ..Default::default() };
        let mut found = None;
        let _ = Context::default().run_ui(raw, |ui| found = ui.input_mut(tab_shortcut));
        found
    }

    #[test]
    fn tab_shortcuts_need_ctrl_and_tell_shift_apart() {
        assert_eq!(shortcut_for(Key::Tab, false, false), Some(TabShortcut::Cycle(1)));
        assert_eq!(shortcut_for(Key::Tab, true, false), Some(TabShortcut::Cycle(-1)));
        assert_eq!(shortcut_for(Key::T, false, false), Some(TabShortcut::New));
        assert_eq!(shortcut_for(Key::W, false, false), Some(TabShortcut::CloseActive));
        for key in [Key::Tab, Key::T, Key::W, Key::Num1, Key::P, Key::Space] {
            assert_eq!(shortcut_for(key, false, true), None, "{key:?} without Ctrl");
        }
        assert_eq!(shortcut_for(Key::P, false, false), None);
    }

    #[test]
    fn title_names_the_app_and_the_active_folder() {
        assert_eq!(window_title(None), "Image Moz");
        assert_eq!(window_title(Some(Path::new(r"D:\Pics"))), "Image Moz — D:\\Pics");
    }
}
