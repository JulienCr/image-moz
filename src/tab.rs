//! One open folder (`Tab`) and the tabs of one window (`WindowState`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use eframe::egui::{self, Context};

use crate::config::{self, Config, FolderSettings, WindowSession};
use crate::geometry;
use crate::layout::{self, Dir};
use crate::library::{self, Entry, FsChange, Library, SortOrder};
use crate::preview::PreviewState;
use crate::selection::Selection;
use crate::ui::gallery::{Gallery, Thumbs};
use crate::watcher::FolderWatcher;

type ScanResult = io::Result<Vec<Entry>>;

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderIssue {
    Missing,
    Unreadable,
}

pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn wake(ctx: &Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// What a path from the command line, a drop or another instance asks to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget {
    pub folder: PathBuf,
    /// An image file was given: its folder opens and this file gets selected.
    pub select: Option<PathBuf>,
}

impl OpenTarget {
    pub fn from_path(path: &Path) -> Option<Self> {
        if path.as_os_str().is_empty() {
            return None;
        }
        if path.is_file() {
            let folder = path.parent()?.to_path_buf();
            return Some(Self { folder, select: Some(path.to_path_buf()) });
        }
        Some(Self { folder: path.to_path_buf(), select: None })
    }
}

/// The tab to show after `closed` was removed, leaving `remaining` tabs.
pub fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if remaining == 0 {
        return 0;
    }
    match closed.cmp(&active) {
        std::cmp::Ordering::Less => active - 1,
        std::cmp::Ordering::Equal => closed.min(remaining - 1),
        std::cmp::Ordering::Greater => active,
    }
}

/// Tabs to reopen from a stored window, and the index of the one to show among them.
pub fn restore_plan(stored: &WindowSession, exists: impl Fn(&Path) -> bool) -> (Vec<PathBuf>, usize) {
    let mut kept: Vec<PathBuf> = Vec::new();
    let mut active = 0;
    for (i, folder) in stored.tabs.iter().enumerate() {
        let key = config::folder_key(folder);
        if !exists(folder) || kept.iter().any(|k| config::folder_key(k) == key) {
            continue;
        }
        if i <= stored.active {
            active = kept.len();
        }
        kept.push(folder.clone());
    }
    (kept, active)
}

pub struct Tab {
    /// Stable across moves between windows, unlike its index.
    pub id: u64,
    pub root: PathBuf,
    /// What the library, watcher and scan were built with; changing it means `reopen`.
    pub recursive: bool,
    pub sort: SortOrder,
    /// Single ungrouped list in recursive mode; no effect otherwise.
    pub flat: bool,
    pub library: Library,
    pub gallery: Gallery,
    pub selection: Selection,
    pub hovered: Option<PathBuf>,
    pub scroll_to: Option<usize>,
    /// Order the selection positions refer to, with the gallery version it came from.
    pub order_snapshot: (u64, Vec<PathBuf>),
    pub preview: Option<PreviewState>,
    pub folder_issue: Option<FolderIssue>,
    /// Images that arrived since the tab was last visible.
    pub unseen: usize,
    /// True for the tab its window shows.
    pub visible: bool,
    /// Image to select once the scan lists it.
    pending_select: Option<PathBuf>,
    watcher: Option<FolderWatcher>,
    change_tx: Sender<FsChange>,
    changes: Receiver<FsChange>,
    scan: Option<Receiver<ScanResult>>,
    scan_is_initial: bool,
    rescan_wanted: bool,
    /// Changes received while a scan runs; replayed on top of its result.
    deferred: Vec<FsChange>,
}

