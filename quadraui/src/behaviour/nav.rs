//! List/tree navigation: [`NavKey`] turns a keystroke into a navigation
//! intent, [`FlatNav`] resolves that intent against a flat, index-addressed
//! list, and [`TreeNavCore`] resolves selection/scroll against a flattened
//! tree-shaped row list addressed by [`crate::TreePath`].

use crate::event::{Key, NamedKey};
use crate::interactive::KeyEvent;
use crate::primitives::scrollbar::{
    scroll_by_clamped, scroll_by_clamped_f32, scroll_rows_floor, wheel_delta, ThumbDrag,
};
use crate::TreePath;

use super::scroll::ensure_visible;

// ─── NavKey ──────────────────────────────────────────────────────────────

/// A keystroke resolved to a navigation intent, independent of which
/// widget or list it moves. [`NavKey::from_key`] is the one place a raw
/// [`KeyEvent`] becomes one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    /// Move one step backward.
    Up,
    /// Move one step forward.
    Down,
    /// Move `n` steps backward. [`NavKey::from_key`] can't know a list's
    /// page size (that's the caller's viewport, not the keystroke), so it
    /// maps the physical Page Up key to `PageUp(1)`; a caller navigating a
    /// paged list reconstructs `NavKey::PageUp(viewport_len)` with the
    /// real page size instead of using that placeholder directly.
    PageUp(usize),
    /// Move `n` steps forward — see [`NavKey::PageUp`]'s doc for the same
    /// placeholder-vs-real-page-size caveat.
    PageDown(usize),
    /// Move to the first item.
    Home,
    /// Move to the last item.
    End,
}

impl NavKey {
    /// Map a keystroke to a [`NavKey`], or `None` if it isn't a
    /// navigation key. The one key→intent mapping every list/tree
    /// navigation caller shares.
    pub fn from_key(event: &KeyEvent) -> Option<NavKey> {
        match event.key {
            Key::Named(NamedKey::Up) => Some(NavKey::Up),
            Key::Named(NamedKey::Down) => Some(NavKey::Down),
            Key::Named(NamedKey::PageUp) => Some(NavKey::PageUp(1)),
            Key::Named(NamedKey::PageDown) => Some(NavKey::PageDown(1)),
            Key::Named(NamedKey::Home) => Some(NavKey::Home),
            Key::Named(NamedKey::End) => Some(NavKey::End),
            _ => None,
        }
    }
}

// ─── FlatNav ─────────────────────────────────────────────────────────────

/// Stateless navigation over a flat, index-addressed list: `len` items,
/// an optional `current` index, a [`NavKey`] intent, a `wrap` flag, and an
/// `is_enabled` predicate a caller uses to skip disabled items (pass
/// `|_| true` when every item is selectable).
///
/// No current selection (`current: None`) seeds at the first enabled item
/// moving forward (`Down`/`PageDown`) or the last enabled item moving
/// backward (`Up`/`PageUp`), matching every existing clamped-index-move
/// copy this type replaces. Returns `None` only when no item is enabled
/// (including `len == 0`).
pub struct FlatNav;

impl FlatNav {
    /// Resolve `key` against a `len`-item list and return the index it
    /// moves to, skipping every item `is_enabled` rejects.
    ///
    /// `Home`/`End` land on the first/last *enabled* item. A zero-magnitude
    /// `PageUp(0)`/`PageDown(0)` moves one step, so a caller that hasn't
    /// resolved its page size yet still makes progress rather than
    /// no-opping. `None` means no item is enabled (including `len == 0`),
    /// which callers typically fold back to their current index.
    pub fn mv(
        len: usize,
        current: Option<usize>,
        key: NavKey,
        wrap: bool,
        is_enabled: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let enabled: Vec<usize> = (0..len).filter(|&i| is_enabled(i)).collect();
        if enabled.is_empty() {
            return None;
        }
        let last = enabled.len() as isize - 1;
        let pos = current.and_then(|cur| enabled.iter().position(|&i| i == cur));

        let new_pos = match key {
            NavKey::Home => 0,
            NavKey::End => last,
            NavKey::Up => Self::step(pos, -1, last, wrap),
            NavKey::Down => Self::step(pos, 1, last, wrap),
            NavKey::PageUp(n) => Self::step(pos, -(n.max(1) as isize), last, wrap),
            NavKey::PageDown(n) => Self::step(pos, n.max(1) as isize, last, wrap),
        };
        enabled.get(new_pos as usize).copied()
    }

