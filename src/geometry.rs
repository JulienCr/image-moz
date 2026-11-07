//! Pure geometry for tab drags and window placement; no widgets and no Win32, so it is testable.

use eframe::egui::{Pos2, Rect, pos2};

use crate::config::WindowBounds;

/// Window frame that surrounds the inner size at 100% scale: side borders plus the title bar.
const FRAME_PX: (i32, i32) = (16, 39);
/// Where a detached window puts the cursor, from its outer top-left, at 100% scale.
const DETACH_GRAB_PX: (f32, f32) = (80.0, 46.0);

/// Index in `0..=tabs.len()` a tab released at `x` would be inserted at.
pub fn insertion_index(tabs: &[Rect], x: f32) -> usize {
    tabs.iter().take_while(|r| r.center().x < x).count()
}

/// Final index of tab `from` once it is re-inserted at `insertion` of the list that still holds it.
pub fn reorder_target(from: usize, insertion: usize) -> usize {
    if insertion > from { insertion - 1 } else { insertion }
}

/// Maps a screen-space cursor in physical pixels to a window's own point space.
pub fn to_local(cursor_px: Pos2, inner_origin_px: Pos2, scale: f32) -> Pos2 {
    pos2((cursor_px.x - inner_origin_px.x) / scale, (cursor_px.y - inner_origin_px.y) / scale)
}

/// Where a window's tab bar and tabs were drawn, plus what is needed to compare it with the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct BarGeometry {
    pub bar: Rect,
    pub tabs: Vec<Rect>,
    /// Top-left of the window's drawing area, in screen pixels.
    pub origin_px: Pos2,
    /// Physical pixels per point of that window; it differs between monitors.
    pub scale: f32,
}

impl BarGeometry {
    pub fn local(&self, cursor_px: Pos2) -> Pos2 {
        to_local(cursor_px, self.origin_px, self.scale)
    }

    /// Insertion index when the cursor is over this bar.
    pub fn drop_index(&self, cursor_px: Pos2) -> Option<usize> {
        let local = self.local(cursor_px);
        self.bar.contains(local).then(|| insertion_index(&self.tabs, local.x))
    }
}

/// The first allowed bar under the cursor, with the insertion index it offers.
pub fn drop_target<K: Copy>(
    bars: &[(K, BarGeometry)],
    cursor_px: Pos2,
    allowed: impl Fn(K) -> bool,
) -> Option<(K, usize)> {
    bars.iter().filter(|(key, _)| allowed(*key)).find_map(|(key, bar)| Some((*key, bar.drop_index(cursor_px)?)))
}

/// A monitor work area in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    fn overlap_area(&self, other: &PxRect) -> i64 {
        let w = (self.right.min(other.right) - self.left.max(other.left)).max(0);
        let h = (self.bottom.min(other.bottom) - self.top.max(other.top)).max(0);
        i64::from(w) * i64::from(h)
    }
}

/// Where and how large to open a restored window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fitted {
    /// `None` leaves the placement to the system.
    pub position: Option<(i32, i32)>,
    pub size: (u32, u32),
}

fn scaled(px: i32, scale: f32) -> i32 {
    (px as f32 * scale).round() as i32
}

