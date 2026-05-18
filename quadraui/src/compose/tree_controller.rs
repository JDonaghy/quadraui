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
//! - Double-click        — emit [`TreeControllerEvent::RowActivated`]
//!
//! Scroll-wheel events follow the [`ScrollDelta`](crate::ScrollDelta)
//! sign convention: positive `delta.y` = scroll content up (decrease
//! offset). Backends normalise their native direction before emitting.

use crate::{
    Backend, ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, Scrollbar,
    SelectionMode, TreePath, TreeRow, TreeRowEditState, TreeView, TreeViewHit, UiEvent, WidgetId,
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
    /// The user confirmed an inline edit (pressed Enter).
    EditConfirmed { path: TreePath, new_text: String },
    /// The user cancelled an inline edit (pressed Escape).
    EditCancelled { path: TreePath },
    /// The text buffer changed during inline editing.
    EditChanged { path: TreePath, text: String },
    /// Right-click on a row. Consumer should build and show a context menu.
    ContextMenuRequested { path: TreePath, position: Point },
}

struct ScrollDrag {
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

struct EditingState {
    path: TreePath,
    text: String,
    cursor: usize,
    selection_anchor: Option<usize>,
    placeholder: Option<String>,
}

pub struct TreeController {
    id: WidgetId,
    rows: Vec<TreeRow>,
    selected_path: Option<TreePath>,
    scroll_offset: usize,
    has_focus: bool,
    scroll_drag: Option<ScrollDrag>,
    vim_keys: bool,
    editing: Option<EditingState>,
    show_scrollbar: bool,
    /// When `Some`, overrides the default `line_height()`-based scrollbar
    /// track width with a fixed native-unit value (e.g. 8.0 px for GTK,
    /// 1.0 cell for TUI — matching MSV's `scrollbar_size`).
    scrollbar_width: Option<f32>,
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
            editing: None,
            show_scrollbar: true,
            scrollbar_width: None,
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

    pub fn show_scrollbar(&self) -> bool {
        self.show_scrollbar
    }

    /// When `false`, `render()` passes the full rect to `draw_tree()`
    /// without reserving scrollbar space. Consumers that manage their
    /// own scrollbar (e.g. through MSV) set this to `false`.
    pub fn set_show_scrollbar(&mut self, show: bool) {
        self.show_scrollbar = show;
    }

    /// Override the scrollbar track width with a fixed native-unit value.
    /// Pass `Some(8.0)` for GTK (matching MSV's 8px track) or
    /// `Some(1.0)` for TUI (matching MSV's 1-cell track). `None` falls
    /// back to `backend.line_height()`.
    pub fn set_scrollbar_width(&mut self, width: Option<f32>) {
        self.scrollbar_width = width;
    }

    // ── Inline editing ───────────────────────────────────────────────

    /// Begin inline editing of the row at `path`.
    ///
    /// `cursor` and `selection_anchor` are byte offsets into
    /// `initial_text`. For select-all: `cursor = len, anchor = Some(0)`.
    /// For filename-stem selection ("main" in "main.rs"):
    /// `cursor = 4, anchor = Some(0)`.
    pub fn start_editing(
        &mut self,
        path: TreePath,
        initial_text: String,
        cursor: usize,
        selection_anchor: Option<usize>,
        placeholder: Option<String>,
    ) {
        self.editing = Some(EditingState {
            path: path.clone(),
            text: initial_text,
            cursor,
            selection_anchor,
            placeholder,
        });
        self.selected_path = Some(path);
    }

