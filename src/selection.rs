use std::collections::BTreeSet;

#[derive(Default, Debug, Clone)]
pub struct Selection {
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    focus: Option<usize>,
}

impl Selection {
    pub fn click(&mut self, idx: usize) {
        self.selected.clear();
        self.selected.insert(idx);
        self.anchor = Some(idx);
        self.focus = Some(idx);
    }

    pub fn ctrl_click(&mut self, idx: usize) {
        if !self.selected.remove(&idx) {
            self.selected.insert(idx);
        }
        self.anchor = Some(idx);
        self.focus = Some(idx);
    }

    pub fn shift_click(&mut self, idx: usize) {
        let Some(anchor) = self.anchor else {
            self.click(idx);
            return;
        };
        self.selected = (anchor.min(idx)..=anchor.max(idx)).collect();
        self.focus = Some(idx);
    }

    pub fn move_to(&mut self, idx: usize, extend: bool) {
        if extend {
            self.shift_click(idx);
        } else {
            self.click(idx);
        }
    }

    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.selected.contains(&idx)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Translates positions after the list changed; `None` means the tile is gone.
    /// A dropped focus falls back to the selected position nearest its old value.
    pub fn remap(&mut self, map: impl Fn(usize) -> Option<usize>) {
        let old_focus = self.focus;
        self.selected = self.selected.iter().filter_map(|&i| map(i)).collect();
        self.anchor = self.anchor.and_then(&map);
        self.focus = old_focus.and_then(&map).or_else(|| {
            let target = old_focus?;
            self.selected.iter().copied().min_by_key(|&i| i.abs_diff(target))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &Selection) -> Vec<usize> {
        s.selected().collect()
    }

    #[test]
    fn click_selects_one() {
        let mut s = Selection::default();
        s.click(3);
        s.click(5);
        assert_eq!(sel(&s), vec![5]);
        assert_eq!(s.focus(), Some(5));
        assert_eq!(s.len(), 1);
        assert!(s.contains(5) && !s.contains(3));
    }

    #[test]
    fn ctrl_click_toggles() {
        let mut s = Selection::default();
        s.click(1);
        s.ctrl_click(4);
        assert_eq!(sel(&s), vec![1, 4]);
        s.ctrl_click(4);
        assert_eq!(sel(&s), vec![1]);
        assert_eq!(s.focus(), Some(4));
    }

    #[test]
    fn shift_click_range_from_anchor() {
        let mut s = Selection::default();
        s.click(2);
        s.shift_click(5);
        assert_eq!(sel(&s), vec![2, 3, 4, 5]);
        s.shift_click(3);
        assert_eq!(sel(&s), vec![2, 3]);
        assert_eq!(s.focus(), Some(3));
    }

    #[test]
    fn shift_click_without_anchor_acts_like_click() {
        let mut s = Selection::default();
        s.shift_click(4);
        assert_eq!(sel(&s), vec![4]);
    }

    #[test]
    fn shift_click_backwards() {
        let mut s = Selection::default();
        s.click(6);
        s.shift_click(3);
        assert_eq!(sel(&s), vec![3, 4, 5, 6]);
        assert_eq!(s.focus(), Some(3));
    }

    #[test]
    fn move_extend_keeps_anchor() {
        let mut s = Selection::default();
        s.click(4);
        s.move_to(5, true);
        s.move_to(6, true);
        assert_eq!(sel(&s), vec![4, 5, 6]);
        s.move_to(3, true);
        assert_eq!(sel(&s), vec![3, 4]);
        s.move_to(8, false);
        assert_eq!(sel(&s), vec![8]);
        assert_eq!(s.focus(), Some(8));
    }

    #[test]
    fn remap_drops_deleted_and_shifts() {
        let mut s = Selection::default();
        s.click(1);
        s.shift_click(4); // {1,2,3,4}, anchor 1, focus 4
        s.remap(|i| match i {
            2 => None,
            i if i > 2 => Some(i - 1),
            i => Some(i),
        });
        assert_eq!(sel(&s), vec![1, 2, 3]);
        assert_eq!(s.focus(), Some(3));
        s.shift_click(0);
        assert_eq!(sel(&s), vec![0, 1]);
    }

    #[test]
    fn remap_dropped_focus_falls_back_to_nearest_selected() {
        let mut s = Selection::default();
        s.click(2);
        s.ctrl_click(9);
        s.ctrl_click(5); // focus 5
        s.remap(|i| if i == 5 { None } else { Some(i) });
        assert_eq!(sel(&s), vec![2, 9]);
        assert_eq!(s.focus(), Some(2));
    }

    #[test]
    fn remap_dropped_anchor_and_all_selected() {
        let mut s = Selection::default();
        s.click(2);
        s.remap(|_| None);
        assert!(s.is_empty());
        assert_eq!(s.focus(), None);
        s.shift_click(3);
        assert_eq!(sel(&s), vec![3]);
    }
}
