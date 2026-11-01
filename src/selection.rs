use std::collections::BTreeSet;
#[derive(Default, Debug, Clone)]
pub struct Selection {
    pub items: BTreeSet<usize>,
    pub focus: Option<usize>,
    anchor: Option<usize>,
}
pub struct Saved {
    paths: std::collections::HashSet<std::path::PathBuf>,
    focus: Option<std::path::PathBuf>,
}
impl Selection {
    pub fn replace(&mut self, items: BTreeSet<usize>, focus: Option<usize>) {
        self.items = items;
        self.focus = focus
            .filter(|n| self.items.contains(n))
            .or(self.items.first().copied());
        self.anchor = self.focus;
    }
    pub fn save(&self, rows: &[crate::background::Row]) -> Saved {
        Saved {
            paths: self
                .items
                .iter()
                .filter_map(|&n| rows.get(n).and_then(|r| r.path.clone()))
                .collect(),
            focus: self
                .focus
                .and_then(|n| rows.get(n))
                .and_then(|r| r.path.clone()),
        }
    }
    pub fn restore(&mut self, saved: Saved, rows: &[crate::background::Row]) {
        self.items = rows
            .iter()
            .enumerate()
            .filter_map(|(n, r)| {
                r.path
                    .as_ref()
                    .is_some_and(|p| saved.paths.contains(p))
                    .then_some(n)
            })
            .collect();
        self.focus = rows
            .iter()
            .position(|r| {
                r.path
                    .as_ref()
                    .is_some_and(|p| saved.focus.as_ref() == Some(p))
            })
            .or(self.items.first().copied());
        self.anchor = self.focus;
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn click(&mut self, n: usize, ctrl: bool, shift: bool, right: bool) {
        if right && self.items.contains(&n) {
            self.focus = Some(n);
            return;
        }
        if right {
            self.single(n);
            return;
        }
        if shift {
            let anchor = self.anchor.or(self.focus).unwrap_or(n);
            if !ctrl {
                self.items.clear();
            }
            self.items.extend(anchor.min(n)..=anchor.max(n));
            self.anchor = Some(anchor);
            self.focus = Some(n);
        } else if ctrl {
            if !self.items.remove(&n) {
                self.items.insert(n);
            }
            self.anchor = Some(n);
            self.focus = Some(n);
        } else {
            self.single(n);
        }
    }
    pub fn single(&mut self, n: usize) {
        self.items.clear();
        self.items.insert(n);
        self.focus = Some(n);
        self.anchor = Some(n);
    }
    pub fn navigate(&mut self, n: usize, ctrl: bool, shift: bool) {
        if ctrl && !shift {
            self.focus = Some(n);
        } else {
            self.click(n, ctrl, shift, false);
        }
    }
    pub fn all(&mut self, count: usize) {
        self.items = (0..count).collect();
        self.focus = self
            .focus
            .filter(|n| *n < count)
            .or((count > 0).then_some(0));
        self.anchor = self.focus;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_ranges_right_click_and_keyboard_focus() {
        let mut s = Selection::default();
        s.click(2, false, false, false);
        s.click(5, true, false, false);
        assert_eq!(s.items, BTreeSet::from([2, 5]));
        s.click(5, false, false, true);
        assert_eq!(s.items.len(), 2);
        s.click(7, false, true, false);
        assert_eq!(s.items, BTreeSet::from([5, 6, 7]));
        s.click(1, true, true, false);
        assert_eq!(s.items, BTreeSet::from([1, 2, 3, 4, 5, 6, 7]));
        s.click(3, true, false, false);
        assert!(!s.items.contains(&3));
        s.navigate(8, true, false);
        assert_eq!(s.focus, Some(8));
        assert_eq!(s.items.len(), 6);
        s.click(9, false, false, true);
        assert_eq!(s.items, BTreeSet::from([9]));
        s.all(5000);
        assert_eq!(s.items.len(), 5000);
        s.clear();
        assert!(s.items.is_empty());
    }
}