    /// Move `pos` (an index into the filtered `enabled` list, not the raw
    /// list) by `delta`, clamped to `[0, last]` or wrapped, per `wrap`.
    /// `pos == None` (no current selection) seeds at either end,
    /// depending on `delta`'s sign, same convention `FlatNav::mv`'s doc
    /// describes.
    fn step(pos: Option<usize>, delta: isize, last: isize, wrap: bool) -> isize {
        match pos {
            Some(p) => {
                if wrap {
                    // `crate::focus::cycle_index` is the crate's one
                    // wrapping-index helper; reuse it rather than
                    // re-deriving the same `rem_euclid` here.
                    crate::focus::cycle_index(p, delta, last as usize + 1) as isize
                } else {
                    (p as isize + delta).clamp(0, last)
                }
            }
            None if delta > 0 => 0,
            None => last,
        }
    }
}

// ─── TreeNavCore ─────────────────────────────────────────────────────────

/// A flattened row exposing enough shape for [`TreeNavCore`] to navigate it,
/// without depending on `crate::TreeRow`/`crate::TreeView`.
pub trait NavRow {
    /// This row's path. Selection is tracked by path (not index) so it
    /// survives a re-flatten that shifts indices.
    fn path(&self) -> &TreePath;
    /// Visual indent level. Unused by the navigation core today, but part
    /// of the trait so a future core transition (e.g. a indent-aware
    /// Left/Right variant) doesn't need a second trait.
    fn indent(&self) -> u16;
    /// `None` marks a leaf; `Some(true)` an expanded branch; `Some(false)`
    /// a collapsed branch.
    fn is_expanded(&self) -> Option<bool>;
}

/// Which end of the row list [`TreeNavCore::move_selection_to_edge`] moves
/// the selection to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The first row.
    Start,
    /// The last row.
    End,
}

/// The outcome of a selection-moving operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavOutcome {
    /// No rows to select.
    Empty,
    /// The selection did not move (e.g. already at the clamped edge).
    Unchanged,
    /// The selection moved to this path.
    Selected(TreePath),
}

/// Direction for [`TreeNavCore::horizontal_nav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalDir {
    Left,
    Right,
}

/// What a Left/Right key should do to the selected row, per the rule
/// "Left collapses, else selects the parent; Right expands, else selects
/// the first child."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HorizontalNav {
    /// The selected row is an expanded branch — collapse it.
    Collapse(TreePath),
    /// The selected row is a leaf or already collapsed — select its parent.
    SelectParent(TreePath),
    /// The selected row is a collapsed branch — expand it.
    Expand(TreePath),
    /// The selected row is a leaf or already expanded — select its first
    /// child (the next row in flattened order whose path is this row's
    /// path with one more segment appended).
    SelectFirstChild(TreePath),
    /// Nothing to do (no selection, root row pressed Left with no parent,
    /// leaf row pressed Right with no children).
    None,
}

/// Row-agnostic navigation/selection state: selected path, row-quantised
/// scroll offset, pixel-precise scroll position, and an in-progress
/// scrollbar thumb drag.
///
/// Owns exactly the state and transitions that depend only on a flattened
/// row's `path`/`indent`/`is_expanded` shape — selection by [`TreePath`],
/// Up/Down/PageUp/PageDown/Home/End, Left/Right collapse-or-select-parent /
/// expand-or-select-first-child, scroll-to-follow, scrollbar drag, and
/// activation. It never owns row content: every method that needs the
/// current row list takes `rows: &[R]` as a parameter, so a caller never
/// constructs a throwaway controller just to navigate a borrowed view. A
/// caller's row type implements [`NavRow`] to plug into it;
/// `crate::TreeRow` does, in `compose::tree_controller`, and
/// `TreeController` is a thin layer over this core plus its own
/// `TreeView`-specific rendering, hit-testing and inline editing.
#[derive(Debug, Default)]
pub struct TreeNavCore {
    selected_path: Option<TreePath>,
    scroll_offset: usize,
    /// Pixel-precise scroll position, accumulated from fractional wheel
    /// deltas. `None` whenever the scroll position was last set by a
    /// whole-row operation — see [`TreeNavCore::scroll_px`].
    scroll_px: Option<f32>,
    scroll_drag: Option<ThumbDrag>,
}

