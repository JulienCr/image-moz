//! The set of windows and who owns which tab: moves, detaches, closes and the root window's succession.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::{self, Session, WindowBounds, WindowSession};
use crate::geometry::{BarGeometry, Fitted};
use crate::tab::{WindowState, restore_plan};

pub type WindowId = u64;

/// The window eframe owns; it can be emptied and refilled but never removed.
pub const ROOT: WindowId = 0;

/// How long a new window is nudged towards its intended bounds, which a monitor with another scale can distort.
const SETTLE_FOR: Duration = Duration::from_secs(3);
const SETTLE_ATTEMPTS: u8 = 8;
/// The bounds must hold this long: a scale change after the move can undo them.
const SETTLE_HOLD: Duration = Duration::from_millis(400);

/// Where a window was asked to open, kept until the native window reports it is there.
pub struct Placement {
    pub target: WindowBounds,
    since: Instant,
    attempts: u8,
    holding_since: Option<Instant>,
}

impl Placement {
    pub fn new(target: WindowBounds) -> Self {
        Self { target, since: Instant::now(), attempts: 0, holding_since: None }
    }

    /// What to do after seeing the window at `actual` (within `tolerance` px of the target or not).
    pub fn step(&mut self, matches: bool) -> Step {
        if self.since.elapsed() >= SETTLE_FOR {
            return Step::Done;
        }
        if matches {
            let held = *self.holding_since.get_or_insert_with(Instant::now);
            return if held.elapsed() >= SETTLE_HOLD { Step::Done } else { Step::Wait };
        }
        self.holding_since = None;
        self.attempts += 1;
        if self.attempts > SETTLE_ATTEMPTS { Step::Done } else { Step::Correct }
    }
}

/// What a window that should be somewhere else needs next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Resend the target bounds.
    Correct,
    /// At the target, but not for long enough to trust it yet.
    Wait,
    Done,
}

pub struct AppWindow {
    pub id: WindowId,
    pub state: WindowState,
    /// Where a secondary viewport opens; ignored once it exists.
    pub initial: Option<Fitted>,
    /// Last known outer position and inner size, kept while the window is minimized.
    pub bounds: Option<WindowBounds>,
    pub hwnd: Option<isize>,
    /// Tab bar rects of the current frame; `None` when the window drew none.
    pub geometry: Option<BarGeometry>,
    pub placement: Option<Placement>,
    /// Title last sent to the native window.
    pub title: String,
}

impl AppWindow {
    fn new(id: WindowId, state: WindowState) -> Self {
        Self { id, state, initial: None, bounds: None, hwnd: None, geometry: None, placement: None, title: String::new() }
    }
}

/// A structural change the caller has to mirror on the native windows.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// The viewport is no longer shown.
    Closed(WindowId),
    /// The root now holds another window's tabs and should take that window's place.
    Promoted(Option<WindowBounds>),
}

pub struct Windows {
    list: Vec<AppWindow>,
    next_id: WindowId,
    /// Window ids in order of activation, most recent last.
    recent: Vec<WindowId>,
}

impl Default for Windows {
    fn default() -> Self {
        Self::new(WindowState::default())
    }
}

