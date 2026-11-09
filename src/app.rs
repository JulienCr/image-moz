use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{
    self, Color32, Context, CornerRadius, Frame, IconData, Id, Key, Margin, Modifiers, Pos2, Rect, ViewportBuilder,
    ViewportCommand, ViewportId, pos2, vec2,
};
use image::RgbaImage;

use crate::config::{Config, FolderSettings, MAX_SIZE, MIN_SIZE, WindowBounds};
use crate::formats::{self, DecodeError};
use crate::geometry::{self, BarGeometry, Fitted, PxRect};
use crate::instance::Primary;
use crate::layout::{Dir, LayoutKind};
use crate::platform;
use crate::tab::{FolderIssue, OpenTarget, Tab, WindowState, absolute, wake};
use crate::ui::gallery::{self, Thumbs};
use crate::ui::preview_view;
use crate::ui::status::{self, StatusInfo};
use crate::ui::tabbar::{self, DragView, TabBarEvents};
use crate::ui::toolbar;
use crate::windows::{Change, Placement, ROOT, Step, WindowId, WindowPlan, Windows, session_plan};

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const SIZE_STEP: f32 = 16.0;
const TOAST_DURATION: Duration = Duration::from_secs(2);
const APP_NAME: &str = "Image Moz";
const DEFAULT_SIZE: (f32, f32) = (1200.0, 800.0);
const MIN_SIZE_PTS: [f32; 2] = [480.0, 320.0];
/// After a restore, the stored active window is raised once all windows exist, and focus changes
/// the OS makes while they open are not taken for the user's choice, until this much time has passed.
const RESTORE_RAISE_AT: Duration = Duration::from_millis(400);
const RESTORE_FOCUS_FOR: Duration = Duration::from_millis(1000);
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

struct PendingCopy {
    window: WindowId,
    files: Vec<PathBuf>,
    decoded: Receiver<Result<RgbaImage, DecodeError>>,
}

struct Toast {
    window: WindowId,
    text: String,
    at: Instant,
}

/// Where a released tab would land: a window and the slot among its tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DropTarget {
    window: WindowId,
    index: usize,
}

/// A tab being dragged. The window that started it keeps the mouse capture, so the
/// release is read from the global button state rather than from egui.
struct TabDrag {
    tab_id: u64,
    label: String,
    cursor_px: Pos2,
    target: Option<DropTarget>,
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
    windows: Windows,
    /// The window whose UI is being built; every per-window method acts on it.
    cur: WindowId,
    drag: Option<TabDrag>,
    icon: Option<Arc<IconData>>,
    /// Pixels per point a new window starts with, used to turn stored pixels into points.
    scale: f32,
    /// Pixels per point of the root window, for placing it after another window replaced it.
    root_scale: f32,
    /// Windows whose title bar close button was pressed this frame.
    closing: Vec<WindowId>,
    open_requested: bool,
    toast: Option<Toast>,
    pending_copy: Option<PendingCopy>,
    instance: Option<Primary>,
    published_target: isize,
    /// Window to bring forward after a restore, while the OS hands the focus to the last one created.
    restore_focus: Option<(WindowId, Instant)>,
    restore_raised: bool,
}

/// Everything decided before the first window exists: the session to restore and the root's placement.
pub struct Startup {
    config: Config,
    plans: Vec<WindowPlan>,
    active_window: usize,
    icon: Option<Arc<IconData>>,
    monitors: Vec<PxRect>,
    scale: f32,
    /// Where the root was asked to open, when its stored position is still on a monitor.
    root_placement: Option<WindowBounds>,
}

fn load_icon() -> Option<Arc<IconData>> {
    match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => Some(Arc::new(icon)),
        Err(e) => {
            log::warn!("cannot decode the window icon: {e}");
            None
        }
    }
}