impl TreeNavCore {
    pub fn new() -> Self {
        Self::default()
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn selected_path(&self) -> Option<&TreePath> {
        self.selected_path.as_ref()
    }

    pub fn set_selected_path(&mut self, path: Option<TreePath>) {
        self.selected_path = path;
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset;
        self.scroll_px = None;
    }

    pub fn scroll_px(&self) -> Option<f32> {
        self.scroll_px
    }

    pub fn is_dragging(&self) -> bool {
        self.scroll_drag.is_some()
    }

    // ── Selection movement ──────────────────────────────────────────

    /// Move the selection by `delta` rows (negative = up), clamped to the
    /// row list, scrolling it into view. With no current selection, a
    /// positive delta selects the first row and a non-positive delta
    /// selects the last row (matches a bare Down/Up press with nothing
    /// selected yet).
    pub fn move_selection_by<R: NavRow>(
        &mut self,
        rows: &[R],
        delta: isize,
        viewport_rows: usize,
    ) -> NavOutcome {
        if rows.is_empty() {
            return NavOutcome::Empty;
        }
        let count = rows.len();
        let current = self.selected_row_index(rows);

        let new = if let Some(cur) = current {
            let target = cur as isize + delta;
            target.max(0).min(count as isize - 1) as usize
        } else if delta > 0 {
            0
        } else {
            count - 1
        };

        if current == Some(new) {
            return NavOutcome::Unchanged;
        }

        let path = rows[new].path().clone();
        self.selected_path = Some(path.clone());
        self.scroll_to_visible(new, viewport_rows);
        NavOutcome::Selected(path)
    }

    /// Move the selection to the first (`Edge::Start`) or last (`Edge::End`)
    /// row, scrolling it into view.
    pub fn move_selection_to_edge<R: NavRow>(
        &mut self,
        rows: &[R],
        edge: Edge,
        viewport_rows: usize,
    ) -> NavOutcome {
        if rows.is_empty() {
            return NavOutcome::Empty;
        }
        let row = match edge {
            Edge::Start => 0,
            Edge::End => rows.len() - 1,
        };
        let path = rows[row].path().clone();
        self.selected_path = Some(path.clone());
        self.scroll_to_visible(row, viewport_rows);
        NavOutcome::Selected(path)
    }

    /// The currently selected row's index in `rows`, found by path (not
    /// cached) so it stays correct across a re-flatten that shifts indices.
    pub fn selected_row_index<R: NavRow>(&self, rows: &[R]) -> Option<usize> {
        let sel = self.selected_path.as_ref()?;
        rows.iter().position(|r| r.path() == sel)
    }

    /// The path of the currently selected row, for an activation (Enter) or
    /// expand-toggle (Space) key — `None` when nothing is selected.
    pub fn activate(&self) -> Option<TreePath> {
        self.selected_path.clone()
    }

    /// The path of the currently selected row if it is a branch
    /// (`is_expanded().is_some()`) — the shared "Space toggles expand"
    /// precondition. `None` when there is no selection or the selected row
    /// is a leaf.
    pub fn toggle_expand<R: NavRow>(&self, rows: &[R]) -> Option<TreePath> {
        let sel = self.selected_path.as_ref()?;
        let row = rows.iter().find(|r| r.path() == sel)?;
        row.is_expanded().map(|_| row.path().clone())
    }

    /// Classify a Left/Right key against the selected row: Left collapses
    /// an expanded branch, else selects the parent; Right expands a
    /// collapsed branch, else selects the first child. `rows` must be in
    /// flattened depth-first order (a child immediately follows its
    /// parent) for the first-child lookup to find it.
    pub fn horizontal_nav<R: NavRow>(&self, rows: &[R], dir: HorizontalDir) -> HorizontalNav {
        let Some(sel) = self.selected_path.as_ref() else {
            return HorizontalNav::None;
        };
        let Some(idx) = rows.iter().position(|r| r.path() == sel) else {
            return HorizontalNav::None;
        };
        let row = &rows[idx];
        match dir {
            HorizontalDir::Left => match row.is_expanded() {
                Some(true) => HorizontalNav::Collapse(row.path().clone()),
                _ => {
                    let path = row.path();
                    if path.len() <= 1 {
                        return HorizontalNav::None;
                    }
                    let parent = path[..path.len() - 1].to_vec();
                    if rows.iter().any(|r| r.path() == &parent) {
                        HorizontalNav::SelectParent(parent)
                    } else {
                        HorizontalNav::None
                    }
                }
            },
            HorizontalDir::Right => match row.is_expanded() {
                Some(false) => HorizontalNav::Expand(row.path().clone()),
                _ => match rows.get(idx + 1) {
                    Some(next) if next.path().len() == row.path().len() + 1 => {
                        let parent = row.path();
                        if next.path()[..parent.len()] == parent[..] {
                            HorizontalNav::SelectFirstChild(next.path().clone())
                        } else {
                            HorizontalNav::None
                        }
                    }
                    _ => HorizontalNav::None,
                },
            },
        }
    }

    // ── Scroll-to-follow ─────────────────────────────────────────────

    pub fn scroll_to_visible(&mut self, row: usize, viewport_rows: usize) {
        let new_offset = ensure_visible(self.scroll_offset, viewport_rows, row);
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.scroll_px = None;
        }
    }

