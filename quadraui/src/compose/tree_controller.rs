//! `TreeController` — a composed controller for a single keyboard-navigable
//! `TreeView` with scrollbar.
//!
//! Owns the interaction state machine: selection movement with
//! scroll-to-follow, click hit-testing, scrollbar thumb drag, and
//! scroll-wheel handling.
//!
//! Apps push row data per frame via [`TreeController::set_rows`], call
//! [`TreeController::render`] + [`TreeController::handle`], and match on
//! [`TreeControllerEvent`] for semantic actions.
//!
//! Keyboard behaviour (`j`/`k` require [`TreeController::set_vim_keys`]`(true)`,
//! which is the default):
//! - `Up` / `k`          — previous row (scroll-to-follow)
//! - `Down` / `j`        — next row (scroll-to-follow)
//! - `Home`              — first row
//! - `End`               — last row
//! - `PageUp` / `PageDown` — jump by viewport rows
//! - `Enter`             — emit [`TreeControllerEvent::RowActivated`]

use crate::{
    Backend, ButtonMask, Key, MouseButton, NamedKey, Rect, Scrollbar, SelectionMode, TreePath,
    TreeRow, TreeView, TreeViewHit, UiEvent, WidgetId,
};

/// What happened after [`TreeController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeControllerEvent {
    /// Row clicked or selected via keyboard navigation.
    RowSelected { path: TreePath },
    /// Enter pressed on the currently selected row.
    RowActivated { path: TreePath },
    /// Scrollbar interaction (drag or page).
    ScrollChanged,
    /// Event consumed (drag update, hover) — caller should redraw.
    Consumed,
    /// Event not relevant to the tree.
    Ignored,
}

struct ScrollDrag {
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

pub struct TreeController {
    id: WidgetId,
    rows: Vec<TreeRow>,
    selected_path: Option<TreePath>,
    scroll_offset: usize,
    has_focus: bool,
    scroll_drag: Option<ScrollDrag>,
    vim_keys: bool,
}

impl TreeController {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WidgetId::new(id),
            rows: Vec::new(),
            selected_path: None,
            scroll_offset: 0,
            has_focus: true,
            scroll_drag: None,
            vim_keys: true,
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_rows(&mut self, rows: Vec<TreeRow>) {
        self.rows = rows;
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn selected_path(&self) -> Option<&TreePath> {
        self.selected_path.as_ref()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_selected_path(&mut self, path: Option<TreePath>) {
        self.selected_path = path;
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset;
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    pub fn vim_keys(&self) -> bool {
        self.vim_keys
    }

    /// Enable or disable `j`/`k` as aliases for Down/Up. Default `true`.
    /// Consumers wanting fully custom key bindings can set this to `false`
    /// and call the navigation primitives directly.
    pub fn set_vim_keys(&mut self, enabled: bool) {
        self.vim_keys = enabled;
    }

    // ── Render ────────────────────────────────────────────────────────

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let (tree_rect, sb_rect) = self.split_rect(backend, rect);
        let tree = self.build_tree_view(tree_rect);
        backend.draw_tree(tree_rect, &tree);
        if let Some(sb_rect) = sb_rect {
            let sb = self.build_scrollbar(backend, sb_rect);
            backend.draw_scrollbar(sb_rect, &sb);
        }
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> TreeControllerEvent {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, position.x, position.y),

            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => self.drag_to(position.y),

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.scroll_drag = None;
                TreeControllerEvent::Ignored
            }

            UiEvent::Scroll { delta, .. } => {
                let rows = if delta.y > 0.0 { -1 } else { 1 };
                self.scroll_by(backend, rect, rows);
                TreeControllerEvent::Consumed
            }

            UiEvent::KeyPressed { key, .. } => self.handle_key(key, backend, rect),

            _ => TreeControllerEvent::Ignored,
        }
    }

    // ── Navigation primitives (pub for SidebarSystem reuse) ───────────

    pub fn move_selection_by(&mut self, delta: isize, viewport_rows: usize) -> TreeControllerEvent {
        if self.rows.is_empty() {
            return TreeControllerEvent::Ignored;
        }
        let count = self.rows.len();
        let current = self.selected_row_index();

        let new = if let Some(cur) = current {
            let target = cur as isize + delta;
            target.max(0).min(count as isize - 1) as usize
        } else if delta > 0 {
            0
        } else {
            count - 1
        };

        if current == Some(new) {
            return TreeControllerEvent::Consumed;
        }

        let path = self.rows[new].path.clone();
        self.selected_path = Some(path.clone());
        self.scroll_to_visible(new, viewport_rows);
        TreeControllerEvent::RowSelected { path }
    }