impl Windows {
    pub fn new(root: WindowState) -> Self {
        Self { list: vec![AppWindow::new(ROOT, root)], next_id: ROOT + 1, recent: vec![ROOT] }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn ids(&self) -> Vec<WindowId> {
        self.list.iter().map(|w| w.id).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AppWindow> {
        self.list.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut AppWindow> {
        self.list.iter_mut()
    }

    pub fn get(&self, id: WindowId) -> Option<&AppWindow> {
        self.list.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut AppWindow> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    /// The tabs of a window that must exist, e.g. the one being drawn.
    pub fn state(&self, id: WindowId) -> &WindowState {
        &self.get(id).expect("the window being drawn exists").state
    }

    pub fn state_mut(&mut self, id: WindowId) -> &mut WindowState {
        &mut self.get_mut(id).expect("the window being drawn exists").state
    }

    /// Marks `id` as the window the user touched last.
    pub fn touch(&mut self, id: WindowId) {
        if self.get(id).is_some() {
            self.recent.retain(|&r| r != id);
            self.recent.push(id);
        }
    }

    /// The most recently touched window that still exists, the root when none was touched.
    pub fn last_active(&self) -> WindowId {
        self.recent.iter().rev().copied().find(|&id| self.get(id).is_some()).unwrap_or(ROOT)
    }

    pub fn add(&mut self, state: WindowState, initial: Option<Fitted>) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        let mut window = AppWindow::new(id, state);
        window.initial = initial;
        window.bounds = initial.and_then(|f| {
            let (x, y) = f.position?;
            Some(WindowBounds { x, y, width: f.size.0, height: f.size.1 })
        });
        window.placement = window.bounds.map(Placement::new);
        self.list.push(window);
        self.touch(id);
        id
    }

    /// Window and index of the tab with this id.
    pub fn locate_tab(&self, tab_id: u64) -> Option<(WindowId, usize)> {
        self.list.iter().find_map(|w| Some((w.id, w.state.tabs.iter().position(|t| t.id == tab_id)?)))
    }

    /// Window and index of the tab showing `folder`, in any window.
    pub fn find_folder(&self, folder: &Path) -> Option<(WindowId, usize)> {
        self.list.iter().find_map(|w| Some((w.id, w.state.find(folder)?)))
    }

    /// Re-inserts a tab before `insertion` in its own window, then shows it.
    pub fn reorder(&mut self, tab_id: u64, insertion: usize) {
        if let Some((window, index)) = self.locate_tab(tab_id)
            && let Some(window) = self.get_mut(window)
        {
            window.state.reorder(index, insertion);
        }
    }

    /// Moves the tab itself (watcher, library, selection and scroll with it) and shows it there.
    pub fn move_tab(&mut self, tab_id: u64, to: WindowId, insertion: usize) -> Vec<Change> {
        let Some((from, index)) = self.locate_tab(tab_id) else { return Vec::new() };
        if self.get(to).is_none() {
            return Vec::new();
        }
        if from == to {
            self.reorder(tab_id, insertion);
            return Vec::new();
        }
        let Some(tab) = self.get_mut(from).and_then(|w| w.state.remove_tab(index)) else { return Vec::new() };
        if let Some(window) = self.get_mut(to) {
            window.state.insert_tab(insertion, tab);
        }
        self.touch(to);
        self.settle()
    }

    /// Moves a tab to a new window. A window holding only that tab is left alone: it would just recreate itself.
    pub fn detach(&mut self, tab_id: u64, initial: Fitted) -> Option<WindowId> {
        let (from, index) = self.locate_tab(tab_id)?;
        if self.get(from)?.state.tabs.len() < 2 {
            return None;
        }
        let tab = self.get_mut(from)?.state.remove_tab(index)?;
        let mut state = WindowState::default();
        state.insert_tab(0, tab);
        Some(self.add(state, Some(initial)))
    }

    /// Closes a window with its tabs. Returns `None` for the last window, which only the exit path may close.
    pub fn close_window(&mut self, id: WindowId) -> Option<Vec<Change>> {
        if self.list.len() < 2 || self.get(id).is_none() {
            return None;
        }
        if let Some(window) = self.get_mut(id) {
            window.state = WindowState::default();
        }
        Some(self.settle())
    }

    /// Drops windows left without tabs, and refills an empty root from the most recent other window.
    pub fn settle(&mut self) -> Vec<Change> {
        let mut changes = Vec::new();
        let emptied: Vec<WindowId> =
            self.list.iter().filter(|w| w.id != ROOT && w.state.tabs.is_empty()).map(|w| w.id).collect();
        for id in emptied {
            self.list.retain(|w| w.id != id);
            changes.push(Change::Closed(id));
        }
        if self.list[0].state.tabs.is_empty() && self.list.len() > 1 {
            let donor = self.recent.iter().rev().copied().find(|&id| id != ROOT && self.get(id).is_some());
            let donor = donor.unwrap_or(self.list[1].id);
            if let Some(at) = self.list.iter().position(|w| w.id == donor) {
                let donor = self.list.remove(at);
                self.recent.retain(|&r| r != donor.id);
                self.list[0].state = donor.state;
                self.list[0].bounds = donor.bounds;
                self.touch(ROOT);
                changes.push(Change::Promoted(donor.bounds));
            }
        }
        self.recent.retain(|&id| self.list.iter().any(|w| w.id == id));
        changes
    }

    pub fn session(&self) -> Session {
        let active = self.last_active();
        Session {
            windows: self.list.iter().map(|w| WindowSession { bounds: w.bounds, ..w.state.session() }).collect(),
            active_window: self.list.iter().position(|w| w.id == active).unwrap_or(0),
        }
    }
}

/// One window to reopen from a stored session.
#[derive(Debug, PartialEq)]
pub struct WindowPlan {
    pub folders: Vec<PathBuf>,
    pub active: usize,
    pub bounds: Option<WindowBounds>,
}

/// Windows to reopen, skipping vanished folders and windows left without tabs, and which one was active.
pub fn session_plan(session: &Session, exists: impl Fn(&Path) -> bool) -> (Vec<WindowPlan>, usize) {
    let mut plans = Vec::new();
    let mut active_window = 0;
    for (i, stored) in session.windows.iter().enumerate() {
        let (folders, active) = restore_plan(stored, &exists);
        if folders.is_empty() {
            continue;
        }
        if i == session.active_window {
            active_window = plans.len();
        }
        plans.push(WindowPlan { folders, active, bounds: stored.bounds });
    }
    (plans, active_window)
}

/// Whether two folders are the same tab target, whatever their spelling.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    config::folder_key(a) == config::folder_key(b)
}

#[cfg(test)]
mod tests {
    use eframe::egui::Context;

    use super::*;
    use crate::config::Config;
    use crate::geometry::Fitted;

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

    fn state_of(folders: &[PathBuf]) -> WindowState {
        let (ctx, config) = (Context::default(), Config::default());
        let mut state = WindowState::default();
        for f in folders {
            state.open_folder(&ctx, &config, f.clone());
        }
        state
    }

    fn roots(state: &WindowState) -> Vec<PathBuf> {
        state.tabs.iter().map(|t| t.root.clone()).collect()
    }

    fn placement() -> Fitted {
        Fitted { position: Some((10, 10)), size: (800, 600) }
    }

    /// Root holds `a` (tabs 0..2), a second window holds the rest.
    fn two_windows(d: &[PathBuf], split: usize) -> (Windows, WindowId) {
        let mut windows = Windows::new(state_of(&d[..split]));
        let other = windows.add(state_of(&d[split..]), Some(placement()));
        (windows, other)
    }

    #[test]
    fn a_tab_moves_to_the_cursor_slot_of_another_window_and_becomes_active() {
        let (_root, d) = dirs(5);
        let (mut windows, other) = two_windows(&d, 2);
        windows.get_mut(ROOT).unwrap().state.activate(0);
        let moved = windows.get(other).unwrap().state.tabs[1].id;

        let changes = windows.move_tab(moved, ROOT, 1);

        assert!(changes.is_empty());
        let root = &windows.get(ROOT).unwrap().state;
        assert_eq!(roots(root), [d[0].clone(), d[3].clone(), d[1].clone()]);
        assert_eq!(root.active, 1);
        assert_eq!(roots(root)[root.active], d[3]);
        assert_eq!([root.tabs[0].visible, root.tabs[1].visible, root.tabs[2].visible], [false, true, false]);
        let source = &windows.get(other).unwrap().state;
        assert_eq!(roots(source), [d[2].clone(), d[4].clone()]);
        assert_eq!(windows.last_active(), ROOT);
    }

    #[test]
    fn leaving_the_active_tab_hands_the_source_window_to_its_neighbour() {
        let (_root, d) = dirs(5);
        let (mut windows, other) = two_windows(&d, 2);
        windows.get_mut(other).unwrap().state.activate(2);
        let moved = windows.get(other).unwrap().state.tabs[2].id;
        windows.move_tab(moved, ROOT, 9);
        let source = &windows.get(other).unwrap().state;
        assert_eq!((source.active, source.tabs[source.active].root.clone()), (1, d[3].clone()));
        assert!(source.tabs[1].visible && !source.tabs[0].visible);
        let target = &windows.get(ROOT).unwrap().state;
        assert_eq!(roots(target).last(), Some(&d[4]));
    }

    #[test]
    fn a_tab_keeps_its_identity_and_state_when_it_moves() {
        let (_root, d) = dirs(3);
        let (mut windows, other) = two_windows(&d, 1);
        let tab = &mut windows.get_mut(other).unwrap().state.tabs[0];
        tab.selection.click(4);
        tab.scroll_to = Some(9);
        let id = tab.id;
        windows.move_tab(id, ROOT, 0);
        let (window, index) = windows.locate_tab(id).unwrap();
        assert_eq!(window, ROOT);
        let moved = &windows.get(ROOT).unwrap().state.tabs[index];
        assert_eq!(moved.root, d[1]);
        assert!(moved.selection.contains(4));
        assert_eq!(moved.scroll_to, Some(9));
    }

    #[test]
    fn the_last_tab_leaving_a_secondary_window_closes_it() {
        let (_root, d) = dirs(3);
        let (mut windows, other) = two_windows(&d, 2);
        let only = windows.get(other).unwrap().state.tabs[0].id;
        let changes = windows.move_tab(only, ROOT, 0);
        assert_eq!(changes, [Change::Closed(other)]);
        assert_eq!(windows.ids(), [ROOT]);
        assert_eq!(windows.last_active(), ROOT);
        assert_eq!(roots(&windows.get(ROOT).unwrap().state), [d[2].clone(), d[0].clone(), d[1].clone()]);
    }

    #[test]
    fn the_root_emptied_by_a_move_takes_over_the_other_window() {
        let (_root, d) = dirs(3);
        let mut windows = Windows::new(state_of(&d[..1]));
        let other = windows.add(state_of(&d[1..]), Some(placement()));
        windows.get_mut(other).unwrap().bounds = Some(WindowBounds { x: 5, y: 6, width: 700, height: 500 });
        let only = windows.get(ROOT).unwrap().state.tabs[0].id;

        let changes = windows.move_tab(only, other, 0);

        assert_eq!(changes, [Change::Promoted(Some(WindowBounds { x: 5, y: 6, width: 700, height: 500 }))]);
        assert_eq!(windows.ids(), [ROOT]);
        assert_eq!(roots(&windows.get(ROOT).unwrap().state), [d[0].clone(), d[1].clone(), d[2].clone()]);
        assert_eq!(windows.get(ROOT).unwrap().bounds.unwrap().x, 5);
        assert_eq!(windows.last_active(), ROOT);
    }

    #[test]
    fn detaching_opens_a_window_for_the_tab_and_fixes_the_source() {
        let (_root, d) = dirs(3);
        let mut windows = Windows::new(state_of(&d));
        windows.get_mut(ROOT).unwrap().state.activate(1);
        let id = windows.get(ROOT).unwrap().state.tabs[1].id;

        let fresh = windows.detach(id, placement()).unwrap();

        let source = &windows.get(ROOT).unwrap().state;
        assert_eq!((roots(source), source.active), (vec![d[0].clone(), d[2].clone()], 1));
        let new = windows.get(fresh).unwrap();
        assert_eq!((roots(&new.state), new.state.active), (vec![d[1].clone()], 0));
        assert_eq!(new.initial, Some(placement()));
        assert!(new.state.tabs[0].visible);
        assert_eq!(windows.last_active(), fresh);
    }

    #[test]
    fn a_window_with_a_single_tab_cannot_be_detached() {
        let (_root, d) = dirs(2);
        let (mut windows, other) = two_windows(&d, 1);
        let only = windows.get(other).unwrap().state.tabs[0].id;
        assert_eq!(windows.detach(only, placement()), None);
        assert_eq!(windows.ids(), [ROOT, other]);
    }

    #[test]
    fn reordering_inside_a_window_follows_the_insertion_marker() {
        let (_root, d) = dirs(4);
        let mut windows = Windows::new(state_of(&d));
        let first = windows.get(ROOT).unwrap().state.tabs[0].id;
        windows.reorder(first, 3);
        let state = &windows.get(ROOT).unwrap().state;
        assert_eq!(roots(state), [d[1].clone(), d[2].clone(), d[0].clone(), d[3].clone()]);
        assert_eq!(state.active, 2);
        let last = state.tabs[3].id;
        windows.reorder(last, 0);
        let state = &windows.get(ROOT).unwrap().state;
        assert_eq!(roots(state)[0], d[3]);
        assert_eq!(state.active, 0);
        let same = state.tabs[0].id;
        windows.reorder(same, 1);
        assert_eq!(roots(&windows.get(ROOT).unwrap().state)[0], d[3]);
    }

    #[test]
    fn closing_the_root_promotes_the_window_used_last() {
        let (_root, d) = dirs(4);
        let mut windows = Windows::new(state_of(&d[..1]));
        let b = windows.add(state_of(&d[1..2]), Some(placement()));
        let c = windows.add(state_of(&d[2..]), Some(placement()));
        windows.touch(b);
        windows.touch(ROOT);
        windows.touch(b);

        let changes = windows.close_window(ROOT).unwrap();

        assert!(matches!(changes.as_slice(), [Change::Promoted(_)]));
        assert_eq!(windows.ids(), [ROOT, c]);
        assert_eq!(roots(&windows.get(ROOT).unwrap().state), [d[1].clone()]);
        assert_eq!(roots(&windows.get(c).unwrap().state), d[2..]);
    }

    #[test]
    fn closing_a_secondary_window_drops_its_tabs_and_the_last_window_cannot_close() {
        let (_root, d) = dirs(3);
        let (mut windows, other) = two_windows(&d, 1);
        assert_eq!(windows.close_window(other), Some(vec![Change::Closed(other)]));
        assert_eq!(windows.ids(), [ROOT]);
        assert_eq!(windows.close_window(ROOT), None);
        assert_eq!(roots(&windows.get(ROOT).unwrap().state), [d[0].clone()]);
    }

    #[test]
    fn the_last_active_window_follows_focus_and_survives_closures() {
        let (_root, d) = dirs(3);
        let mut windows = Windows::new(state_of(&d[..1]));
        assert_eq!(windows.last_active(), ROOT);
        let b = windows.add(state_of(&d[1..2]), None);
        let c = windows.add(state_of(&d[2..]), None);
        assert_eq!(windows.last_active(), c);
        windows.touch(b);
        assert_eq!(windows.last_active(), b);
        windows.close_window(b);
        assert_eq!(windows.last_active(), c);
        windows.close_window(c);
        assert_eq!(windows.last_active(), ROOT);
        windows.touch(99);
        assert_eq!(windows.last_active(), ROOT);
    }

    #[test]
    fn a_folder_is_found_in_whichever_window_holds_it() {
        let (_root, d) = dirs(4);
        let (windows, other) = two_windows(&d, 2);
        assert_eq!(windows.find_folder(&d[1]), Some((ROOT, 1)));
        let shouted = PathBuf::from(d[3].to_string_lossy().to_uppercase());
        assert_eq!(windows.find_folder(&shouted), Some((other, 1)));
        assert_eq!(windows.find_folder(Path::new(r"X:\nowhere")), None);
        assert!(same_folder(&d[3], &shouted));
    }

    #[test]
    fn the_session_lists_every_window_with_its_bounds_and_the_active_one() {
        let (_root, d) = dirs(3);
        let (mut windows, other) = two_windows(&d, 1);
        let bounds = WindowBounds { x: 1, y: 2, width: 800, height: 600 };
        windows.get_mut(other).unwrap().bounds = Some(bounds);
        windows.get_mut(other).unwrap().state.activate(1);
        let session = windows.session();
        assert_eq!(session.active_window, 1);
        assert_eq!(session.windows.len(), 2);
        assert_eq!(session.windows[0], WindowSession { tabs: vec![d[0].clone()], active: 0, bounds: None });
        assert_eq!(session.windows[1], WindowSession { tabs: d[1..].to_vec(), active: 1, bounds: Some(bounds) });
    }

    #[test]
    fn restoring_skips_vanished_folders_and_drops_windows_left_empty() {
        let session = Session {
            windows: vec![
                WindowSession { tabs: vec!["gone".into()], active: 0, bounds: None },
                WindowSession {
                    tabs: vec!["a".into(), "gone".into(), "b".into()],
                    active: 2,
                    bounds: Some(WindowBounds { x: 1, y: 1, width: 5, height: 5 }),
                },
                WindowSession { tabs: vec!["c".into()], active: 0, bounds: None },
            ],
            active_window: 2,
        };
        let (plans, active) = session_plan(&session, |p| p.to_string_lossy() != "gone");
        assert_eq!(plans.len(), 2);
        assert_eq!((plans[0].folders.clone(), plans[0].active), (vec!["a".into(), "b".into()], 1));
        assert_eq!(plans[0].bounds.map(|b| b.x), Some(1));
        assert_eq!(plans[1].folders, [PathBuf::from("c")]);
        assert_eq!(active, 1);

        let (none, active) = session_plan(&session, |_| false);
        assert!(none.is_empty());
        assert_eq!(active, 0);
        let stale = Session { active_window: 0, ..session };
        assert_eq!(session_plan(&stale, |p| p.to_string_lossy() != "gone").1, 0);
    }

    #[test]
    fn a_placement_is_trusted_only_after_it_has_held() {
        let target = WindowBounds { x: 0, y: 0, width: 10, height: 10 };
        let mut placement = Placement::new(target);
        assert_eq!(placement.step(false), Step::Correct);
        assert_eq!(placement.step(true), Step::Wait);
        assert_eq!(placement.step(false), Step::Correct);
        assert_eq!(placement.step(true), Step::Wait);
        std::thread::sleep(SETTLE_HOLD + Duration::from_millis(50));
        assert_eq!(placement.step(true), Step::Done);
        let mut stubborn = Placement::new(target);
        for _ in 0..SETTLE_ATTEMPTS {
            assert_eq!(stubborn.step(false), Step::Correct);
        }
        assert_eq!(stubborn.step(false), Step::Done);
    }

    #[test]
    fn a_new_tab_opened_anywhere_gets_unique_ids() {
        let (_root, d) = dirs(3);
        let windows = Windows::new(state_of(&d));
        let mut ids: Vec<u64> = windows.get(ROOT).unwrap().state.tabs.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }
}