/// Builder for a window; `fitted` is `None` for the default size and a system-chosen position.
fn window_builder(title: String, fitted: Option<Fitted>, scale: f32, icon: Option<Arc<IconData>>) -> ViewportBuilder {
    let (width, height) = fitted.map_or(DEFAULT_SIZE, |f| (f.size.0 as f32 / scale, f.size.1 as f32 / scale));
    let mut builder = ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([width, height])
        .with_min_inner_size(MIN_SIZE_PTS);
    if let Some((x, y)) = fitted.and_then(|f| f.position) {
        builder = builder.with_position([x as f32 / scale, y as f32 / scale]);
    }
    if let Some(icon) = icon {
        builder = builder.with_icon(icon);
    }
    builder
}

impl Startup {
    pub fn from_config(config: Config) -> Self {
        let (plans, active_window) = session_plan(&config.session, Path::is_dir);
        let (monitors, scale) = (platform::monitor_work_areas(), platform::system_scale());
        let root_fit = plans.first().and_then(|p| p.bounds).map(|b| geometry::fit_to_monitors(b, &monitors, scale));
        let root_placement = root_fit.and_then(|f| {
            let (x, y) = f.position?;
            Some(WindowBounds { x, y, width: f.size.0, height: f.size.1 })
        });
        Self { config, plans, active_window, icon: load_icon(), monitors, scale, root_placement }
    }

    /// The root viewport, placed where the first stored window was when it is still on a monitor.
    pub fn root_viewport(&self) -> ViewportBuilder {
        let stored = self.plans.first().and_then(|p| p.bounds);
        let fitted = stored.map(|b| geometry::fit_to_monitors(b, &self.monitors, self.scale));
        window_builder(APP_NAME.to_string(), fitted, self.scale, self.icon.clone())
    }
}

fn viewport_id(window: WindowId) -> ViewportId {
    if window == ROOT { ViewportId::ROOT } else { ViewportId::from_hash_of(("image-moz-window", window)) }
}