    // ── Whole-row scrolling ───────────────────────────────────────────

    pub fn page_scroll(&mut self, delta: isize, total_rows: usize, viewport_rows: usize) {
        self.scroll_by(delta, total_rows, viewport_rows);
    }

    pub fn scroll_by(&mut self, delta: isize, total_rows: usize, viewport_rows: usize) {
        let max = total_rows.saturating_sub(viewport_rows);
        self.scroll_offset = scroll_by_clamped(self.scroll_offset, delta, max);
        self.scroll_px = None;
    }

    /// Accumulate a fractional wheel/trackpad row delta into
    /// [`Self::scroll_px`] instead of snapping straight to a whole row.
    ///
    /// `row_delta` is already in row units (positive = scroll content up
    /// per [`crate::ScrollDelta`]'s convention). `row_height` converts it to
    /// the pixel currency `scroll_px` is stored in.
    pub fn scroll_by_wheel(
        &mut self,
        row_delta: f32,
        row_height: f32,
        total_rows: usize,
        viewport_rows: usize,
    ) {
        let row_height = if row_height.is_finite() && row_height > 0.0 {
            row_height
        } else {
            1.0
        };
        let max_offset = total_rows.saturating_sub(viewport_rows);
        let max_px = max_offset as f32 * row_height;
        let current_px = self
            .scroll_px
            .unwrap_or(self.scroll_offset as f32 * row_height);
        let delta_px = wheel_delta(row_delta, row_height);
        let new_px = scroll_by_clamped_f32(current_px, delta_px, max_px);
        self.scroll_px = Some(new_px);
        // `scroll_rows_floor` here must agree with `resolve_scroll_px`'s own
        // floor — rounding instead would desync `scroll_offset` (used for
        // hit-testing and `ensure_visible`) from the row the paint path
        // actually starts painting from whenever the fractional part of
        // `new_px / row_height` is >= 0.5.
        self.scroll_offset = scroll_rows_floor(new_px, row_height).min(max_offset);
    }

    // ── Scrollbar drag ───────────────────────────────────────────────