    /// Cancel editing programmatically.
    pub fn cancel_editing(&mut self) {
        self.editing = None;
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn editing_path(&self) -> Option<&TreePath> {
        self.editing.as_ref().map(|e| &e.path)
    }

    pub fn editing_text(&self) -> Option<&str> {
        self.editing.as_ref().map(|e| e.text.as_str())
    }

    pub fn editing_cursor(&self) -> usize {
        self.editing.as_ref().map(|e| e.cursor).unwrap_or(0)
    }

    pub fn editing_selection_anchor(&self) -> Option<usize> {
        self.editing.as_ref().and_then(|e| e.selection_anchor)
    }

    pub fn editing_placeholder(&self) -> Option<&str> {
        self.editing.as_ref().and_then(|e| e.placeholder.as_deref())
    }

    pub fn edit_insert_char_via(&mut self, ch: char) -> TreeControllerEvent {
        if self.editing.is_none() {
            return TreeControllerEvent::Ignored;
        }
        self.edit_insert_char(ch)
    }

    pub fn edit_insert_str_via(&mut self, s: &str) -> TreeControllerEvent {
        if self.editing.is_none() {
            return TreeControllerEvent::Ignored;
        }
        self.edit_insert_str(s)
    }

    pub fn handle_edit_key_via(&mut self, key: &Key, modifiers: &Modifiers) -> TreeControllerEvent {
        if self.editing.is_none() {
            return TreeControllerEvent::Ignored;
        }
        self.handle_edit_key(key, modifiers)
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
            UiEvent::CharTyped(ch) if self.editing.is_some() => self.edit_insert_char(*ch),

            UiEvent::ClipboardPaste(text) if self.editing.is_some() => self.edit_insert_str(text),

            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, position.x, position.y),

            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => self.right_click(backend, rect, *position),

            UiEvent::DoubleClick { position, .. } => {
                self.double_click(backend, rect, position.x, position.y)
            }

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
                let vr = self.viewport_rows(backend, rect);
                let rows = if delta.y > 0.0 { -1 } else { 1 };
                self.scroll_by(rows, vr);
                TreeControllerEvent::Consumed
            }

