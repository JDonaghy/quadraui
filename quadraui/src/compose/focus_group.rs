//! `FocusGroup` — a tiny helper for Tab-cycling between N focusable
//! regions by index.
//!
//! Unlike [`FocusRing`](super::FocusRing) (which tracks [`WidgetId`]s
//! and starts focused), `FocusGroup` works with plain indices, starts
//! unfocused (`None`), and supports dynamic `count` changes.
//!
//! Typical use: sidebar sections, panel layouts, dialog tab order.

/// Manages Tab-cycling across N focusable regions with wrap-around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusGroup {
    count: usize,
    active: Option<usize>,
}

impl FocusGroup {
    pub fn new(count: usize) -> Self {
        Self {
            count,
            active: None,
        }
    }

    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn set_active(&mut self, idx: Option<usize>) {
        self.active = idx;
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Update the region count. Clamps `active` if it would be out of bounds.
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        if let Some(idx) = self.active {
            if count == 0 {
                self.active = None;
            } else if idx >= count {
                self.active = Some(count - 1);
            }
        }
    }

    /// Cycle by `delta` (typically +1 for Tab, -1 for Shift+Tab).
    /// Wraps around. If `active` is `None`, +delta starts at 0,
    /// -delta starts at last.
    pub fn cycle(&mut self, delta: isize) {
        let n = self.count as isize;
        if n == 0 {
            return;
        }
        let next = match self.active {
            Some(i) => ((i as isize + delta).rem_euclid(n)) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    (n - 1) as usize
                }
            }
        };
        self.active = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_unfocused() {
        let fg = FocusGroup::new(3);
        assert_eq!(fg.active(), None);
        assert_eq!(fg.count(), 3);
    }

    #[test]
    fn cycle_forward_from_none_starts_at_zero() {
        let mut fg = FocusGroup::new(3);
        fg.cycle(1);
        assert_eq!(fg.active(), Some(0));
    }

    #[test]
    fn cycle_backward_from_none_starts_at_last() {
        let mut fg = FocusGroup::new(3);
        fg.cycle(-1);
        assert_eq!(fg.active(), Some(2));
    }

    #[test]
    fn cycle_forward_wraps() {
        let mut fg = FocusGroup::new(3);
        fg.cycle(1); // 0
        fg.cycle(1); // 1
        fg.cycle(1); // 2
        fg.cycle(1); // wraps to 0
        assert_eq!(fg.active(), Some(0));
    }

    #[test]
    fn cycle_backward_wraps() {
        let mut fg = FocusGroup::new(3);
        fg.set_active(Some(0));
        fg.cycle(-1);
        assert_eq!(fg.active(), Some(2));
    }

    #[test]
    fn cycle_on_empty_is_noop() {
        let mut fg = FocusGroup::new(0);
        fg.cycle(1);
        assert_eq!(fg.active(), None);
    }

    #[test]
    fn set_count_clamps_active() {
        let mut fg = FocusGroup::new(5);
        fg.set_active(Some(4));
        fg.set_count(3);
        assert_eq!(fg.active(), Some(2));
    }

    #[test]
    fn set_count_to_zero_clears_active() {
        let mut fg = FocusGroup::new(3);
        fg.set_active(Some(1));
        fg.set_count(0);
        assert_eq!(fg.active(), None);
    }

    #[test]
    fn set_count_preserves_valid_active() {
        let mut fg = FocusGroup::new(5);
        fg.set_active(Some(2));
        fg.set_count(4);
        assert_eq!(fg.active(), Some(2));
    }

    #[test]
    fn set_active_and_read_back() {
        let mut fg = FocusGroup::new(3);
        fg.set_active(Some(1));
        assert_eq!(fg.active(), Some(1));
        fg.set_active(None);
        assert_eq!(fg.active(), None);
    }

    #[test]
    fn single_item_wraps_to_self() {
        let mut fg = FocusGroup::new(1);
        fg.cycle(1);
        assert_eq!(fg.active(), Some(0));
        fg.cycle(1);
        assert_eq!(fg.active(), Some(0));
        fg.cycle(-1);
        assert_eq!(fg.active(), Some(0));
    }
}