impl Tab {
    pub fn open(ctx: &Context, root: PathBuf, settings: FolderSettings) -> Self {
        let (change_tx, changes) = crossbeam_channel::unbounded();
        let mut tab = Self {
            id: NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed),
            library: Library::new(root.clone(), settings.recursive),
            root,
            recursive: settings.recursive,
            sort: settings.sort,
            flat: settings.flat,
            gallery: Gallery::new(),
            selection: Selection::default(),
            hovered: None,
            scroll_to: None,
            order_snapshot: (0, Vec::new()),
            preview: None,
            folder_issue: None,
            unseen: 0,
            visible: false,
            pending_select: None,
            watcher: None,
            change_tx,
            changes,
            scan: None,
            scan_is_initial: true,
            rescan_wanted: false,
            deferred: Vec::new(),
        };
        tab.arm_watcher(ctx);
        tab.start_scan(ctx);
        tab
    }

    /// Re-reads the folder from scratch with new settings: new library, watcher and scan.
    pub fn reopen(&mut self, ctx: &Context, thumbs: &mut Thumbs, settings: FolderSettings) {
        self.recursive = settings.recursive;
        self.sort = settings.sort;
        self.flat = settings.flat;
        self.library = Library::new(self.root.clone(), settings.recursive);
        self.selection.clear();
        self.order_snapshot.1.clear();
        self.preview = None;
        self.hovered = None;
        self.pending_select = None;
        self.deferred.clear();
        self.rescan_wanted = false;
        self.folder_issue = None;
        self.gallery.reset(thumbs);
        let (tx, rx) = crossbeam_channel::unbounded();
        self.change_tx = tx;
        self.changes = rx;
        self.arm_watcher(ctx);
        self.scan_is_initial = true;
        self.start_scan(ctx);
    }

    pub fn label(&self) -> String {
        match self.root.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => self.root.display().to_string(),
        }
    }

    /// The watcher is armed and its folder still there.
    pub fn is_live(&self) -> bool {
        self.watcher.is_some() && self.folder_issue.is_none()
    }

    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// F5: rescan, and give failed tiles another attempt.
    pub fn refresh(&mut self, thumbs: &mut Thumbs) {
        self.rescan_wanted = true;
        self.gallery.retry_failed(thumbs, &mut self.library);
    }

    /// Selects `path` once the scan lists it; ignored when the folder is settled and lacks it.
    pub fn select_when_listed(&mut self, path: PathBuf) {
        self.gallery.clear_filter();
        if self.scan.is_some() || self.library.get(&path).is_some() {
            self.pending_select = Some(path);
        }
    }

    /// (Re)starts the watcher; false when the OS refuses (e.g. the folder is gone).
    fn arm_watcher(&mut self, ctx: &Context) -> bool {
        self.watcher = None;
        match FolderWatcher::start(&self.root, self.recursive, self.change_tx.clone(), wake(ctx)) {
            Ok(watcher) => {
                self.watcher = Some(watcher);
                true
            }
            Err(e) => {
                log::warn!("cannot watch {}: {e}", self.root.display());
                false
            }
        }
    }

    fn start_scan(&mut self, ctx: &Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (folder, recursive, wake) = (self.root.clone(), self.recursive, wake(ctx));
        std::thread::spawn(move || {
            let _ = tx.send(library::scan(&folder, recursive));
            wake();
        });
        self.scan = Some(rx);
    }

    fn note_arrivals(&mut self, count: usize) {
        if !self.visible {
            self.unseen += count;
        }
    }

    /// Swaps in a scan result, keeping dims, error and highlight of unchanged files.
    fn apply_scan(&mut self, ctx: &Context, thumbs: &mut Thumbs, result: ScanResult) {
        match result {
            Err(e) => {
                log::warn!("scan failed: {e}");
                let missing = e.kind() == io::ErrorKind::NotFound;
                if self.scan_is_initial || missing {
                    self.gallery.replace_entries(thumbs, &mut self.library, Vec::new());
                    self.folder_issue = Some(if missing { FolderIssue::Missing } else { FolderIssue::Unreadable });
                }
            }
            Ok(mut entries) => {
                // A watcher that never started, or died with its folder, is re-armed here.
                let needs_watcher = self.watcher.is_none() || self.folder_issue.is_some();
                self.folder_issue = None;
                let now = Instant::now();
                let mut arrived = 0;
                for entry in &mut entries {
                    match self.library.get(&entry.path) {
                        Some(old) if old.mtime == entry.mtime && old.size == entry.size => {
                            entry.dims = old.dims;
                            entry.triangles = old.triangles;
                            entry.no_preview = old.no_preview;
                            entry.error = old.error;
                            entry.arrived_at = old.arrived_at;
                        }
                        Some(_) => {}
                        None if !self.scan_is_initial => {
                            entry.arrived_at = Some(now);
                            arrived += 1;
                        }
                        None => {}
                    }
                }
                self.note_arrivals(arrived);
                self.gallery.replace_entries(thumbs, &mut self.library, entries);
                if needs_watcher && self.arm_watcher(ctx) {
                    // Changes between the scan and the arming were not watched.
                    self.rescan_wanted = true;
                }
            }
        }
        log::debug!("scan of {} applied: {} entries", self.root.display(), self.library.len());
        self.scan_is_initial = false;
        for change in std::mem::take(&mut self.deferred) {
            self.apply_change(thumbs, &change);
        }
        if self.pending_select.as_ref().is_some_and(|p| self.library.get(p).is_none()) {
            self.pending_select = None;
        }
    }

    fn apply_change(&mut self, thumbs: &mut Thumbs, change: &FsChange) {
        log::debug!("fs change: {change:?}");
        let before = self.library.len();
        self.gallery.apply_change(thumbs, &mut self.library, change, Instant::now());
        let arrived = self.library.len().saturating_sub(before);
        self.note_arrivals(arrived);
    }

    /// Collects scan results and watcher events; at most one scan runs at a time.
    pub fn pump(&mut self, ctx: &Context, thumbs: &mut Thumbs) {
        if let Some(scan) = &self.scan {
            match scan.try_recv() {
                Ok(result) => {
                    self.scan = None;
                    self.apply_scan(ctx, thumbs, result);
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
                change => self.apply_change(thumbs, &change),
            }
        }
        if self.rescan_wanted && self.scan.is_none() {
            self.rescan_wanted = false;
            self.start_scan(ctx);
        }
    }

    /// Tile id of `path` in the current gallery order.
    fn position_of(&self, path: &Path) -> Option<usize> {
        let listed = &self.library.get(path)?.path;
        self.gallery.order().iter().position(|p| p == listed)
    }

    /// Call after the gallery was laid out: selects and scrolls to the awaited image.
    pub fn resolve_pending_select(&mut self, ctx: &Context) {
        let Some(path) = &self.pending_select else { return };
        match self.position_of(path) {
            Some(id) => {
                self.selection.click(id);
                self.scroll_to = Some(id);
                self.pending_select = None;
                ctx.request_repaint();
            }
            // Listed but not laid out: its section is collapsed.
            None if self.library.get(path).is_some() => self.pending_select = None,
            None => {}
        }
    }

    pub fn focus_id(&self) -> Option<usize> {
        self.selection.focus().filter(|&i| i < self.gallery.order().len())
    }

    pub fn move_focus(&mut self, dir: Dir, extend: bool) {
        let target = match self.focus_id() {
            Some(from) => layout::neighbor(self.gallery.layout(), from, dir),
            None => Some(0),
        };
        if let Some(target) = target {
            self.selection.move_to(target, extend);
            self.scroll_to = Some(target);
        }
    }

    /// Opens the preview on tile `id`, or retargets an open one.
    pub fn show_preview(&mut self, ctx: &Context, id: usize) {
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

    pub fn open_preview(&mut self, ctx: &Context) {
        if let Some(id) = self.focus_id() {
            self.show_preview(ctx, id);
        }
    }

    pub fn step_preview(&mut self, ctx: &Context, delta: isize) {
        let Some(preview) = &self.preview else { return };
        let target = preview.index.checked_add_signed(delta).filter(|&i| i < self.gallery.order().len());
        if let Some(target) = target {
            self.show_preview(ctx, target);
        }
    }

    /// Keeps the preview on its file when the gallery order changes underneath it.
    pub fn sync_preview(&mut self) {
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

    /// Absolute paths of the selection, or of the focused tile when nothing is selected.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
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

    /// Re-targets selection positions when the gallery order was rebuilt.
    pub fn remap_selection(&mut self, ctx: &Context) {
        let version = self.gallery.order_version();
        if version == self.order_snapshot.0 {
            return;
        }
        let new_order = self.gallery.order();
        if new_order != self.order_snapshot.1.as_slice() {
            let positions: std::collections::HashMap<&PathBuf, usize> =
                new_order.iter().enumerate().map(|(i, p)| (p, i)).collect();
            let old = &self.order_snapshot.1;
            self.selection.remap(|i| old.get(i).and_then(|p| positions.get(p).copied()));
            ctx.request_repaint();
        }
        self.order_snapshot = (version, self.gallery.order().to_vec());
    }
}

/// The tabs of one window and the one it shows.
#[derive(Default)]
pub struct WindowState {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl WindowState {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Index of the tab showing `folder`, compared by folder key.
    pub fn find(&self, folder: &Path) -> Option<usize> {
        let key = config::folder_key(folder);
        self.tabs.iter().position(|t| config::folder_key(&t.root) == key)
    }

    pub fn activate(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            tab.visible = i == index;
        }
        let tab = &mut self.tabs[index];
        tab.unseen = 0;
        tab.gallery.activate();
    }

    /// Shows the tab of `folder`, opening one when there is none. Returns its index.
    pub fn open_folder(&mut self, ctx: &Context, config: &Config, folder: PathBuf) -> usize {
        let index = match self.find(&folder) {
            Some(i) => i,
            None => {
                let settings = config.folder_settings(&folder);
                self.tabs.push(Tab::open(ctx, folder, settings));
                self.tabs.len() - 1
            }
        };
        self.activate(index);
        index
    }

    pub fn close(&mut self, index: usize) {
        self.remove_tab(index);
    }

    /// Takes the tab out without dropping it, keeping the active tab (or its neighbour) shown.
    pub fn remove_tab(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let was_active = index == self.active;
        let tab = self.tabs.remove(index);
        self.active = active_after_close(self.active, index, self.tabs.len());
        if was_active {
            self.activate(self.active);
        }
        Some(tab)
    }

    /// Inserts `tab` before position `at` (clamped) and shows it. Returns its index.
    pub fn insert_tab(&mut self, at: usize, tab: Tab) -> usize {
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.activate(at);
        at
    }

    /// Re-inserts tab `from` before `insertion` (an index in the list that still holds it) and shows it.
    pub fn reorder(&mut self, from: usize, insertion: usize) {
        if from >= self.tabs.len() {
            return;
        }
        let to = geometry::reorder_target(from, insertion.min(self.tabs.len()));
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.activate(to);
    }

    /// Moves to the next (`+1`) or previous (`-1`) tab, wrapping around.
    pub fn cycle(&mut self, delta: isize) {
        let len = self.tabs.len() as isize;
        if len > 1 {
            self.activate((self.active as isize + delta).rem_euclid(len) as usize);
        }
    }

    pub fn session(&self) -> WindowSession {
        WindowSession { tabs: self.tabs.iter().map(|t| t.root.clone()).collect(), active: self.active, bounds: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(n: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
        let root = tempfile::tempdir().unwrap();
        let dirs = (0..n)
            .map(|i| {
                let d = root.path().join(format!("Dir{i}"));
                std::fs::create_dir(&d).unwrap();
                d
            })
            .collect();
        (root, dirs)
    }

    fn window_of(folders: &[PathBuf]) -> WindowState {
        let (ctx, config) = (Context::default(), Config::default());
        let mut window = WindowState::default();
        for f in folders {
            window.open_folder(&ctx, &config, f.clone());
        }
        window
    }

    #[test]
    fn closing_keeps_the_same_tab_or_its_neighbour_active() {
        assert_eq!(active_after_close(2, 0, 3), 1);
        assert_eq!(active_after_close(1, 1, 3), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
        assert_eq!(active_after_close(0, 2, 2), 0);
        assert_eq!(active_after_close(0, 0, 0), 0);
    }

    #[test]
    fn opening_an_open_folder_activates_it_whatever_its_spelling() {
        let (_root, d) = dirs(2);
        let mut window = window_of(&d);
        assert_eq!((window.tabs.len(), window.active), (2, 1));

        let shouted = PathBuf::from(d[0].to_string_lossy().to_uppercase());
        let again = window.open_folder(&Context::default(), &Config::default(), shouted);
        assert_eq!((again, window.tabs.len(), window.active), (0, 2, 0));

        let slashed = PathBuf::from(format!("{}\\", d[1].display()));
        assert_eq!(window.find(&slashed), Some(1));
    }

    #[test]
    fn closing_tabs_fixes_up_the_active_one() {
        let (_root, d) = dirs(3);
        let mut window = window_of(&d);
        window.activate(1);
        window.close(0);
        assert_eq!(window.active_tab().unwrap().root, d[1]);
        window.close(1);
        assert_eq!(window.active_tab().unwrap().root, d[1]);
        window.close(0);
        assert!(window.active_tab().is_none());
        assert_eq!(window.session(), WindowSession::default());
    }

    #[test]
    fn exactly_one_tab_is_visible_and_activation_clears_its_badge() {
        let (_root, d) = dirs(2);
        let mut window = window_of(&d);
        window.tabs[0].unseen = 3;
        assert_eq!([window.tabs[0].visible, window.tabs[1].visible], [false, true]);
        window.cycle(1);
        assert_eq!([window.tabs[0].visible, window.tabs[1].visible], [true, false]);
        assert_eq!(window.tabs[0].unseen, 0);
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        let (_root, d) = dirs(3);
        let mut window = window_of(&d);
        window.cycle(1);
        assert_eq!(window.active, 0);
        window.cycle(-1);
        assert_eq!(window.active, 2);
    }

    #[test]
    fn a_new_tab_takes_the_folder_settings_from_the_config() {
        let (_root, d) = dirs(1);
        let mut config = Config::default();
        config.set_folder_settings(&d[0], FolderSettings { recursive: true, sort: SortOrder::NameAsc, flat: false });
        let mut window = WindowState::default();
        window.open_folder(&Context::default(), &config, d[0].clone());
        let tab = window.active_tab().unwrap();
        assert_eq!((tab.recursive, tab.sort), (true, SortOrder::NameAsc));
    }

    #[test]
    fn restore_skips_missing_folders_and_duplicates_and_keeps_the_active_tab() {
        let stored = WindowSession {
            tabs: ["a", "gone", "b", "A\\", "c"].map(PathBuf::from).to_vec(),
            active: 2,
            ..Default::default()
        };
        let exists = |p: &Path| p.to_string_lossy() != "gone";
        let (tabs, active) = restore_plan(&stored, exists);
        assert_eq!(tabs, ["a", "b", "c"].map(PathBuf::from));
        assert_eq!(active, 1);

        let on_missing = WindowSession { active: 1, ..stored };
        let (tabs, active) = restore_plan(&on_missing, exists);
        assert_eq!((tabs.len(), active), (3, 0));
    }

    #[test]
    fn an_image_file_opens_its_folder_and_asks_to_select_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, b"x").unwrap();
        let target = OpenTarget::from_path(&file).unwrap();
        assert_eq!(target, OpenTarget { folder: dir.path().to_path_buf(), select: Some(file) });
        let target = OpenTarget::from_path(dir.path()).unwrap();
        assert_eq!(target, OpenTarget { folder: dir.path().to_path_buf(), select: None });
        let missing = dir.path().join("nope");
        assert_eq!(OpenTarget::from_path(&missing).unwrap().folder, missing);
        assert_eq!(OpenTarget::from_path(Path::new("")), None);
    }

    #[test]
    fn images_arriving_in_a_background_tab_are_counted_until_it_is_shown() {
        let (root, d) = dirs(2);
        let ctx = Context::default();
        let cache = crate::thumbs::DiskCache::open(root.path().join("cache")).unwrap();
        let mut thumbs = Thumbs::with_cache(&ctx, cache);
        let mut window = window_of(&d);
        for tab in &mut window.tabs {
            for _ in 0..200 {
                tab.pump(&ctx, &mut thumbs);
                if !tab.is_scanning() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(!tab.is_scanning());
        }
        let new_file = d[0].join("new.png");
        std::fs::write(&new_file, b"not really a png").unwrap();
        window.tabs[0].apply_change(&mut thumbs, &FsChange::Upsert(new_file));
        assert_eq!(window.tabs[0].unseen, 1);
        window.activate(0);
        assert_eq!(window.tabs[0].unseen, 0);
        let second = d[0].join("second.png");
        std::fs::write(&second, b"x").unwrap();
        window.tabs[0].apply_change(&mut thumbs, &FsChange::Upsert(second));
        assert_eq!(window.tabs[0].unseen, 0);
    }
}