            UiEvent::KeyPressed { key, modifiers, .. } => {
                self.handle_key(key, modifiers, backend, rect)
            }

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
        modifiers: &Modifiers,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> TreeControllerEvent {
        if self.editing.is_some() {
            return self.handle_edit_key(key, modifiers);
        }
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

    // ── Inline editing key dispatch ──────────────────────────────────

    fn handle_edit_key(&mut self, key: &Key, modifiers: &Modifiers) -> TreeControllerEvent {
        match key {
            Key::Named(NamedKey::Enter) => {
                let e = self.editing.take().unwrap();
                TreeControllerEvent::EditConfirmed {
                    path: e.path,
                    new_text: e.text,
                }
            }
            Key::Named(NamedKey::Escape) => {
                let e = self.editing.take().unwrap();
                TreeControllerEvent::EditCancelled { path: e.path }
            }
            Key::Named(NamedKey::Backspace) => self.edit_backspace(),
            Key::Named(NamedKey::Delete) => self.edit_delete(),
            Key::Named(NamedKey::Left) => {
                self.edit_move_cursor_left(modifiers.shift);
                TreeControllerEvent::Consumed
            }
            Key::Named(NamedKey::Right) => {
                self.edit_move_cursor_right(modifiers.shift);
                TreeControllerEvent::Consumed
            }
            Key::Named(NamedKey::Home) => {
                self.edit_move_home(modifiers.shift);
                TreeControllerEvent::Consumed
            }
            Key::Named(NamedKey::End) => {
                self.edit_move_end(modifiers.shift);
                TreeControllerEvent::Consumed
            }
            Key::Char('a') if modifiers.ctrl => {
                self.edit_select_all();
                TreeControllerEvent::Consumed
            }
            Key::Char(c) => self.edit_insert_char(*c),
            _ => TreeControllerEvent::Consumed,
        }
    }

    // ── Text buffer manipulation helpers ─────────────────────────────

    fn edit_delete_selection(&mut self) -> bool {
        let e = self.editing.as_mut().unwrap();
        if let Some(anchor) = e.selection_anchor {
            if anchor != e.cursor {
                let lo = anchor.min(e.cursor);
                let hi = anchor.max(e.cursor);
                let lo = snap_to_char_boundary(&e.text, lo);
                let hi = snap_to_char_boundary(&e.text, hi);
                e.text.replace_range(lo..hi, "");
                e.cursor = lo;
                e.selection_anchor = None;
                return true;
            }
        }
        false
    }

    fn edit_insert_char(&mut self, c: char) -> TreeControllerEvent {
        self.edit_delete_selection();
        let e = self.editing.as_mut().unwrap();
        let cursor = snap_to_char_boundary(&e.text, e.cursor);
        e.text.insert(cursor, c);
        e.cursor = cursor + c.len_utf8();
        e.selection_anchor = None;
        self.emit_edit_changed()
    }

    fn edit_insert_str(&mut self, s: &str) -> TreeControllerEvent {
        self.edit_delete_selection();
        let e = self.editing.as_mut().unwrap();
        let cursor = snap_to_char_boundary(&e.text, e.cursor);
        e.text.insert_str(cursor, s);
        e.cursor = cursor + s.len();
        e.selection_anchor = None;
        self.emit_edit_changed()
    }

    fn edit_backspace(&mut self) -> TreeControllerEvent {
        if self.edit_delete_selection() {
            return self.emit_edit_changed();
        }
        let e = self.editing.as_mut().unwrap();
        if e.cursor == 0 {
            return TreeControllerEvent::Consumed;
        }
        let prev = prev_char_boundary(&e.text, e.cursor);
        e.text.replace_range(prev..e.cursor, "");
        e.cursor = prev;
        e.selection_anchor = None;
        self.emit_edit_changed()
    }

    fn edit_delete(&mut self) -> TreeControllerEvent {
        if self.edit_delete_selection() {
            return self.emit_edit_changed();
        }
        let e = self.editing.as_mut().unwrap();
        if e.cursor >= e.text.len() {
            return TreeControllerEvent::Consumed;
        }
        let next = next_char_boundary(&e.text, e.cursor);
        e.text.replace_range(e.cursor..next, "");
        self.emit_edit_changed()
    }

    fn edit_move_cursor_left(&mut self, extend_selection: bool) {
        let e = self.editing.as_mut().unwrap();
        if e.cursor == 0 {
            if !extend_selection {
                e.selection_anchor = None;
            }
            return;
        }
        if extend_selection && e.selection_anchor.is_none() {
            e.selection_anchor = Some(e.cursor);
        }
        e.cursor = prev_char_boundary(&e.text, e.cursor);
        if !extend_selection {
            e.selection_anchor = None;
        }
    }

    fn edit_move_cursor_right(&mut self, extend_selection: bool) {
        let e = self.editing.as_mut().unwrap();
        if e.cursor >= e.text.len() {
            if !extend_selection {
                e.selection_anchor = None;
            }
            return;
        }
        if extend_selection && e.selection_anchor.is_none() {
            e.selection_anchor = Some(e.cursor);
        }
        e.cursor = next_char_boundary(&e.text, e.cursor);
        if !extend_selection {
            e.selection_anchor = None;
        }
    }

    fn edit_move_home(&mut self, extend_selection: bool) {
        let e = self.editing.as_mut().unwrap();
        if extend_selection && e.selection_anchor.is_none() {
            e.selection_anchor = Some(e.cursor);
        }
        e.cursor = 0;
        if !extend_selection {
            e.selection_anchor = None;
        }
    }

    fn edit_move_end(&mut self, extend_selection: bool) {
        let e = self.editing.as_mut().unwrap();
        if extend_selection && e.selection_anchor.is_none() {
            e.selection_anchor = Some(e.cursor);
        }
        e.cursor = e.text.len();
        if !extend_selection {
            e.selection_anchor = None;
        }
    }

    fn edit_select_all(&mut self) {
        let e = self.editing.as_mut().unwrap();
        e.selection_anchor = Some(0);
        e.cursor = e.text.len();
    }

    fn emit_edit_changed(&self) -> TreeControllerEvent {
        let e = self.editing.as_ref().unwrap();
        TreeControllerEvent::EditChanged {
            path: e.path.clone(),
            text: e.text.clone(),
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

    fn double_click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        x: f32,
        y: f32,
    ) -> TreeControllerEvent {
        let (tree_rect, _) = self.split_rect(backend, rect);
        if !rect_contains(tree_rect, x, y) {
            return TreeControllerEvent::Ignored;
        }
        let tree = self.build_tree_view(tree_rect);
        let layout = backend.tree_layout(tree_rect, &tree);
        match layout.hit_test(x - tree_rect.x, y - tree_rect.y) {
            TreeViewHit::Row(idx) => {
                let path = self.rows[idx].path.clone();
                self.selected_path = Some(path.clone());
                TreeControllerEvent::RowActivated { path }
            }
            TreeViewHit::Empty => TreeControllerEvent::Consumed,
        }
    }

    fn right_click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        position: Point,
    ) -> TreeControllerEvent {
        let (tree_rect, _) = self.split_rect(backend, rect);
        if !rect_contains(tree_rect, position.x, position.y) {
            return TreeControllerEvent::Ignored;
        }
        let tree = self.build_tree_view(tree_rect);
        let layout = backend.tree_layout(tree_rect, &tree);
        match layout.hit_test(position.x - tree_rect.x, position.y - tree_rect.y) {
            TreeViewHit::Row(idx) => {
                let path = self.rows[idx].path.clone();
                self.selected_path = Some(path.clone());
                TreeControllerEvent::ContextMenuRequested { path, position }
            }
            TreeViewHit::Empty => TreeControllerEvent::Consumed,
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

    pub fn scroll_by(&mut self, delta: isize, viewport_rows: usize) {
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
        let mut rows = self.rows.clone();
        if let Some(ref editing) = self.editing {
            if let Some(row) = rows.iter_mut().find(|r| r.path == editing.path) {
                row.edit = Some(TreeRowEditState {
                    text: editing.text.clone(),
                    cursor: editing.cursor,
                    selection_anchor: editing.selection_anchor,
                    placeholder: editing.placeholder.clone(),
                });
            }
        }
        TreeView {
            id: self.id.clone(),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: self.selected_path.clone(),
            scroll_offset: self.scroll_offset,
            style: Default::default(),
            has_focus: self.has_focus,
        }
    }

    fn scrollbar_track_width(&self, backend: &dyn Backend) -> f32 {
        self.scrollbar_width
            .unwrap_or_else(|| backend.line_height())
    }

    fn needs_scrollbar(&self, backend: &dyn Backend, tree_rect: Rect) -> bool {
        self.rows.len() > self.viewport_rows(backend, tree_rect)
    }

    fn split_rect(&self, backend: &dyn Backend, rect: Rect) -> (Rect, Option<Rect>) {
        if !self.show_scrollbar {
            return (rect, None);
        }
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
        let lh = backend.line_height();
        let visible = if lh > 0.0 {
            (sb_rect.height / lh).floor()
        } else {
            total
        };
        let track_w = self.scrollbar_track_width(backend);
        let min_thumb = track_w.max(1.0);
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

fn snap_to_char_boundary(s: &str, byte: usize) -> usize {
    let byte = byte.min(s.len());
    let mut i = byte;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn prev_char_boundary(s: &str, byte: usize) -> usize {
    if byte == 0 {
        return 0;
    }
    let mut i = byte - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, byte: usize) -> usize {
    let byte = byte.min(s.len());
    if byte >= s.len() {
        return s.len();
    }
    let mut i = byte + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
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
                edit: None,
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

    // ── scroll_by (pub) ─────────────────────────────────────────────

    #[test]
    fn scroll_by_positive_advances_offset() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 20));
        tc.scroll_by(3, 5);
        assert_eq!(tc.scroll_offset(), 3);
    }

    #[test]
    fn scroll_by_negative_retreats_offset() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 20));
        tc.set_scroll_offset(5);
        tc.scroll_by(-2, 5);
        assert_eq!(tc.scroll_offset(), 3);
    }