/// Keeps a stored window on the monitor it overlaps most; one on no monitor loses its position.
pub fn fit_to_monitors(saved: WindowBounds, work: &[PxRect], scale: f32) -> Fitted {
    let size = (saved.width.max(1), saved.height.max(1));
    let Some(primary) = work.first() else {
        return Fitted { position: Some((saved.x, saved.y)), size };
    };
    let (frame_w, frame_h) = (scaled(FRAME_PX.0, scale), scaled(FRAME_PX.1, scale));
    let shrink = |monitor: &PxRect| {
        let w = (size.0 as i32).min((monitor.width() - frame_w).max(1));
        let h = (size.1 as i32).min((monitor.height() - frame_h).max(1));
        (w as u32, h as u32)
    };
    let outer = PxRect {
        left: saved.x,
        top: saved.y,
        right: saved.x.saturating_add(size.0 as i32 + frame_w),
        bottom: saved.y.saturating_add(size.1 as i32 + frame_h),
    };
    let best = work.iter().max_by_key(|m| m.overlap_area(&outer)).unwrap_or(primary);
    if best.overlap_area(&outer) == 0 {
        return Fitted { position: None, size: shrink(primary) };
    }
    let size = shrink(best);
    let (outer_w, outer_h) = (size.0 as i32 + frame_w, size.1 as i32 + frame_h);
    let x = saved.x.clamp(best.left, (best.right - outer_w).max(best.left));
    let y = saved.y.clamp(best.top, (best.bottom - outer_h).max(best.top));
    Fitted { position: Some((x, y)), size }
}