    pub fn jump_to_edge(&mut self, to_start: bool, viewport_rows: usize) -> TreeControllerEvent {
        if self.rows.is_empty() {
            return TreeControllerEvent::Ignored;
        }
        let row = if to_start { 0 } else { self.rows.len() - 1 };
        let path = self.rows[row].path.clone();
        self.selected_path = Some(path.clone());
        self.scroll_to_visible(row, viewport_rows);
        TreeControllerEvent::RowSelected { path }
    }

    pub fn activate_selection(&self) -> TreeControllerEvent {
        match &self.selected_path {
            Some(path) => TreeControllerEvent::RowActivated { path: path.clone() },
            None => TreeControllerEvent::Ignored,
        }
    }

    pub fn scroll_to_visible(&mut self, row: usize, viewport_rows: usize) {
        if viewport_rows == 0 {
            return;
        }
        if row < self.scroll_offset {
            self.scroll_offset = row;
        } else if row >= self.scroll_offset + viewport_rows {
            self.scroll_offset = row.saturating_sub(viewport_rows.saturating_sub(1));
        }
    }

    pub fn page_scroll(&mut self, delta: isize, viewport_rows: usize) {
        let max = self.rows.len().saturating_sub(viewport_rows) as isize;
        let cur = self.scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.scroll_offset = new;
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn handle_key(
        &mut self,
        key: &Key,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> TreeControllerEvent {
        let vim_up = self.vim_keys && matches!(key, Key::Char('k'));
        let vim_down = self.vim_keys && matches!(key, Key::Char('j'));
        match key {
            Key::Named(NamedKey::Up) => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by(-1, vr)
            }
            _ if vim_up => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by(-1, vr)
            }
            Key::Named(NamedKey::Down) => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by(1, vr)
            }
            _ if vim_down => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by(1, vr)
            }
            Key::Named(NamedKey::Home) => {
                let vr = self.viewport_rows(backend, rect);
                self.jump_to_edge(true, vr)
            }
            Key::Named(NamedKey::End) => {
                let vr = self.viewport_rows(backend, rect);
                self.jump_to_edge(false, vr)
            }
            Key::Named(NamedKey::PageUp) => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by(-((vr.max(1) - 1).max(1) as isize), vr)
            }
            Key::Named(NamedKey::PageDown) => {
                let vr = self.viewport_rows(backend, rect);
                self.move_selection_by((vr.max(1) - 1).max(1) as isize, vr)
            }
            Key::Named(NamedKey::Enter) => self.activate_selection(),
            _ => TreeControllerEvent::Ignored,
        }
    }

    fn click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        x: f32,
        y: f32,
    ) -> TreeControllerEvent {
        if !rect_contains(rect, x, y) {
            return TreeControllerEvent::Ignored;
        }
        let (tree_rect, sb_rect) = self.split_rect(backend, rect);

        if let Some(sb_rect) = sb_rect {
            if rect_contains(sb_rect, x, y) {
                return self.click_scrollbar(backend, tree_rect, sb_rect, x, y);
            }
        }

        if rect_contains(tree_rect, x, y) {
            let tree = self.build_tree_view(tree_rect);
            let layout = backend.tree_layout(tree_rect, &tree);
            match layout.hit_test(x - tree_rect.x, y - tree_rect.y) {
                TreeViewHit::Row(idx) => {
                    let path = self.rows[idx].path.clone();
                    self.selected_path = Some(path.clone());
                    TreeControllerEvent::RowSelected { path }
                }
                TreeViewHit::Empty => TreeControllerEvent::Consumed,
            }
        } else {
            TreeControllerEvent::Ignored
        }
    }

    fn click_scrollbar(
        &mut self,
        backend: &mut dyn Backend,
        tree_rect: Rect,
        sb_rect: Rect,
        _x: f32,
        y: f32,
    ) -> TreeControllerEvent {
        let viewport_rows = self.viewport_rows(backend, tree_rect);
        let max_offset = self.rows.len().saturating_sub(viewport_rows);
        if max_offset == 0 {
            return TreeControllerEvent::Ignored;
        }

        let sb = self.build_scrollbar(backend, sb_rect);
        let thumb_top = sb_rect.y + sb.thumb_start;
        let thumb_bottom = thumb_top + sb.thumb_len;

        if y >= thumb_top && y < thumb_bottom {
            let travel = (sb_rect.height - sb.thumb_len).max(0.0);
            self.scroll_drag = Some(ScrollDrag {
                origin_y: y,
                origin_offset: self.scroll_offset,
                travel,
                max_offset,
            });
            TreeControllerEvent::ScrollChanged
        } else if y < thumb_top {
            self.page_scroll(-(viewport_rows as isize), viewport_rows);
            TreeControllerEvent::ScrollChanged
        } else {
            self.page_scroll(viewport_rows as isize, viewport_rows);
            TreeControllerEvent::ScrollChanged
        }
    }

    fn drag_to(&mut self, y: f32) -> TreeControllerEvent {
        let Some(drag) = &self.scroll_drag else {
            return TreeControllerEvent::Ignored;
        };
        if drag.travel <= 0.0 || drag.max_offset == 0 {
            return TreeControllerEvent::Ignored;
        }
        let dy = y - drag.origin_y;
        let drow = dy / drag.travel * drag.max_offset as f32;
        let new = (drag.origin_offset as f32 + drow).round() as i32;
        let new = new.max(0) as usize;
        let new = new.min(drag.max_offset);
        if new == self.scroll_offset {
            return TreeControllerEvent::Ignored;
        }
        self.scroll_offset = new;
        TreeControllerEvent::Consumed
    }

    fn scroll_by(&mut self, backend: &mut dyn Backend, rect: Rect, delta: isize) {
        let (tree_rect, _) = self.split_rect(backend, rect);
        let viewport_rows = self.viewport_rows(backend, tree_rect);
        let max = self.rows.len().saturating_sub(viewport_rows) as isize;
        let cur = self.scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.scroll_offset = new;
    }

    fn viewport_rows(&self, backend: &dyn Backend, rect: Rect) -> usize {
        let tree = self.build_tree_view(rect);
        let mut shadow = tree;
        shadow.scroll_offset = 0;
        let layout = backend.tree_layout(rect, &shadow);
        layout.visible_rows.len()
    }

    pub fn selected_row_index(&self) -> Option<usize> {
        let sel = self.selected_path.as_ref()?;
        self.rows.iter().position(|r| &r.path == sel)
    }

    fn build_tree_view(&self, _rect: Rect) -> TreeView {
        TreeView {
            id: self.id.clone(),
            rows: self.rows.clone(),
            selection_mode: SelectionMode::Single,
            selected_path: self.selected_path.clone(),
            scroll_offset: self.scroll_offset,
            style: Default::default(),
            has_focus: self.has_focus,
        }
    }

    fn scrollbar_track_width(&self, backend: &dyn Backend) -> f32 {
        backend.line_height()
    }

    fn needs_scrollbar(&self, backend: &dyn Backend, tree_rect: Rect) -> bool {
        self.rows.len() > self.viewport_rows(backend, tree_rect)
    }

    fn split_rect(&self, backend: &dyn Backend, rect: Rect) -> (Rect, Option<Rect>) {
        let track_w = self.scrollbar_track_width(backend);
        if rect.width <= track_w {
            return (rect, None);
        }
        let tree_rect = Rect::new(rect.x, rect.y, rect.width - track_w, rect.height);
        if !self.needs_scrollbar(backend, tree_rect) {
            return (rect, None);
        }
        let sb_rect = Rect::new(rect.x + rect.width - track_w, rect.y, track_w, rect.height);
        (tree_rect, Some(sb_rect))
    }

    fn build_scrollbar(&self, backend: &dyn Backend, sb_rect: Rect) -> Scrollbar {
        let total = self.rows.len() as f32;
        let visible = {
            // Approximate visible rows from height / line_height.
            let lh = backend.line_height();
            if lh > 0.0 {
                (sb_rect.height / lh).floor()
            } else {
                total
            }
        };
        let min_thumb = backend.line_height().max(1.0);
        let is_dragging = self.scroll_drag.is_some();
        let mut sb = Scrollbar::vertical(
            format!("{}-scrollbar", self.id.0),
            sb_rect,
            self.scroll_offset as f32,
            total,
            visible,
            min_thumb,
        );
        sb.dragging = is_dragging;
        sb
    }
}