    #[test]
    fn scroll_by_clamps_to_bounds() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 10));
        tc.scroll_by(100, 5);
        assert_eq!(tc.scroll_offset(), 5); // 10 - 5
        tc.scroll_by(-100, 5);
        assert_eq!(tc.scroll_offset(), 0);
    }

    // ── Inline editing ──────────────────────────────────────────────

    #[test]
    fn start_editing_initializes_state() {
        let mut tc = test_controller();
        tc.start_editing(vec![2], "hello.rs".into(), 8, Some(0), None);
        assert!(tc.is_editing());
        assert_eq!(tc.editing_path(), Some(&vec![2]));
        assert_eq!(tc.editing_text(), Some("hello.rs"));
        assert_eq!(tc.selected_path(), Some(&vec![2]));
    }

    #[test]
    fn cancel_editing_clears_state() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "a.rs".into(), 4, Some(0), None);
        tc.cancel_editing();
        assert!(!tc.is_editing());
        assert_eq!(tc.editing_path(), None);
    }

    #[test]
    fn char_key_inserts_during_editing() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], String::new(), 0, None, None);
        let ev = tc.handle_edit_key_via(&Key::Char('j'), &Modifiers::default());
        assert!(matches!(ev, TreeControllerEvent::EditChanged { .. }));
        assert_eq!(tc.editing_text(), Some("j"));
    }

    #[test]
    fn enter_confirms_editing() {
        let mut tc = test_controller();
        tc.start_editing(vec![1], "new.txt".into(), 7, Some(0), None);
        // Select-all is on; type to replace.
        tc.handle_edit_key_via(&Key::Char('x'), &Modifiers::default());
        let ev = tc.handle_edit_key_via(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(
            ev,
            TreeControllerEvent::EditConfirmed {
                path: vec![1],
                new_text: "x".into()
            }
        );
        assert!(!tc.is_editing());
    }

    #[test]
    fn escape_cancels_editing() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "old.txt".into(), 7, Some(0), None);
        let ev = tc.handle_edit_key_via(&Key::Named(NamedKey::Escape), &Modifiers::default());
        assert_eq!(ev, TreeControllerEvent::EditCancelled { path: vec![0] });
        assert!(!tc.is_editing());
    }

    #[test]
    fn backspace_deletes_char() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "abc".into(), 3, Some(0), None);
        // Clear select-all first by pressing End.
        tc.handle_edit_key_via(&Key::Named(NamedKey::End), &Modifiers::default());
        tc.handle_edit_key_via(&Key::Named(NamedKey::Backspace), &Modifiers::default());
        assert_eq!(tc.editing_text(), Some("ab"));
    }

    #[test]
    fn left_right_move_cursor() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "ab".into(), 2, Some(0), None);
        // End clears select-all and goes to end.
        tc.handle_edit_key_via(&Key::Named(NamedKey::End), &Modifiers::default());
        tc.handle_edit_key_via(&Key::Named(NamedKey::Left), &Modifiers::default());
        tc.handle_edit_key_via(&Key::Char('X'), &Modifiers::default());
        assert_eq!(tc.editing_text(), Some("aXb"));
    }

    #[test]
    fn shift_right_extends_selection_and_backspace_deletes_range() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "abcd".into(), 4, Some(0), None);
        // Home to clear select-all, then Shift+Right twice to select "ab".
        tc.handle_edit_key_via(&Key::Named(NamedKey::Home), &Modifiers::default());
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        tc.handle_edit_key_via(&Key::Named(NamedKey::Right), &shift);
        tc.handle_edit_key_via(&Key::Named(NamedKey::Right), &shift);
        tc.handle_edit_key_via(&Key::Named(NamedKey::Backspace), &Modifiers::default());
        assert_eq!(tc.editing_text(), Some("cd"));
    }

    #[test]
    fn build_tree_view_stamps_edit_state() {
        let mut tc = test_controller();
        tc.start_editing(vec![2], "renamed".into(), 7, Some(0), None);
        let tree = tc.build_tree_view(Rect::new(0.0, 0.0, 40.0, 10.0));
        let row = &tree.rows[2];
        assert!(row.edit.is_some());
        let edit = row.edit.as_ref().unwrap();
        assert_eq!(edit.text, "renamed");
    }

    #[test]
    fn nav_keys_suppressed_during_editing() {
        let mut tc = test_controller();
        tc.set_selected_path(Some(vec![2]));
        tc.start_editing(vec![2], "test".into(), 4, Some(0), None);
        let ev = tc.handle_edit_key_via(&Key::Named(NamedKey::Down), &Modifiers::default());
        assert_eq!(ev, TreeControllerEvent::Consumed);
        assert_eq!(tc.selected_path(), Some(&vec![2]));
    }

    #[test]
    fn select_all_then_type_replaces() {
        let mut tc = test_controller();
        tc.start_editing(vec![0], "old".into(), 3, Some(0), None);
        // start_editing selects all, so typing replaces.
        tc.handle_edit_key_via(&Key::Char('n'), &Modifiers::default());
        assert_eq!(tc.editing_text(), Some("n"));
    }

    #[test]
    fn partial_selection_preserves_unselected_text() {
        let mut tc = test_controller();
        // Select only the stem "main" in "main.rs" (bytes 0..4).
        tc.start_editing(vec![0], "main.rs".into(), 4, Some(0), None);
        tc.handle_edit_key_via(&Key::Char('a'), &Modifiers::default());
        assert_eq!(tc.editing_text(), Some("a.rs"));
    }

    // ── Scrollbar toggle and width ──────────────────────────────────

    #[test]
    fn show_scrollbar_defaults_to_true() {
        let tc = TreeController::new("t");
        assert!(tc.show_scrollbar());
    }

    #[test]
    fn set_show_scrollbar_false_suppresses_scrollbar() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 100));
        tc.set_show_scrollbar(false);
        let rect = Rect::new(0.0, 0.0, 80.0, 10.0);
        let (tree_rect, sb_rect) = tc.split_rect(&MockBackend, rect);
        assert!(sb_rect.is_none());
        assert_eq!(tree_rect.width, 80.0);
    }

    #[test]
    fn set_scrollbar_width_overrides_track() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 100));
        tc.set_scrollbar_width(Some(8.0));
        let rect = Rect::new(0.0, 0.0, 80.0, 10.0);
        let (tree_rect, sb_rect) = tc.split_rect(&MockBackend, rect);
        let sb = sb_rect.expect("scrollbar should be present");
        assert_eq!(sb.width, 8.0);
        assert_eq!(tree_rect.width, 72.0);
    }

    #[test]
    fn default_scrollbar_width_uses_line_height() {
        let mut tc = TreeController::new("t");
        tc.set_rows(fake_rows("r", 100));
        let rect = Rect::new(0.0, 0.0, 80.0, 10.0);
        let (tree_rect, sb_rect) = tc.split_rect(&MockBackend, rect);
        let sb = sb_rect.expect("scrollbar should be present");
        // MockBackend.line_height() = 1.0
        assert_eq!(sb.width, 1.0);
        assert_eq!(tree_rect.width, 79.0);
    }

    struct MockBackend;

    impl Backend for MockBackend {
        fn viewport(&self) -> crate::Viewport {
            crate::Viewport {
                width: 80.0,
                height: 24.0,
                scale: 1.0,
            }
        }
        fn begin_frame(&mut self, _v: crate::Viewport) {}
        fn end_frame(&mut self) {}
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _t: std::time::Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn register_accelerator(&mut self, _a: &crate::Accelerator) {}
        fn unregister_accelerator(&mut self, _id: &crate::AcceleratorId) {}
        fn modal_stack_mut(&mut self) -> &mut crate::ModalStack {
            unimplemented!()
        }
        fn services(&self) -> &dyn crate::backend::PlatformServices {
            unimplemented!()
        }
        fn line_height(&self) -> f32 {
            1.0
        }
        fn char_width(&self) -> f32 {
            1.0
        }
        fn draw_tree(&mut self, _r: Rect, _t: &crate::TreeView) {}
        fn draw_list(&mut self, _r: Rect, _l: &crate::ListView) {}
        fn draw_data_table(
            &mut self,
            _r: Rect,
            _t: &crate::DataTable,
            _h: Option<usize>,
        ) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn data_table_layout(&self, _r: Rect, _t: &crate::DataTable) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn draw_form(&mut self, _r: Rect, _f: &crate::Form) {}
        fn draw_palette(&mut self, _r: Rect, _p: &crate::Palette) {}
        fn draw_status_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::status_bar::StatusBar,
            _hovered_id: Option<&WidgetId>,
            _pressed_id: Option<&WidgetId>,
        ) -> crate::StatusBarLayout {
            unimplemented!()
        }
        fn draw_tab_bar(
            &mut self,
            _r: Rect,
            _b: &crate::TabBar,
            _h: Option<usize>,
        ) -> crate::TabBarHits {
            unimplemented!()
        }
        fn draw_activity_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::primitives::activity_bar::ActivityBarRowHit> {
            unimplemented!()
        }
        fn draw_terminal(&mut self, _r: Rect, _t: &crate::Terminal) {}
        fn draw_text_display(&mut self, _r: Rect, _t: &crate::TextDisplay) {}
        fn draw_command_line(&mut self, _r: Rect, _c: &crate::CommandLine) {}
        fn status_bar_layout(&self, _r: Rect, _b: &crate::StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn tab_bar_layout(&self, _r: Rect, _b: &crate::TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            _r: Rect,
            _t: &crate::TextDisplay,
        ) -> crate::TextDisplayLayout {
            unimplemented!()
        }
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(Rect, WidgetId)> {
            unimplemented!()
        }
        fn draw_dialog(&mut self, _d: &crate::Dialog, _l: &crate::DialogLayout) -> Vec<Rect> {
            unimplemented!()
        }
        fn draw_multi_section_view(&mut self, _r: Rect, _v: &crate::MultiSectionView) {}
        fn msv_layout(
            &self,
            _r: Rect,
            _v: &crate::MultiSectionView,
        ) -> crate::MultiSectionViewLayout {
            unimplemented!()
        }
        fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
            unimplemented!()
        }
        fn tree_layout(
            &self,
            rect: Rect,
            tree: &crate::TreeView,
        ) -> crate::primitives::tree::TreeViewLayout {
            let lh = self.line_height();
            let visible: usize = if lh > 0.0 {
                (rect.height / lh).floor() as usize
            } else {
                0
            };
            let end = tree.scroll_offset + visible;
            let rows: Vec<crate::primitives::tree::VisibleTreeRow> = (tree.scroll_offset
                ..end.min(tree.rows.len()))
                .enumerate()
                .map(|(vi, ri)| crate::primitives::tree::VisibleTreeRow {
                    row_idx: ri,
                    bounds: Rect::new(0.0, vi as f32 * lh, rect.width, lh),
                })
                .collect();
            crate::primitives::tree::TreeViewLayout {
                viewport_width: rect.width,
                viewport_height: rect.height,
                visible_rows: rows,
                hit_regions: Vec::new(),
                resolved_scroll_offset: tree.scroll_offset,
            }
        }
        fn form_layout(&self, _r: Rect, _f: &crate::Form) -> crate::primitives::form::FormLayout {
            unimplemented!()
        }
        fn draw_editor(
            &mut self,
            _r: Rect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            Default::default()
        }
        fn draw_message_list(
            &mut self,
            _r: Rect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }
        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }
        fn draw_find_replace(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }
        fn draw_completions(
            &mut self,
            _c: &crate::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }
        fn draw_scrollbar(&mut self, _r: Rect, _s: &crate::Scrollbar) {}
        fn draw_menu_bar(&mut self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn menu_bar_layout(&self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn draw_split(&mut self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn split_layout(&self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn draw_panel(&mut self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn panel_layout(&self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn draw_toast_stack(
            &mut self,
            _r: Rect,
            _s: &crate::ToastStack,
        ) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn toast_stack_layout(&self, _r: Rect, _s: &crate::ToastStack) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn draw_progress(&mut self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn progress_layout(&self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn draw_spinner(&mut self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn spinner_layout(&self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn draw_command_center(
            &mut self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn command_center_layout(
            &self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn draw_chart(
            &mut self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
            _h: Option<(usize, usize)>,
            _x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn chart_layout(
            &self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
    }
}