fn px_rect(rect: Rect, scale: f32) -> PxRect {
    let px = |v: f32| (v * scale).round() as i32;
    PxRect { left: px(rect.min.x), top: px(rect.min.y), right: px(rect.max.x), bottom: px(rect.max.y) }
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
    pub fn new(cc: &eframe::CreationContext, startup: Startup, cli: Option<OpenTarget>, instance: Option<Primary>) -> Self {
        // Ctrl+= / Ctrl+- resize the thumbnails here, not the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        install_symbol_font(&cc.egui_ctx);
        if let Some(instance) = &instance {
            instance.set_wake(wake(&cc.egui_ctx));
        }
        let Startup { config, plans, active_window, icon, monitors, scale, root_placement } = startup;
        let ctx = &cc.egui_ctx;
        let mut windows = Windows::default();
        let mut ids = Vec::new();
        for (i, plan) in plans.iter().enumerate() {
            let mut state = WindowState::default();
            for folder in &plan.folders {
                state.open_folder(ctx, &config, folder.clone());
            }
            state.activate(plan.active);
            let id = if i == 0 {
                *windows.state_mut(ROOT) = state;
                if let Some(window) = windows.get_mut(ROOT) {
                    window.placement = root_placement.map(Placement::new);
                }
                ROOT
            } else {
                let fitted = plan.bounds.map(|b| geometry::fit_to_monitors(b, &monitors, scale));
                windows.add(state, fitted)
            };
            if let Some(window) = windows.get_mut(id) {
                window.bounds = plan.bounds;
            }
            ids.push(id);
        }
        let restore_focus = ids.get(active_window).copied().filter(|_| ids.len() > 1);
        if let Some(id) = restore_focus {
            windows.touch(id);
        }
        let config_path = Config::default_path();
        let mut app = Self {
            config,
            config_path,
            config_dirty: false,
            last_save: None,
            thumbs: Thumbs::new(ctx),
            windows,
            cur: ROOT,
            drag: None,
            icon,
            scale,
            root_scale: scale,
            closing: Vec::new(),
            open_requested: false,
            toast: None,
            pending_copy: None,
            instance,
            published_target: 0,
            restore_focus: restore_focus.map(|id| (id, Instant::now())),
            restore_raised: false,
        };
        if let Some(target) = cli {
            let dest = app.windows.last_active();
            app.open_target(ctx, target, dest);
        }
        app
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
        let session = self.windows.session();
        if self.config.session != session {
            self.config.session = session;
            self.touch_config();
        }
    }

    /// Opens the folder in a new tab of `dest`, or shows the one that already has it in any window.
    /// Returns the window that now shows the folder.
    fn open_target(&mut self, ctx: &Context, target: OpenTarget, dest: WindowId) -> WindowId {
        let (window, index) = match self.windows.find_folder(&target.folder) {
            Some((window, index)) => {
                self.windows.state_mut(window).activate(index);
                (window, index)
            }
            None => (dest, self.windows.state_mut(dest).open_folder(ctx, &self.config, target.folder)),
        };
        if let Some(path) = target.select {
            self.windows.state_mut(window).tabs[index].select_when_listed(path);
        }
        if window != dest {
            self.raise(ctx, window);
        }
        ctx.request_repaint();
        window
    }

    fn open_path(&mut self, ctx: &Context, path: &Path) {
        if let Some(target) = OpenTarget::from_path(path) {
            self.open_target(ctx, target, self.cur);
        }
    }

    fn close_tab(&mut self, ctx: &Context, index: usize) {
        self.windows.state_mut(self.cur).close(index);
        ctx.request_repaint();
    }

    /// Brings a window to the front, restoring it when minimized.
    fn raise(&mut self, ctx: &Context, id: WindowId) {
        self.windows.touch(id);
        let Some(window) = self.windows.get(id) else { return };
        match window.hwnd {
            Some(hwnd) => platform::raise_window(hwnd),
            None => {
                let viewport = viewport_id(id);
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::Focus);
            }
        }
    }

    /// Opens what another launch forwarded in the window used last and brings it forward.
    fn handle_forwarded(&mut self, ctx: &Context) {
        let forwarded = self.instance.as_ref().map(Primary::try_iter).unwrap_or_default();
        for path in forwarded {
            log::debug!("forwarded from another launch: {path:?}");
            let dest = self.windows.last_active();
            let shown = match path.as_deref().and_then(OpenTarget::from_path) {
                Some(target) => self.open_target(ctx, target, dest),
                None => dest,
            };
            self.raise(ctx, shown);
        }
    }

    /// Tells the pipe thread which window a forwarded launch should raise.
    fn publish_target(&mut self) {
        let last = self.windows.last_active();
        let hwnd = self.windows.get(last).and_then(|w| w.hwnd).unwrap_or(0);
        if hwnd != self.published_target {
            self.published_target = hwnd;
            if let Some(instance) = &self.instance {
                instance.set_target_window(hwnd);
            }
        }
    }

    /// Collects scan results and watcher events of every tab of every window, then thumbnail results.
    fn pump(&mut self, ctx: &Context) {
        for tab in self.windows.iter_mut().flat_map(|w| w.state.tabs.iter_mut()) {
            tab.pump(ctx, &mut self.thumbs);
        }
        let mut owners: Vec<_> = self
            .windows
            .iter_mut()
            .flat_map(|w| w.state.tabs.iter_mut())
            .map(|t| (&mut t.library, &mut t.gallery))
            .collect();
        self.thumbs.poll(ctx, &mut owners);
    }

    /// Reads what the viewport reports about its window. Returns where the drawing area sits on screen,
    /// in physical pixels, and the pixels per point; `None` while minimized.
    fn observe_window(&mut self, ctx: &Context, id: WindowId) -> Option<(Pos2, f32)> {
        let info = ctx.input(|i| i.viewport().clone());
        let scale = ctx.pixels_per_point();
        if id == ROOT {
            self.root_scale = scale;
        }
        if info.focused == Some(true) {
            let held_by_restore = self.restore_focus.is_some_and(|(want, _)| want != id);
            if !held_by_restore {
                self.windows.touch(id);
            }
            if let Some(hwnd) = platform::foreground_own_window()
                && let Some(window) = self.windows.get_mut(id)
            {
                window.hwnd = Some(hwnd);
            }
        }
        if info.close_requested() && self.windows.len() > 1 {
            // Only the last window may close the app: a root closed while others live is handed over.
            if id == ROOT {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::CancelClose);
            }
            self.closing.push(id);
        }
        let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) else { return None };
        if info.minimized == Some(true) {
            return None;
        }
        let outer_px = px_rect(outer, scale);
        let taken: Vec<isize> = self.windows.iter().filter(|w| w.id != id).filter_map(|w| w.hwnd).collect();
        let window = self.windows.get_mut(id)?;
        let actual = WindowBounds {
            x: outer_px.left,
            y: outer_px.top,
            width: (inner.width() * scale).round().max(1.0) as u32,
            height: (inner.height() * scale).round().max(1.0) as u32,
        };
        window.bounds = Some(actual);
        let step = window.placement.as_mut().map(|p| (p.step(geometry::bounds_match(p.target, actual, 2)), p.target));
        match step {
            Some((Step::Wait, _)) => ctx.request_repaint_after(Duration::from_millis(150)),
            Some((Step::Correct, target)) => {
                log::debug!("placing window {id}: actual {actual:?} wanted {target:?} at scale {scale}");
                let viewport = viewport_id(id);
                let position = pos2(target.x as f32 / scale, target.y as f32 / scale);
                let size = vec2(target.width as f32 / scale, target.height as f32 / scale);
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::OuterPosition(position));
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::InnerSize(size));
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            Some((Step::Done, _)) => window.placement = None,
            None => {}
        }
        if window.hwnd.is_none() {
            window.hwnd = platform::find_own_window(outer_px, &taken);
        }
        Some((pos2(inner.min.x * scale, inner.min.y * scale), scale))
    }

    fn drop_target(&self, cursor: Pos2) -> Option<DropTarget> {
        let bars: Vec<(WindowId, BarGeometry)> =
            self.windows.iter().filter_map(|w| Some((w.id, w.geometry.clone()?))).collect();
        let top = platform::top_level_window_at(cursor.x as i32, cursor.y as i32);
        // A bar covered by another window is not a target; a window with an unknown handle is given the benefit of the doubt.
        let allowed = |id: WindowId| {
            let hwnd = self.windows.get(id).and_then(|w| w.hwnd);
            top.is_none_or(|t| hwnd.is_none_or(|h| h == t))
        };
        let (window, index) = geometry::drop_target(&bars, cursor, allowed)?;
        Some(DropTarget { window, index })
    }

    /// Follows the cursor of a running tab drag and applies it when the button comes up.
    fn update_drag(&mut self, ctx: &Context) {
        let Some(mut drag) = self.drag.take() else { return };
        log::trace!("drag frame");
        if let Some((x, y)) = platform::cursor_pos() {
            drag.cursor_px = pos2(x as f32, y as f32);
        }
        drag.target = self.drop_target(drag.cursor_px);
        if platform::primary_button_down() {
            // Nothing else wakes the other windows while the pointer is held in this one.
            ctx.request_repaint();
            self.drag = Some(drag);
        } else {
            self.finish_drag(ctx, &drag);
        }
    }

    fn finish_drag(&mut self, ctx: &Context, drag: &TabDrag) {
        log::debug!("tab drag released: tab {} target {:?}", drag.tab_id, drag.target);
        let Some((source, _)) = self.windows.locate_tab(drag.tab_id) else { return };
        match drag.target {
            Some(target) if target.window == source => self.windows.reorder(drag.tab_id, target.index),
            Some(target) => {
                let changes = self.windows.move_tab(drag.tab_id, target.window, target.index);
                self.apply_changes(ctx, changes);
                if let Some((holder, _)) = self.windows.locate_tab(drag.tab_id) {
                    self.raise(ctx, holder);
                }
            }
            None => self.detach(drag, source),
        }
        ctx.request_repaint();
    }

    /// Opens the dragged tab in a new window under the cursor, as large as the window it left.
    fn detach(&mut self, drag: &TabDrag, source: WindowId) {
        let from = self.windows.get(source);
        let scale = from.and_then(|w| w.geometry.as_ref()).map_or(self.scale, |g| g.scale);
        let default = ((DEFAULT_SIZE.0 * scale) as u32, (DEFAULT_SIZE.1 * scale) as u32);
        let size = from.and_then(|w| w.bounds).map_or(default, |b| (b.width, b.height));
        let wanted = geometry::detach_bounds((drag.cursor_px.x as i32, drag.cursor_px.y as i32), size, scale);
        let fitted = geometry::fit_to_monitors(wanted, &platform::monitor_work_areas(), scale);
        self.windows.detach(drag.tab_id, fitted);
    }

    /// Mirrors structural changes on the native windows. Closed viewports simply stop being shown.
    fn apply_changes(&mut self, ctx: &Context, changes: Vec<Change>) {
        for change in changes {
            let Change::Promoted(bounds) = change else { continue };
            if let Some(b) = bounds {
                let scale = self.root_scale.max(0.5);
                let position = pos2(b.x as f32 / scale, b.y as f32 / scale);
                let size = vec2(b.width as f32 / scale, b.height as f32 / scale);
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::OuterPosition(position));
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::InnerSize(size));
            }
            self.raise(ctx, ROOT);
        }
    }

    fn begin_frame(&mut self, ctx: &Context) {
        self.closing.clear();
        for window in self.windows.iter_mut() {
            window.geometry = None;
        }
        self.handle_forwarded(ctx);
        self.pump(ctx);
        self.poll_copy(ctx);
        self.expire_toast(ctx);
    }

    fn end_frame(&mut self, ctx: &Context) {
        self.update_drag(ctx);
        let mut changes = Vec::new();
        for id in std::mem::take(&mut self.closing) {
            changes.extend(self.windows.close_window(id).unwrap_or_default());
        }
        changes.extend(self.windows.settle());
        self.apply_changes(ctx, changes);
        let visible: HashSet<u64> =
            self.windows.iter().filter_map(|w| w.state.active_tab()).map(|t| t.gallery.id()).collect();
        self.thumbs.retain_galleries(|id| visible.contains(&id));
        self.keep_restored_focus(ctx);
        self.publish_target();
        self.sync_session();
        self.flush_config(ctx);
    }

    fn keep_restored_focus(&mut self, ctx: &Context) {
        let Some((want, since)) = self.restore_focus else { return };
        if since.elapsed() > RESTORE_FOCUS_FOR || self.windows.get(want).is_none() {
            self.restore_focus = None;
            return;
        }
        if !self.restore_raised && since.elapsed() >= RESTORE_RAISE_AT {
            self.restore_raised = true;
            self.raise(ctx, want);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn expire_toast(&mut self, ctx: &Context) {
        let Some(toast) = &self.toast else { return };
        let age = toast.at.elapsed();
        if age >= TOAST_DURATION && self.pending_copy.is_none() {
            self.toast = None;
        } else {
            ctx.request_repaint_after(TOAST_DURATION.saturating_sub(age).max(Duration::from_millis(50)));
        }
    }

    fn handle_input(&mut self, ctx: &Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            self.open_path(ctx, &path);
        }

        match ctx.input_mut(tab_shortcut) {
            Some(TabShortcut::New) => self.open_requested = true,
            Some(TabShortcut::CloseActive) => self.close_tab(ctx, self.windows.state(self.cur).active),
            Some(TabShortcut::Cycle(delta)) => {
                self.windows.state_mut(self.cur).cycle(delta);
                ctx.request_repaint();
            }
            None => {}
        }

        let typing = ctx.egui_wants_keyboard_input();
        let browsing = self.windows.state(self.cur).active_tab().is_some_and(|t| t.preview.is_none());
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
        if rescan && let Some(tab) = self.windows.state_mut(self.cur).active_tab_mut() {
            tab.refresh(&mut self.thumbs);
        }
        self.handle_gallery_keys(ctx);
    }

    fn handle_gallery_keys(&mut self, ctx: &Context) {
        let Some(tab) = self.windows.state(self.cur).active_tab() else { return };
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
                if let Some(tab) = self.windows.state_mut(self.cur).active_tab_mut() {
                    tab.move_focus(dir, extend);
                }
            }
            Some(KeyAction::CopyFiles) => self.copy_files(ctx),
            Some(KeyAction::CopyPaths) => self.copy_paths(ctx),
            Some(KeyAction::Pin) => self.toggle_focus_pin(),
            Some(KeyAction::Open) => {
                if let Some(tab) = self.windows.state_mut(self.cur).active_tab_mut() {
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
                let Some(tab) = self.windows.state_mut(self.cur).active_tab_mut() else { return };
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
        let Some(tab) = self.windows.state(self.cur).active_tab() else { return };
        let Some(path) = tab.focus_id().map(|i| tab.gallery.order()[i].clone()) else { return };
        if tab.library.get(&path).is_some() && path.exists() {
            self.config.toggle_pin(&tab.root, &path);
            self.touch_config();
        }
    }

    fn notify(&mut self, ctx: &Context, window: WindowId, text: String) {
        self.toast = Some(Toast { window, text, at: Instant::now() });
        ctx.request_repaint_after(TOAST_DURATION);
    }

    fn copy_files(&mut self, ctx: &Context) {
        let Some(tab) = self.windows.state(self.cur).active_tab() else { return };
        let files = tab.selected_paths();
        if files.is_empty() {
            return;
        }
        let focus = match &tab.preview {
            Some(preview) => {
                if let Some(img) = preview.decoded_for(&preview.path) {
                    self.finish_copy(ctx, self.cur, files, Some((*img).clone()));
                    return;
                }
                Some(preview.path.clone())
            }
            None => tab.focus_id().map(|i| tab.gallery.order()[i].clone()),
        };
        let Some(focus) = focus else {
            self.finish_copy(ctx, self.cur, files, None);
            return;
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wake = wake(ctx);
        std::thread::spawn(move || {
            let _ = tx.send(formats::decode(&focus));
            wake();
        });
        self.pending_copy = Some(PendingCopy { window: self.cur, files, decoded: rx });
        self.notify(ctx, self.cur, "Copie en cours…".to_string());
    }

    fn poll_copy(&mut self, ctx: &Context) {
        let Some(pending) = &self.pending_copy else { return };
        let result = match pending.decoded.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(DecodeError::Unsupported),
        };
        let Some(PendingCopy { window, files, .. }) = self.pending_copy.take() else { return };
        if let Err(e) = &result {
            log::warn!("cannot decode image for the clipboard: {e}");
        }
        self.finish_copy(ctx, window, files, result.ok());
    }

    fn finish_copy(&mut self, ctx: &Context, window: WindowId, files: Vec<PathBuf>, bitmap: Option<RgbaImage>) {
        let text = match platform::copy_files_and_bitmap(&files, bitmap.as_ref()) {
            Ok(()) => format!("Copié : {} fichier(s)", files.len()),
            Err(e) => format!("Copie impossible : {e}"),
        };
        self.notify(ctx, window, text);
    }

    fn copy_paths(&mut self, ctx: &Context) {
        let paths = self.windows.state(self.cur).active_tab().map(Tab::selected_paths).unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n");
        let message = match platform::copy_text(&text) {
            Ok(()) => "Chemins copiés".to_string(),
            Err(e) => format!("Copie impossible : {e}"),
        };
        self.notify(ctx, self.cur, message);
    }

    fn pick_folder(&mut self, ctx: &Context) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(tab) = self.windows.state(self.cur).active_tab().filter(|t| t.root.is_dir()) {
            dialog = dialog.set_directory(&tab.root);
        }
        if let Some(folder) = dialog.pick_folder() {
            self.open_target(ctx, OpenTarget { folder, select: None }, self.cur);
        }
    }

    /// Applies toolbar edits of the folder settings to the active tab and the config.
    fn apply_folder_settings(&mut self, ctx: &Context, settings: FolderSettings) {
        let Some(tab) = self.windows.state_mut(self.cur).active_tab_mut() else { return };
        if settings.recursive != tab.recursive {
            tab.reopen(ctx, &mut self.thumbs, settings);
        }
        if settings.flat != tab.flat {
            tab.flat = settings.flat;
            tab.gallery.scroll_to_top();
        }
        tab.sort = settings.sort;
        if self.config.set_folder_settings(&tab.root, settings) {
            self.config_dirty = true;
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let win = self.windows.state_mut(self.cur);
        let Some(tab) = win.tabs.get_mut(win.active) else {
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
            flat: tab.recursive && tab.flat,
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

impl ImageMozApp {
    fn sync_title(&mut self, ctx: &Context, id: WindowId) {
        let title = window_title(self.windows.state(id).active_tab().map(|t| t.root.as_path()));
        let Some(window) = self.windows.get_mut(id) else { return };
        // Other windows get their title through their viewport builder.
        if id == ROOT && window.title != title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
        }
        window.title = title;
    }

    fn start_drag(&mut self, id: WindowId, index: usize) {
        if let Some(tab) = self.windows.state(id).tabs.get(index) {
            self.drag = Some(TabDrag { tab_id: tab.id, label: tab.label(), cursor_px: Pos2::ZERO, target: None });
        }
    }

    /// A label that follows the cursor, drawn by whichever window the cursor is over.
    fn paint_drag_ghost(&self, ctx: &Context, screen: Option<(Pos2, f32)>) {
        let (Some(drag), Some((origin, scale))) = (&self.drag, screen) else { return };
        let at = geometry::to_local(drag.cursor_px, origin, scale);
        if !ctx.viewport_rect().contains(at) {
            return;
        }
        let layer = egui::LayerId::new(egui::Order::Tooltip, Id::new("tab_drag_ghost"));
        let painter = ctx.layer_painter(layer);
        let galley = painter.layout_no_wrap(drag.label.clone(), egui::FontId::proportional(13.0), Color32::WHITE);
        let rect = Rect::from_min_size(at + vec2(14.0, 14.0), galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(rect, CornerRadius::same(4), Color32::from_black_alpha(210));
        painter.galley(rect.min + vec2(8.0, 4.0), galley, Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    /// The whole UI of one window: tab bar, toolbar, gallery or preview, status bar.
    fn show_window(&mut self, ui: &mut egui::Ui, id: WindowId) {
        self.cur = id;
        let ctx = ui.ctx().clone();
        let screen = self.observe_window(&ctx, id);
        self.handle_input(&ctx);
        self.sync_title(&ctx, id);
        if let Some(tab) = self.windows.state_mut(id).active_tab_mut() {
            tab.sync_preview();
        }

        let drag_view = DragView {
            dragged: self.drag.as_ref().map(|d| d.tab_id),
            marker: self.drag.as_ref().and_then(|d| d.target).filter(|t| t.window == id).map(|t| t.index),
        };
        let win = self.windows.state(id);
        let (tab_events, bar_rect) = if win.tabs.is_empty() {
            (TabBarEvents::default(), None)
        } else {
            let bar = egui::Panel::top(Id::new(("tabs", id)))
                .show(ui, |ui| tabbar::show(ui, &win.tabs, win.active, drag_view));
            (bar.inner, Some(bar.response.rect))
        };
        let previewing = win.active_tab().is_some_and(|t| t.preview.is_some());
        let mut settings = win
            .active_tab()
            .map_or_else(FolderSettings::default, |t| FolderSettings { recursive: t.recursive, sort: t.sort, flat: t.flat });
        let mut filter = win.active_tab().map_or_else(String::new, |t| t.gallery.filter().to_owned());
        let has_tab = win.active_tab().is_some();
        let events = if previewing {
            toolbar::ToolbarEvents::default()
        } else {
            egui::Panel::top(Id::new(("toolbar", id)))
                .show(ui, |ui| toolbar::show(ui, &mut self.config, &mut settings, &mut filter, has_tab))
                .inner
        };
        if !previewing
            && let Some(tab) = self.windows.state_mut(id).active_tab_mut()
            && tab.gallery.filter() != filter
        {
            tab.gallery.set_filter(&filter);
            ctx.request_repaint();
        }
        let tab = self.windows.state(id).active_tab();
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
            matching: tab.and_then(|t| t.gallery.match_count()),
            selected: tab.map_or(0, |t| t.selection.len()),
            loading: tab.is_some_and(Tab::is_scanning),
            message: self.toast.as_ref().filter(|t| t.window == id).map(|t| t.text.as_str()),
        };
        egui::Panel::bottom(Id::new(("status", id))).show(ui, |ui| status::show(ui, &info));
        if previewing {
            let win = self.windows.state_mut(id);
            if let Some(tab) = win.tabs.get_mut(win.active) {
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
            self.windows.state_mut(id).activate(index);
            ctx.request_repaint();
        }
        if let Some(index) = tab_events.drag_started {
            self.start_drag(id, index);
        }
        if let Some(index) = tab_events.close {
            self.close_tab(&ctx, index);
        }
        if events.open || std::mem::take(&mut self.open_requested) {
            self.pick_folder(&ctx);
        }
        if let (Some(bar), Some((origin_px, scale))) = (bar_rect, screen)
            && let Some(window) = self.windows.get_mut(id)
        {
            window.geometry = Some(BarGeometry { bar, tabs: tab_events.tab_rects, origin_px, scale });
        }
        self.log_tab_layout(id);
        self.paint_drag_ghost(&ctx, screen);
    }

    /// Debug log of where each tab sits on screen (physical pixels), whenever it changes.
    /// The end-to-end suite reads it because secondary windows are not reachable through UI Automation.
    fn log_tab_layout(&mut self, id: WindowId) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let Some(window) = self.windows.get(id) else { return };
        let (Some(geometry), Some(bounds)) = (&window.geometry, window.bounds) else { return };
        let (origin, scale) = (geometry.origin_px, geometry.scale);
        let tabs: Vec<String> = window
            .state
            .tabs
            .iter()
            .zip(&geometry.tabs)
            .map(|(tab, r)| {
                let (left, right) = ((origin.x + r.min.x * scale).round(), (origin.x + r.max.x * scale).round());
                let (top, bottom) = ((origin.y + r.min.y * scale).round(), (origin.y + r.max.y * scale).round());
                format!("{}:{left}:{right}:{top}:{bottom}", tabbar::title(tab))
            })
            .collect();
        let line = format!("tabbar outer={},{} tabs={}", bounds.x, bounds.y, tabs.join("|"));
        if let Some(window) = self.windows.get_mut(id)
            && window.logged_tabs != line
        {
            log::debug!("{line}");
            window.logged_tabs = line;
        }
    }

    fn show_secondary(&mut self, ctx: &Context, id: WindowId) {
        let Some(window) = self.windows.get(id) else { return };
        let title = window_title(window.state.active_tab().map(|t| t.root.as_path()));
        let builder = window_builder(title, window.initial, self.scale, self.icon.clone());
        ctx.show_viewport_immediate(viewport_id(id), builder, |ui, _class| self.show_window(ui, id));
    }
}

impl eframe::App for ImageMozApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.begin_frame(&ctx);
        for id in self.windows.ids() {
            if id == ROOT {
                self.show_window(ui, ROOT);
            } else {
                self.show_secondary(&ctx, id);
            }
        }
        self.end_frame(&ctx);
    }

    fn on_exit(&mut self) {
        log::debug!("exiting with {} window(s)", self.windows.len());
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
    fn a_stored_window_opens_at_its_pixels_whatever_the_scale() {
        let fitted = Fitted { position: Some((300, 150)), size: (1500, 900) };
        let builder = window_builder("t".into(), Some(fitted), 1.5, None);
        assert_eq!(builder.position, Some(pos2(200.0, 100.0)));
        assert_eq!(builder.inner_size, Some(vec2(1000.0, 600.0)));
        let default = window_builder("t".into(), None, 1.5, None);
        assert_eq!((default.position, default.inner_size), (None, Some(vec2(1200.0, 800.0))));
    }

    #[test]
    fn title_names_the_app_and_the_active_folder() {
        assert_eq!(window_title(None), "Image Moz");
        assert_eq!(window_title(Some(Path::new(r"D:\Pics"))), "Image Moz — D:\\Pics");
    }
}