/// Bounds for a window detached with the cursor at `cursor_px`, keeping the source's inner size.
pub fn detach_bounds(cursor_px: (i32, i32), inner_px: (u32, u32), scale: f32) -> WindowBounds {
    WindowBounds {
        x: cursor_px.0 - (DETACH_GRAB_PX.0 * scale).round() as i32,
        y: cursor_px.1 - (DETACH_GRAB_PX.1 * scale).round() as i32,
        width: inner_px.0,
        height: inner_px.1,
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{pos2, vec2};

    use super::*;

    fn tab_rects(n: usize) -> Vec<Rect> {
        (0..n).map(|i| Rect::from_min_size(pos2(10.0 + i as f32 * 100.0, 4.0), vec2(90.0, 24.0))).collect()
    }

    fn bounds(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
        WindowBounds { x, y, width, height }
    }

    const SCREEN: PxRect = PxRect { left: 0, top: 0, right: 1920, bottom: 1040 };

    #[test]
    fn insertion_index_is_the_gap_nearest_the_cursor() {
        let tabs = tab_rects(3);
        assert_eq!(insertion_index(&tabs, -50.0), 0);
        assert_eq!(insertion_index(&tabs, 40.0), 0);
        assert_eq!(insertion_index(&tabs, 60.0), 1);
        assert_eq!(insertion_index(&tabs, 150.0), 1);
        assert_eq!(insertion_index(&tabs, 170.0), 2);
        assert_eq!(insertion_index(&tabs, 900.0), 3);
        assert_eq!(insertion_index(&[], 10.0), 0);
    }

    #[test]
    fn reordering_accounts_for_the_dragged_tab_leaving_its_slot() {
        assert_eq!(reorder_target(1, 0), 0);
        assert_eq!(reorder_target(1, 1), 1);
        assert_eq!(reorder_target(1, 2), 1);
        assert_eq!(reorder_target(1, 3), 2);
        assert_eq!(reorder_target(0, 3), 2);
        assert_eq!(reorder_target(2, 0), 0);
    }

    #[test]
    fn cursor_conversion_uses_each_windows_own_scale() {
        let at_100 = to_local(pos2(1100.0, 300.0), pos2(1000.0, 200.0), 1.0);
        assert_eq!(at_100, pos2(100.0, 100.0));
        let at_150 = to_local(pos2(1150.0, 350.0), pos2(1000.0, 200.0), 1.5);
        assert_eq!(at_150, pos2(100.0, 100.0));
    }

    fn geometry(origin: Pos2, scale: f32) -> BarGeometry {
        BarGeometry {
            bar: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 32.0)),
            tabs: tab_rects(2),
            origin_px: origin,
            scale,
        }
    }

    #[test]
    fn a_drop_needs_the_cursor_over_the_bar_not_the_content_below_it() {
        let bar = geometry(pos2(100.0, 100.0), 1.0);
        assert_eq!(bar.drop_index(pos2(160.0, 110.0)), Some(1));
        assert_eq!(bar.drop_index(pos2(160.0, 140.0)), None);
        assert_eq!(bar.drop_index(pos2(50.0, 110.0)), None);
        let scaled = geometry(pos2(100.0, 100.0), 2.0);
        assert_eq!(scaled.drop_index(pos2(100.0 + 2.0 * 160.0, 100.0 + 2.0 * 10.0)), Some(2));
    }

    #[test]
    fn overlapping_bars_resolve_to_the_allowed_one() {
        let bars = [(1u32, geometry(pos2(0.0, 0.0), 1.0)), (2u32, geometry(pos2(0.0, 0.0), 1.0))];
        let cursor = pos2(5.0, 5.0);
        assert_eq!(drop_target(&bars, cursor, |_| true).map(|t| t.0), Some(1));
        assert_eq!(drop_target(&bars, cursor, |k| k == 2).map(|t| t.0), Some(2));
        assert_eq!(drop_target(&bars, cursor, |_| false), None);
        assert_eq!(drop_target(&bars, pos2(5.0, 500.0), |_| true), None);
    }

    #[test]
    fn a_window_on_screen_is_left_alone() {
        let fitted = fit_to_monitors(bounds(100, 80, 1200, 800), &[SCREEN], 1.0);
        assert_eq!(fitted, Fitted { position: Some((100, 80)), size: (1200, 800) });
    }

    #[test]
    fn a_window_hanging_off_an_edge_is_pulled_back_in() {
        let fitted = fit_to_monitors(bounds(1500, 900, 1200, 800), &[SCREEN], 1.0);
        assert_eq!(fitted.size, (1200, 800));
        assert_eq!(fitted.position, Some((1920 - 1200 - 16, 1040 - 800 - 39)));
    }

    #[test]
    fn a_window_on_an_unplugged_monitor_loses_its_position() {
        let fitted = fit_to_monitors(bounds(2500, 100, 1000, 700), &[SCREEN], 1.0);
        assert_eq!(fitted, Fitted { position: None, size: (1000, 700) });
        let above = fit_to_monitors(bounds(100, -3000, 1000, 700), &[SCREEN], 1.0);
        assert_eq!(above.position, None);
    }

    #[test]
    fn a_window_larger_than_the_monitor_shrinks_to_fit() {
        let fitted = fit_to_monitors(bounds(0, 0, 5000, 4000), &[SCREEN], 1.0);
        assert_eq!(fitted.size, (1920 - 16, 1040 - 39));
        assert_eq!(fitted.position, Some((0, 0)));
    }

    #[test]
    fn a_monitor_left_of_the_primary_keeps_negative_coordinates() {
        let left = PxRect { left: -1920, top: 0, right: 0, bottom: 1080 };
        let fitted = fit_to_monitors(bounds(-1500, 50, 1000, 600), &[SCREEN, left], 1.0);
        assert_eq!(fitted.position, Some((-1500, 50)));
    }

    #[test]
    fn frame_allowance_follows_the_scale() {
        let fitted = fit_to_monitors(bounds(1000, 600, 1000, 800), &[SCREEN], 2.0);
        assert_eq!(fitted.position, Some((1920 - 1000 - 32, 1040 - 800 - 78)));
    }

    #[test]
    fn unknown_monitors_trust_the_stored_bounds() {
        let fitted = fit_to_monitors(bounds(-40, 7, 900, 500), &[], 1.0);
        assert_eq!(fitted, Fitted { position: Some((-40, 7)), size: (900, 500) });
    }

    #[test]
    fn a_detached_window_opens_with_the_cursor_over_its_first_tab() {
        let b = detach_bounds((1000, 500), (1200, 800), 1.0);
        assert_eq!((b.x, b.y, b.width, b.height), (920, 454, 1200, 800));
        let b = detach_bounds((1000, 500), (1200, 800), 2.0);
        assert_eq!((b.x, b.y), (840, 408));
    }
}