    /// Begin a scrollbar thumb drag at pixel position `y` along the track,
    /// with the track's draggable length `travel` and `max_offset` rows of
    /// slack.
    pub fn begin_scrollbar_drag(&mut self, y: f32, travel: f32, max_offset: f32) {
        self.scroll_drag = Some(ThumbDrag::begin(
            y,
            self.scroll_offset as f32,
            travel,
            max_offset,
        ));
    }

    pub fn end_scrollbar_drag(&mut self) {
        self.scroll_drag = None;
    }

    /// Update the scroll offset for a drag-in-progress at pixel position
    /// `y`. Returns the new offset if it changed, `None` if there is no
    /// active drag or the offset didn't move.
    pub fn drag_to(&mut self, y: f32) -> Option<usize> {
        let drag = self.scroll_drag.as_ref()?;
        let new = drag.offset_at(y)?;
        let new = new.round() as usize;
        if new == self.scroll_offset {
            return None;
        }
        self.scroll_offset = new;
        self.scroll_px = None;
        Some(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── NavKey ─────────────────────────────────────────────────────────

    fn key_event(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Default::default(),
            repeat: false,
        }
    }

    #[test]
    fn from_key_maps_named_nav_keys() {
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::Up))),
            Some(NavKey::Up)
        );
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::Down))),
            Some(NavKey::Down)
        );
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::PageUp))),
            Some(NavKey::PageUp(1))
        );
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::PageDown))),
            Some(NavKey::PageDown(1))
        );
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::Home))),
            Some(NavKey::Home)
        );
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::End))),
            Some(NavKey::End)
        );
    }

    #[test]
    fn from_key_ignores_non_nav_keys() {
        assert_eq!(
            NavKey::from_key(&key_event(Key::Named(NamedKey::Enter))),
            None
        );
        assert_eq!(NavKey::from_key(&key_event(Key::Char('a'))), None);
    }

    // ── FlatNav ────────────────────────────────────────────────────────

    #[test]
    fn flat_nav_on_empty_list_is_none() {
        assert_eq!(FlatNav::mv(0, None, NavKey::Down, false, |_| true), None);
    }

    #[test]
    fn flat_nav_down_from_none_selects_first() {
        assert_eq!(FlatNav::mv(5, None, NavKey::Down, false, |_| true), Some(0));
    }

    #[test]
    fn flat_nav_up_from_none_selects_last() {
        assert_eq!(FlatNav::mv(5, None, NavKey::Up, false, |_| true), Some(4));
    }

    #[test]
    fn flat_nav_clamps_without_wrap() {
        assert_eq!(
            FlatNav::mv(5, Some(4), NavKey::Down, false, |_| true),
            Some(4)
        );
        assert_eq!(
            FlatNav::mv(5, Some(0), NavKey::Up, false, |_| true),
            Some(0)
        );
    }

    #[test]
    fn flat_nav_wraps_when_enabled() {
        assert_eq!(
            FlatNav::mv(5, Some(4), NavKey::Down, true, |_| true),
            Some(0)
        );
        assert_eq!(FlatNav::mv(5, Some(0), NavKey::Up, true, |_| true), Some(4));
    }

    #[test]
    fn flat_nav_home_and_end() {
        assert_eq!(
            FlatNav::mv(5, Some(2), NavKey::Home, false, |_| true),
            Some(0)
        );
        assert_eq!(
            FlatNav::mv(5, Some(2), NavKey::End, false, |_| true),
            Some(4)
        );
    }

    #[test]
    fn flat_nav_page_moves_by_n() {
        assert_eq!(
            FlatNav::mv(10, Some(5), NavKey::PageDown(3), false, |_| true),
            Some(8)
        );
        assert_eq!(
            FlatNav::mv(10, Some(5), NavKey::PageUp(3), false, |_| true),
            Some(2)
        );
    }

    #[test]
    fn flat_nav_skips_disabled_items() {
        // Index 1 is disabled: Down from 0 lands on 2, not 1.
        let is_enabled = |i: usize| i != 1;
        assert_eq!(
            FlatNav::mv(4, Some(0), NavKey::Down, false, is_enabled),
            Some(2)
        );
    }

    #[test]
    fn flat_nav_home_skips_leading_disabled_items() {
        let is_enabled = |i: usize| i != 0;
        assert_eq!(
            FlatNav::mv(4, Some(2), NavKey::Home, false, is_enabled),
            Some(1)
        );
    }

    #[test]
    fn flat_nav_all_disabled_is_none() {
        assert_eq!(
            FlatNav::mv(4, Some(0), NavKey::Down, false, |_| false),
            None
        );
    }

    // ── TreeNavCore ────────────────────────────────────────────────────

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Row {
        path: TreePath,
        indent: u16,
        is_expanded: Option<bool>,
    }

    impl NavRow for Row {
        fn path(&self) -> &TreePath {
            &self.path
        }
        fn indent(&self) -> u16 {
            self.indent
        }
        fn is_expanded(&self) -> Option<bool> {
            self.is_expanded
        }
    }

    fn leaf(path: &[u16]) -> Row {
        Row {
            path: path.to_vec(),
            indent: path.len().saturating_sub(1) as u16,
            is_expanded: None,
        }
    }

    fn branch(path: &[u16], expanded: bool) -> Row {
        Row {
            path: path.to_vec(),
            indent: path.len().saturating_sub(1) as u16,
            is_expanded: Some(expanded),
        }
    }

    fn flat_rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| leaf(&[i as u16])).collect()
    }

    // ── move_selection_by ────────────────────────────────────────────

    #[test]
    fn move_down_from_none_selects_first() {
        let mut core = TreeNavCore::new();
        let rows = flat_rows(5);
        let outcome = core.move_selection_by(&rows, 1, 10);
        assert_eq!(outcome, NavOutcome::Selected(vec![0]));
    }

    #[test]
    fn move_on_empty_rows_is_empty() {
        let mut core = TreeNavCore::new();
        let rows: Vec<Row> = Vec::new();
        assert_eq!(core.move_selection_by(&rows, 1, 10), NavOutcome::Empty);
    }

    #[test]
    fn move_past_last_row_is_unchanged() {
        let mut core = TreeNavCore::new();
        let rows = flat_rows(3);
        core.set_selected_path(Some(vec![2]));
        assert_eq!(core.move_selection_by(&rows, 1, 10), NavOutcome::Unchanged);
    }

    #[test]
    fn page_down_clamps_to_last_row() {
        let mut core = TreeNavCore::new();
        let rows = flat_rows(5);
        core.set_selected_path(Some(vec![0]));
        // A page bigger than the remaining rows clamps instead of panicking
        // or overshooting past the final row.
        let outcome = core.move_selection_by(&rows, 100, 3);
        assert_eq!(outcome, NavOutcome::Selected(vec![4]));
        assert_eq!(core.scroll_offset(), 2, "last row stays in view");
    }

    #[test]
    fn selection_stays_on_same_path_across_a_re_flatten() {
        // Simulates an app inserting a row above the selection and pushing
        // a freshly-flattened row list next frame — selection tracks by
        // path, so it survives even though the selected row's index moved.
        let mut core = TreeNavCore::new();
        let rows_before = vec![leaf(&[0]), leaf(&[1]), leaf(&[2])];
        core.set_selected_path(Some(vec![1]));
        assert_eq!(core.selected_row_index(&rows_before), Some(1));

        let rows_after = vec![leaf(&[10]), leaf(&[0]), leaf(&[1]), leaf(&[2])];
        assert_eq!(core.selected_path(), Some(&vec![1]));
        assert_eq!(core.selected_row_index(&rows_after), Some(2));
    }

    // ── move_selection_to_edge ────────────────────────────────────────

    #[test]
    fn edge_start_and_end() {
        let mut core = TreeNavCore::new();
        let rows = flat_rows(5);
        core.set_selected_path(Some(vec![3]));
        assert_eq!(
            core.move_selection_to_edge(&rows, Edge::Start, 10),
            NavOutcome::Selected(vec![0])
        );
        assert_eq!(
            core.move_selection_to_edge(&rows, Edge::End, 10),
            NavOutcome::Selected(vec![4])
        );
    }

    // ── horizontal_nav ────────────────────────────────────────────────

    #[test]
    fn left_on_expanded_branch_collapses() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], true), leaf(&[0, 0])];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Left),
            HorizontalNav::Collapse(vec![0])
        );
    }

    #[test]
    fn left_on_collapsed_branch_selects_parent() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], true), branch(&[0, 0], false)];
        core.set_selected_path(Some(vec![0, 0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Left),
            HorizontalNav::SelectParent(vec![0])
        );
    }

    #[test]
    fn left_on_leaf_selects_parent() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], true), leaf(&[0, 0])];
        core.set_selected_path(Some(vec![0, 0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Left),
            HorizontalNav::SelectParent(vec![0])
        );
    }

    #[test]
    fn left_on_root_row_is_none() {
        let mut core = TreeNavCore::new();
        let rows = vec![leaf(&[0])];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Left),
            HorizontalNav::None
        );
    }

    #[test]
    fn right_on_collapsed_branch_expands() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], false)];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Right),
            HorizontalNav::Expand(vec![0])
        );
    }

    #[test]
    fn right_on_expanded_branch_selects_first_child() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], true), leaf(&[0, 0]), leaf(&[0, 1])];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Right),
            HorizontalNav::SelectFirstChild(vec![0, 0])
        );
    }

    #[test]
    fn right_on_leaf_is_none() {
        let mut core = TreeNavCore::new();
        let rows = vec![leaf(&[0]), leaf(&[1])];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Right),
            HorizontalNav::None
        );
    }

    #[test]
    fn right_with_no_selection_is_none() {
        let core = TreeNavCore::new();
        let rows = vec![branch(&[0], false)];
        assert_eq!(
            core.horizontal_nav(&rows, HorizontalDir::Right),
            HorizontalNav::None
        );
    }

    // ── scroll_by / page_scroll ───────────────────────────────────────

    #[test]
    fn scroll_by_clamps_to_bounds() {
        let mut core = TreeNavCore::new();
        core.scroll_by(100, 10, 5);
        assert_eq!(core.scroll_offset(), 5);
        core.scroll_by(-100, 10, 5);
        assert_eq!(core.scroll_offset(), 0);
    }

    #[test]
    fn page_scroll_matches_scroll_by() {
        let mut core = TreeNavCore::new();
        core.page_scroll(100, 20, 5);
        assert_eq!(core.scroll_offset(), 15);
    }

    // ── scrollbar drag ────────────────────────────────────────────────

    #[test]
    fn drag_without_begin_is_none() {
        let mut core = TreeNavCore::new();
        assert_eq!(core.drag_to(5.0), None);
    }

    #[test]
    fn end_drag_clears_state() {
        let mut core = TreeNavCore::new();
        core.begin_scrollbar_drag(0.0, 100.0, 10.0);
        assert!(core.is_dragging());
        core.end_scrollbar_drag();
        assert!(!core.is_dragging());
        assert_eq!(core.drag_to(5.0), None);
    }

    // ── activate / toggle_expand ──────────────────────────────────────

    #[test]
    fn activate_with_no_selection_is_none() {
        let core = TreeNavCore::new();
        assert_eq!(core.activate(), None);
    }

    #[test]
    fn activate_returns_selected_path() {
        let mut core = TreeNavCore::new();
        core.set_selected_path(Some(vec![2]));
        assert_eq!(core.activate(), Some(vec![2]));
    }

    #[test]
    fn toggle_expand_on_leaf_is_none() {
        let mut core = TreeNavCore::new();
        let rows = vec![leaf(&[0])];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(core.toggle_expand(&rows), None);
    }

    #[test]
    fn toggle_expand_on_branch_returns_path() {
        let mut core = TreeNavCore::new();
        let rows = vec![branch(&[0], true)];
        core.set_selected_path(Some(vec![0]));
        assert_eq!(core.toggle_expand(&rows), Some(vec![0]));
    }
}