fn rect_contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decoration, StyledText};

    fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
        (0..n)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("{prefix}{i}")),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
            })
            .collect()
    }

    fn test_controller() -> TreeController {
        let mut tc = TreeController::new("test-tree");
        tc.set_rows(fake_rows("item", 5));
        tc
    }

    // ── Accessors ────────────────────────────────────────────────────

    #[test]
    fn new_starts_empty() {
        let tc = TreeController::new("t");
        assert_eq!(tc.selected_path(), None);
        assert_eq!(tc.scroll_offset(), 0);
        assert!(tc.has_focus());
        assert!(tc.rows().is_empty());
    }

    #[test]
    fn set_rows_updates_data() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 3));
        assert_eq!(tc.rows().len(), 3);
    }

    #[test]
    fn set_selected_path_and_read_back() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![2]));
        assert_eq!(tc.selected_path(), Some(&vec![2]));
        tc.set_selected_path(None);
        assert_eq!(tc.selected_path(), None);
    }

    #[test]
    fn set_has_focus() {
        let mut tc = test_controller();
        tc.set_has_focus(false);
        assert!(!tc.has_focus());
    }

    // ── Selection movement ──────────────────────────────────────────

    #[test]
    fn down_selects_first_row_when_none_selected() {
        let mut tc = test_controller();
        let ev = tc.move_selection_by(1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![0]));
        assert!(matches!(ev, TreeControllerEvent::RowSelected { .. }));
    }

    #[test]
    fn down_advances_to_next_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![0]));
        let ev = tc.move_selection_by(1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![1]));
        assert!(matches!(ev, TreeControllerEvent::RowSelected { .. }));
    }

    #[test]
    fn up_moves_to_previous_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![2]));
        tc.move_selection_by(-1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![1]));
    }

    #[test]
    fn clamps_at_first_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![0]));
        let ev = tc.move_selection_by(-1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![0]));
        assert_eq!(ev, TreeControllerEvent::Consumed);
    }

    #[test]
    fn clamps_at_last_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![4]));
        let ev = tc.move_selection_by(1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![4]));
        assert_eq!(ev, TreeControllerEvent::Consumed);
    }

    #[test]
    fn up_with_no_selection_selects_last_row() {
        let mut tc = test_controller();
        tc.move_selection_by(-1, 10);
        assert_eq!(tc.selected_path(), Some(&vec![4]));
    }

    #[test]
    fn home_jumps_to_first_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![3]));
        tc.jump_to_edge(true, 10);
        assert_eq!(tc.selected_path(), Some(&vec![0]));
    }

    #[test]
    fn end_jumps_to_last_row() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![0]));
        tc.jump_to_edge(false, 10);
        assert_eq!(tc.selected_path(), Some(&vec![4]));
    }

    #[test]
    fn page_down_jumps_by_viewport() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![0]));
        let jump = (3_usize.max(1) - 1).max(1) as isize; // viewport=3 → jump=2
        tc.move_selection_by(jump, 3);
        assert_eq!(tc.selected_path(), Some(&vec![2]));
    }

    #[test]
    fn enter_emits_row_activated() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![2]));
        let ev = tc.activate_selection();
        assert_eq!(ev, TreeControllerEvent::RowActivated { path: vec![2] });
    }

    #[test]
    fn enter_with_no_selection_returns_ignored() {
        let tc = test_controller();
        let ev = tc.activate_selection();
        assert_eq!(ev, TreeControllerEvent::Ignored);
    }

    // ── Scroll-to-follow ────────────────────────────────────────────

    #[test]
    fn scroll_follows_when_row_below_viewport() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![0]));
        for _ in 0..4 {
            tc.move_selection_by(1, 3);
        }
        assert_eq!(tc.selected_path(), Some(&vec![4]));
        assert!(tc.scroll_offset() >= 2, "offset={}", tc.scroll_offset());
        assert!(tc.scroll_offset() + 3 > 4);
    }

    #[test]
    fn scroll_follows_when_row_above_viewport() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![4]));
        tc.set_scroll_offset(2);
        for _ in 0..4 {
            tc.move_selection_by(-1, 3);
        }
        assert_eq!(tc.selected_path(), Some(&vec![0]));
        assert_eq!(tc.scroll_offset(), 0);
    }

    // ── Page scroll ─────────────────────────────────────────────────

    #[test]
    fn page_scroll_clamps_to_bounds() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 20));
        tc.page_scroll(100, 5);
        assert_eq!(tc.scroll_offset(), 15); // 20 - 5
        tc.page_scroll(-100, 5);
        assert_eq!(tc.scroll_offset(), 0);
    }

    // ── Empty-tree edge cases ───────────────────────────────────────

    #[test]
    fn move_selection_on_empty_tree_returns_ignored() {
        let mut tc = TreeController::new("t");
        let ev = tc.move_selection_by(1, 10);
        assert_eq!(ev, TreeControllerEvent::Ignored);
    }

    #[test]
    fn jump_to_edge_on_empty_tree_returns_ignored() {
        let mut tc = TreeController::new("t");
        let ev = tc.jump_to_edge(true, 10);
        assert_eq!(ev, TreeControllerEvent::Ignored);
    }
}
