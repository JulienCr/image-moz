use std::time::SystemTime;

use eframe::egui::{Rect, pos2, vec2};
use serde::{Deserialize, Serialize};

pub const GAP: f32 = 4.0;
pub const HEADER_H: f32 = 32.0;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum LayoutKind {
    #[default]
    Grid,
    Justified,
    Bento,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum BentoRule {
    #[default]
    Recent,
    Pinned,
    Resolution,
}

#[derive(Clone, Debug)]
pub struct LayoutItem {
    pub id: usize,
    pub aspect: f32,
    pub mtime: SystemTime,
    pub pixels: u64,
    pub pinned: bool,
}

#[derive(Clone, Debug)]
pub struct LayoutSection {
    pub header: Option<String>,
    pub collapsed: bool,
    pub items: Vec<LayoutItem>,
}

#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub id: usize,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug)]
pub struct HeaderRect {
    pub section: usize,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub tiles: Vec<Placed>,
    pub headers: Vec<HeaderRect>,
    pub height: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Lays out all sections top to bottom; coordinates are relative to the content top-left.
pub fn compute(
    kind: LayoutKind,
    rule: BentoRule,
    sections: &[LayoutSection],
    width: f32,
    target: f32,
) -> Layout {
    let mut out = Layout::default();
    let mut y = 0.0_f32;
    for (si, sec) in sections.iter().enumerate() {
        if si > 0 {
            y += GAP;
        }
        if sec.header.is_some() {
            out.headers.push(HeaderRect {
                section: si,
                rect: Rect::from_min_size(pos2(0.0, y), vec2(width, HEADER_H)),
            });
            y += HEADER_H;
        }
        if sec.collapsed || sec.items.is_empty() {
            continue;
        }
        if sec.header.is_some() {
            y += GAP;
        }
        let h = match kind {
            LayoutKind::Grid => grid(&sec.items, width, target, y, &mut out.tiles),
            LayoutKind::Justified => justified(&sec.items, width, target, y, &mut out.tiles),
            LayoutKind::Bento => bento(&sec.items, rule, width, target, y, &mut out.tiles),
        };
        y += h;
    }
    out.height = y;
    out
}

/// Top of a section header drawn pinned to the viewport while its section scrolls;
/// the next header pushes it up, and it never rises above its natural position.
pub fn sticky_y(natural_y: f32, next_header_y: Option<f32>, viewport_top: f32) -> f32 {
    let ceiling = next_header_y.map_or(f32::INFINITY, |next| next - HEADER_H);
    natural_y.max(viewport_top.min(ceiling))
}

fn column_count(width: f32, target: f32, min: usize) -> usize {
    (((width + GAP) / (target + GAP)).floor() as usize).max(min)
}

fn cell_size(width: f32, cols: usize) -> f32 {
    (width - (cols - 1) as f32 * GAP) / cols as f32
}

fn span(n: usize, cell: f32) -> f32 {
    n as f32 * cell + (n - 1) as f32 * GAP
}

fn grid(items: &[LayoutItem], width: f32, target: f32, y0: f32, tiles: &mut Vec<Placed>) -> f32 {
    let cols = column_count(width, target, 1);
    let side = cell_size(width, cols);
    for (i, it) in items.iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let min = pos2(c as f32 * (side + GAP), y0 + r as f32 * (side + GAP));
        tiles.push(Placed {
            id: it.id,
            rect: Rect::from_min_size(min, vec2(side, side)),
        });
    }
    span(items.len().div_ceil(cols), side)
}

fn emit_row(row: &[LayoutItem], h: f32, y: f32, tiles: &mut Vec<Placed>) {
    let mut x = 0.0;
    for it in row {
        let w = it.aspect.max(0.01) * h;
        tiles.push(Placed {
            id: it.id,
            rect: Rect::from_min_size(pos2(x, y), vec2(w, h)),
        });
        x += w + GAP;
    }
}

fn justified(
    items: &[LayoutItem],
    width: f32,
    target: f32,
    y0: f32,
    tiles: &mut Vec<Placed>,
) -> f32 {
    let mut y = y0;
    let mut start = 0;
    let mut sum_aspect = 0.0_f32;
    for (i, it) in items.iter().enumerate() {
        sum_aspect += it.aspect.max(0.01);
        let n = i + 1 - start;
        if sum_aspect * target + (n - 1) as f32 * GAP >= width {
            let h = (width - (n - 1) as f32 * GAP) / sum_aspect;
            emit_row(&items[start..=i], h, y, tiles);
            y += h + GAP;
            start = i + 1;
            sum_aspect = 0.0;
        }
    }
    if start < items.len() {
        emit_row(&items[start..], target, y, tiles);
        y += target + GAP;
    }
    y - GAP - y0
}

struct Occupancy {
    cols: usize,
    cells: Vec<bool>,
    first_free: usize,
}

impl Occupancy {
    fn ensure_rows(&mut self, rows: usize) {
        if self.cells.len() < rows * self.cols {
            self.cells.resize(rows * self.cols, false);
        }
    }

    fn fits(&mut self, c: usize, r: usize, w: usize, h: usize) -> bool {
        if c + w > self.cols {
            return false;
        }
        self.ensure_rows(r + h);
        (r..r + h).all(|rr| (c..c + w).all(|cc| !self.cells[rr * self.cols + cc]))
    }

    fn take(&mut self, c: usize, r: usize, w: usize, h: usize) {
        for rr in r..r + h {
            for cc in c..c + w {
                self.cells[rr * self.cols + cc] = true;
            }
        }
        while self.first_free < self.cells.len() && self.cells[self.first_free] {
            self.first_free += 1;
        }
    }
}

fn bento(
    items: &[LayoutItem],
    rule: BentoRule,
    width: f32,
    target: f32,
    y0: f32,
    tiles: &mut Vec<Placed>,
) -> f32 {
    let cols = column_count(width, target, 2);
    let cell = cell_size(width, cols);
    let mut featured: Vec<usize> = (0..items.len()).collect();
    match rule {
        BentoRule::Recent => featured.sort_by_key(|&i| std::cmp::Reverse(items[i].mtime)),
        BentoRule::Resolution => featured.sort_by_key(|&i| std::cmp::Reverse(items[i].pixels)),
        BentoRule::Pinned => featured.sort_by_key(|&i| !items[i].pinned),
    }
    let mut placed = vec![false; items.len()];
    let (mut next_featured, mut next_plain) = (0, 0);
    let mut occ = Occupancy {
        cols,
        cells: Vec::new(),
        first_free: 0,
    };
    let mut rows_used = 0;
    for k in 0..items.len() {
        let big = k % 7 == 0;
        let idx = if big {
            while placed[featured[next_featured]] {
                next_featured += 1;
            }
            featured[next_featured]
        } else {
            while placed[next_plain] {
                next_plain += 1;
            }
            next_plain
        };
        placed[idx] = true;
        let (c, r, w, h) = if big {
            let mut pos = occ.first_free;
            loop {
                let (c, r) = (pos % cols, pos / cols);
                if occ.fits(c, r, 2, 2) {
                    break (c, r, 2, 2);
                }
                pos += 1;
            }
        } else {
            let (c, r) = (occ.first_free % cols, occ.first_free / cols);
            let wide = items[idx].aspect >= 1.0;
            let (first, second) = if wide { ((2, 1), (1, 2)) } else { ((1, 2), (2, 1)) };
            let shape = [first, second, (1, 1)]
                .into_iter()
                .find(|&(w, h)| occ.fits(c, r, w, h))
                .unwrap_or((1, 1));
            (c, r, shape.0, shape.1)
        };
        occ.ensure_rows(r + h);
        occ.take(c, r, w, h);
        rows_used = rows_used.max(r + h);
        let min = pos2(c as f32 * (cell + GAP), y0 + r as f32 * (cell + GAP));
        tiles.push(Placed {
            id: items[idx].id,
            rect: Rect::from_min_size(min, vec2(span(w, cell), span(h, cell))),
        });
    }
    span(rows_used.max(1), cell)
}

/// Picks the nearest tile strictly in `dir` from `from`'s center; cross-axis offset costs double.
pub fn neighbor(layout: &Layout, from: usize, dir: Dir) -> Option<usize> {
    let origin = layout.tiles.iter().find(|t| t.id == from)?.rect.center();
    layout
        .tiles
        .iter()
        .filter(|t| t.id != from)
        .filter_map(|t| {
            let d = t.rect.center() - origin;
            let (primary, cross) = match dir {
                Dir::Right => (d.x, d.y.abs()),
                Dir::Left => (-d.x, d.y.abs()),
                Dir::Down => (d.y, d.x.abs()),
                Dir::Up => (-d.y, d.x.abs()),
            };
            (primary > 0.0).then_some((primary + 2.0 * cross, t.id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn item(id: usize, aspect: f32) -> LayoutItem {
        LayoutItem {
            id,
            aspect,
            mtime: UNIX_EPOCH + Duration::from_secs(id as u64),
            pixels: id as u64,
            pinned: false,
        }
    }

    fn section(header: Option<&str>, collapsed: bool, items: Vec<LayoutItem>) -> LayoutSection {
        LayoutSection {
            header: header.map(String::from),
            collapsed,
            items,
        }
    }

    fn square_items(n: usize) -> Vec<LayoutItem> {
        (0..n).map(|i| item(i, 1.0)).collect()
    }

    fn rect_of(l: &Layout, id: usize) -> Rect {
        l.tiles.iter().find(|t| t.id == id).unwrap().rect
    }

    #[test]
    fn grid_columns_and_fill() {
        let s = [section(None, false, square_items(9))];
        let l = compute(LayoutKind::Grid, BentoRule::Recent, &s, 1000.0, 200.0);
        let side = (1000.0 - 3.0 * 4.0) / 4.0;
        assert_eq!(rect_of(&l, 0).width(), side);
        assert_eq!(rect_of(&l, 3).max.x, 1000.0);
        assert_eq!(rect_of(&l, 4).min.y, side + GAP);
        assert_eq!(l.height, 3.0 * side + 2.0 * GAP);
    }

    #[test]
    fn grid_collapsed_section_header_only() {
        let s = [section(Some("A"), true, square_items(5))];
        let l = compute(LayoutKind::Grid, BentoRule::Recent, &s, 1000.0, 200.0);
        assert!(l.tiles.is_empty());
        assert_eq!(l.headers.len(), 1);
        assert_eq!(l.headers[0].rect.height(), HEADER_H);
        assert_eq!(l.height, HEADER_H);
    }

    fn mixed_aspects(n: usize) -> Vec<LayoutItem> {
        (0..n)
            .map(|i| item(i, [1.5, 0.7, 1.0, 2.0][i % 4]))
            .collect()
    }

    #[test]
    fn justified_rows_fill_width() {
        let s = [section(None, false, mixed_aspects(30))];
        let l = compute(LayoutKind::Justified, BentoRule::Recent, &s, 1000.0, 150.0);
        let last_y = l.tiles.last().unwrap().rect.min.y;
        let mut checked = 0;
        for y in l.tiles.iter().map(|t| t.rect.min.y).filter(|&y| y < last_y) {
            let right = l
                .tiles
                .iter()
                .filter(|t| t.rect.min.y == y)
                .map(|t| t.rect.max.x)
                .fold(0.0, f32::max);
            assert!((right - 1000.0).abs() <= 0.5, "row at {y} ends at {right}");
            checked += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn justified_last_row_keeps_target_height() {
        let s = [section(None, false, mixed_aspects(30))];
        let l = compute(LayoutKind::Justified, BentoRule::Recent, &s, 1000.0, 150.0);
        let last = l.tiles.last().unwrap().rect;
        assert_eq!(last.height(), 150.0);
        assert!(last.max.x < 1000.0);
        assert_eq!(l.height, last.max.y);
    }

    #[test]
    fn bento_first_tile_is_2x2_per_section() {
        let s = [
            section(Some("A"), false, square_items(10)),
            section(Some("B"), false, (100..110).map(|i| item(i, 1.0)).collect()),
        ];
        let l = compute(LayoutKind::Bento, BentoRule::Recent, &s, 1000.0, 200.0);
        let cell = (1000.0 - 3.0 * GAP) / 4.0;
        for first in [9, 109] {
            let r = rect_of(&l, first);
            assert!((r.width() - (2.0 * cell + GAP)).abs() < 0.01);
            assert!((r.height() - (2.0 * cell + GAP)).abs() < 0.01);
        }
        assert!(rect_of(&l, 109).min.y > rect_of(&l, 9).max.y);
    }

    #[test]
    fn bento_landscape_2x1_portrait_1x2() {
        let items = vec![item(0, 1.0), item(1, 2.0), item(2, 0.5)];
        let s = [section(None, false, items)];
        let l = compute(LayoutKind::Bento, BentoRule::Pinned, &s, 1000.0, 200.0);
        let cell = (1000.0 - 3.0 * GAP) / 4.0;
        let wide = rect_of(&l, 1);
        assert!((wide.width() - (2.0 * cell + GAP)).abs() < 0.01);
        assert!((wide.height() - cell).abs() < 0.01);
        let tall = rect_of(&l, 2);
        assert!((tall.width() - cell).abs() < 0.01);
        assert!((tall.height() - (2.0 * cell + GAP)).abs() < 0.01);
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn bento_no_overlap_and_within_width() {
        let mut rng = Rng(0x2545F4914F6CDD1D);
        let items: Vec<_> = (0..200)
            .map(|i| item(i, 0.4 + (rng.next() % 200) as f32 / 100.0))
            .collect();
        let s = [section(None, false, items)];
        for rule in [BentoRule::Recent, BentoRule::Pinned, BentoRule::Resolution] {
            let l = compute(LayoutKind::Bento, rule, &s, 1000.0, 200.0);
            assert_eq!(l.tiles.len(), 200);
            for (i, a) in l.tiles.iter().enumerate() {
                assert!(a.rect.min.x >= 0.0 && a.rect.max.x <= 1000.01);
                assert!(a.rect.max.y <= l.height + 0.01);
                for b in &l.tiles[i + 1..] {
                    assert!(
                        !a.rect.shrink(0.5).intersects(b.rect.shrink(0.5)),
                        "{} overlaps {}",
                        a.id,
                        b.id
                    );
                }
            }
        }
    }

    #[test]
    fn bento_rule_resolution_puts_largest_in_big_slot() {
        let mut items = square_items(8);
        items[5].pixels = 1_000_000;
        let s = [section(None, false, items)];
        let l = compute(LayoutKind::Bento, BentoRule::Resolution, &s, 1000.0, 200.0);
        let big = rect_of(&l, 5);
        assert_eq!((big.min.x, big.min.y), (0.0, 0.0));
        assert!(big.width() > 400.0 && big.height() > 400.0);
    }

    #[test]
    fn bento_rule_pinned_puts_pinned_in_big_slot() {
        let mut items = square_items(8);
        items[6].pinned = true;
        let s = [section(None, false, items)];
        let l = compute(LayoutKind::Bento, BentoRule::Pinned, &s, 1000.0, 200.0);
        let big = rect_of(&l, 6);
        assert_eq!((big.min.x, big.min.y), (0.0, 0.0));
        assert!(big.width() > 400.0 && big.height() > 400.0);
        assert!(rect_of(&l, 0).min.x > 0.0 || rect_of(&l, 0).min.y > 0.0);
    }

    #[test]
    fn neighbor_grid_arrows() {
        let s = [section(None, false, square_items(12))];
        let l = compute(LayoutKind::Grid, BentoRule::Recent, &s, 1000.0, 200.0);
        assert_eq!(neighbor(&l, 0, Dir::Right), Some(1));
        assert_eq!(neighbor(&l, 1, Dir::Down), Some(5));
        assert_eq!(neighbor(&l, 5, Dir::Up), Some(1));
        assert_eq!(neighbor(&l, 1, Dir::Left), Some(0));
        assert_eq!(neighbor(&l, 0, Dir::Left), None);
        assert_eq!(neighbor(&l, 0, Dir::Up), None);
    }

    #[test]
    fn neighbor_crosses_section_boundary() {
        let s = [
            section(Some("A"), false, square_items(4)),
            section(Some("B"), false, (10..14).map(|i| item(i, 1.0)).collect()),
        ];
        let l = compute(LayoutKind::Grid, BentoRule::Recent, &s, 1000.0, 200.0);
        assert_eq!(neighbor(&l, 2, Dir::Down), Some(12));
        assert_eq!(neighbor(&l, 12, Dir::Up), Some(2));
    }

    #[test]
    fn header_sticks_to_the_viewport_until_the_next_one_pushes_it() {
        assert_eq!(sticky_y(100.0, Some(900.0), 0.0), 100.0);
        assert_eq!(sticky_y(100.0, Some(900.0), 400.0), 400.0);
        assert_eq!(sticky_y(100.0, Some(900.0), 880.0), 900.0 - HEADER_H);
        assert_eq!(sticky_y(100.0, Some(900.0), 2000.0), 900.0 - HEADER_H);
        assert_eq!(sticky_y(100.0, None, 2000.0), 2000.0);
    }
}
