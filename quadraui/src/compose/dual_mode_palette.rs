//! `DualModePaletteController` — a compose controller that wraps the
//! [`Palette`] primitive and provides toggling between two interaction modes:
//!
//! - **List mode** — the standard search-and-select behaviour: typing
//!   filters the item list; `Enter` confirms the selected row.
//! - **Input mode** — the query field is a standalone free-text input
//!   (e.g. "create new branch name"); `Enter` confirms the raw text.
//!
//! Press `Tab` (or call [`DualModePaletteController::toggle_mode`]) to
//! switch between modes at any time without clearing the query text.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // Instantiate when the palette should open:
//! let mut pal = DualModePaletteController::new(
//!     "Branches",          // palette title
//!     "New branch name:",  // input-mode label (shown in title bar)
//!     branches,            // Vec<PaletteItem>
//! );
//!
//! // In AppLogic::render:
//! pal.render(popup_rect, backend);
//!
//! // In AppLogic::handle:
//! match pal.handle(&event, visible_rows) {
//!     DualModePaletteEvent::ItemConfirmed { idx } => { /* switch to branch */ }
//!     DualModePaletteEvent::TextConfirmed { value } => { /* create branch */ }
//!     DualModePaletteEvent::QueryChanged { value } => { /* refilter items */ }
//!     DualModePaletteEvent::ModeToggled { new_mode } => { /* update hint */ }
//!     DualModePaletteEvent::Cancelled => { /* close */ }
//!     DualModePaletteEvent::Consumed | DualModePaletteEvent::Ignored => {}
//! }
//! ```

use crate::{
    Backend, Key, Modifiers, NamedKey, Palette, PaletteItem, PaletteMode, Rect, UiEvent, WidgetId,
};

/// What happened after [`DualModePaletteController::handle`] processed an
/// event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DualModePaletteEvent {
    /// In List mode: the user confirmed the highlighted item. `idx` is the
    /// index into the `items` slice supplied to the controller.
    ItemConfirmed { idx: usize },
    /// In Input mode: the user pressed `Enter` to confirm the free-text
    /// value. `value` is the current query string.
    TextConfirmed { value: String },
    /// The query text changed. The app should refilter its source list and
    /// call [`DualModePaletteController::set_items`] with the new results.
    QueryChanged { value: String },
    /// The active mode toggled between `List` and `Input`.
    ModeToggled { new_mode: PaletteMode },
    /// The palette was dismissed (`Escape`).
    Cancelled,
    /// An event was consumed (internal state changed, caller should redraw).
    Consumed,
    /// The event was not relevant to this controller.
    Ignored,
}

/// Cross-backend compose controller for a dual-mode palette modal.
///
/// Wraps the [`Palette`] primitive and manages:
/// - current [`PaletteMode`] (`List` or `Input`)
/// - query text and cursor position
/// - item list, selection, and scroll offset
/// - keyboard event dispatch appropriate to the active mode
///
/// Rendering is delegated to [`Backend::draw_palette`].
pub struct DualModePaletteController {
    /// Widget ID forwarded to the rendered `Palette`.
    id: WidgetId,
    /// Title shown in the palette chrome.
    title: String,
    /// Label suffix appended to the title when in `Input` mode
    /// (e.g. `"New branch:"`).  `None` → title is unchanged in Input mode.
    input_label: Option<String>,
    /// Current interaction mode.
    mode: PaletteMode,
    /// Current query / text-input string.
    query: String,
    /// Cursor position: character index (not byte offset) within `query`.
    cursor_char: usize,
    /// Filtered, displayable items (app-managed).
    items: Vec<PaletteItem>,
    /// Index of the highlighted row in `items`.
    selected: usize,
    /// First visible row index (scroll offset into `items`).
    scroll_top: usize,
}

impl DualModePaletteController {
    /// Create a new controller in `List` mode.
    ///
    /// # Arguments
    ///
    /// - `title` — palette title string (shown in the title chrome).
    /// - `input_label` — optional label appended to the title bar when in
    ///   `Input` mode (e.g. `"New branch:"`).  Pass `None` to keep the same
    ///   title in both modes.
    /// - `items` — initial item list (may be empty; replace with
    ///   [`set_items`](Self::set_items) as the app filters its source).
    pub fn new(
        title: impl Into<String>,
        input_label: impl Into<Option<String>>,
        items: Vec<PaletteItem>,
    ) -> Self {
        Self {
            id: WidgetId::new("dual_mode_palette"),
            title: title.into(),
            input_label: input_label.into(),
            mode: PaletteMode::List,
            query: String::new(),
            cursor_char: 0,
            items,
            selected: 0,
            scroll_top: 0,
        }
    }

    // ── Builder setters ───────────────────────────────────────────────

    /// Override the `WidgetId` used for the rendered `Palette`.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Start in `Input` mode instead of `List` mode.
    pub fn with_mode(mut self, mode: PaletteMode) -> Self {
        self.mode = mode;
        self
    }

    // ── State accessors ───────────────────────────────────────────────

    /// Current interaction mode.
    pub fn mode(&self) -> PaletteMode {
        self.mode
    }

    /// Current query / input text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Index of the highlighted item in the current `items` slice.
    pub fn selected(&self) -> usize {
        self.selected
    }

    // ── Mutators ──────────────────────────────────────────────────────

    /// Replace the item list (call this after refiltering the source list
    /// in response to a [`DualModePaletteEvent::QueryChanged`]).
    ///
    /// Selection and scroll offset are reset to 0 on each call.
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.items = items;
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Toggle between `List` and `Input` modes without clearing the query.
    pub fn toggle_mode(&mut self) -> PaletteMode {
        self.mode = match self.mode {
            PaletteMode::List => PaletteMode::Input,
            PaletteMode::Input => PaletteMode::List,
        };
        self.mode
    }

    // ── Render ────────────────────────────────────────────────────────

    /// Paint the palette modal inside `rect`.
    ///
    /// Call this from `AppLogic::render` while the controller is open.
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        let palette = self.build_palette();
        backend.draw_palette(rect, &palette);
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the state machine with a backend-neutral `UiEvent`.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)`.
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> DualModePaletteEvent {
        let result = match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            _ => DualModePaletteEvent::Ignored,
        };
        // Sync scroll after any selection change.
        if matches!(result, DualModePaletteEvent::Consumed) {
            self.sync_scroll(visible_rows);
        }
        result
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn handle_key(&mut self, key: &Key, modifiers: &Modifiers) -> DualModePaletteEvent {
        let ctrl = modifiers.ctrl;

        match self.mode {
            PaletteMode::Input => self.handle_key_input_mode(key, ctrl),
            PaletteMode::List => self.handle_key_list_mode(key, modifiers),
        }
    }

    /// Key dispatch when the palette is in `Input` mode.
    /// `ctrl` is already extracted from modifiers.
    fn handle_key_input_mode(&mut self, key: &Key, ctrl: bool) -> DualModePaletteEvent {
        match key {
            Key::Named(NamedKey::Escape) => DualModePaletteEvent::Cancelled,

            // Enter → confirm the text.
            Key::Named(NamedKey::Enter) => DualModePaletteEvent::TextConfirmed {
                value: self.query.clone(),
            },

            // Tab → toggle to List mode.
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                let new_mode = self.toggle_mode();
                DualModePaletteEvent::ModeToggled { new_mode }
            }

            // Backspace — delete character to the left of cursor.
            Key::Named(NamedKey::Backspace) if !ctrl => {
                if self.cursor_char > 0 {
                    let chars: Vec<char> = self.query.chars().collect();
                    let mut new_query = String::new();
                    for (i, &c) in chars.iter().enumerate() {
                        if i != self.cursor_char - 1 {
                            new_query.push(c);
                        }
                    }
                    self.cursor_char -= 1;
                    self.query = new_query;
                    DualModePaletteEvent::QueryChanged {
                        value: self.query.clone(),
                    }
                } else {
                    DualModePaletteEvent::Consumed
                }
            }

            // Left arrow — move cursor left.
            Key::Named(NamedKey::Left) if !ctrl => {
                if self.cursor_char > 0 {
                    self.cursor_char -= 1;
                }
                DualModePaletteEvent::Consumed
            }

            // Right arrow — move cursor right.
            Key::Named(NamedKey::Right) if !ctrl => {
                let char_count = self.query.chars().count();
                if self.cursor_char < char_count {
                    self.cursor_char += 1;
                }
                DualModePaletteEvent::Consumed
            }

            // Home — jump to start.
            Key::Named(NamedKey::Home) => {
                self.cursor_char = 0;
                DualModePaletteEvent::Consumed
            }

            // End — jump to end.
            Key::Named(NamedKey::End) => {
                self.cursor_char = self.query.chars().count();
                DualModePaletteEvent::Consumed
            }

            // Printable characters — insert at cursor.
            Key::Char(c) if !ctrl => {
                let chars: Vec<char> = self.query.chars().collect();
                let mut new_query = String::new();
                for (i, &ch) in chars.iter().enumerate() {
                    if i == self.cursor_char {
                        new_query.push(*c);
                    }
                    new_query.push(ch);
                }
                if self.cursor_char >= chars.len() {
                    new_query.push(*c);
                }
                self.cursor_char += 1;
                self.query = new_query;
                DualModePaletteEvent::QueryChanged {
                    value: self.query.clone(),
                }
            }

            _ => DualModePaletteEvent::Ignored,
        }
    }

    /// Key dispatch when the palette is in `List` mode.
    /// Scroll synchronisation happens in [`handle`](Self::handle) after this
    /// returns, so `visible_rows` is not passed here.
    fn handle_key_list_mode(&mut self, key: &Key, modifiers: &Modifiers) -> DualModePaletteEvent {
        let ctrl = modifiers.ctrl;

        match key {
            Key::Named(NamedKey::Escape) => DualModePaletteEvent::Cancelled,

            // Enter → activate selected item.
            Key::Named(NamedKey::Enter) => {
                if self.items.is_empty() {
                    DualModePaletteEvent::Consumed
                } else {
                    DualModePaletteEvent::ItemConfirmed { idx: self.selected }
                }
            }

            // Tab → toggle to Input mode.
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                let new_mode = self.toggle_mode();
                DualModePaletteEvent::ModeToggled { new_mode }
            }

            // Arrow up / k — move selection up.
            Key::Named(NamedKey::Up) => {
                self.selected = self.selected.saturating_sub(1);
                DualModePaletteEvent::Consumed
            }
            Key::Char('k') if !ctrl => {
                self.selected = self.selected.saturating_sub(1);
                DualModePaletteEvent::Consumed
            }

            // Arrow down / j — move selection down.
            Key::Named(NamedKey::Down) => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1).min(self.items.len() - 1);
                }
                DualModePaletteEvent::Consumed
            }
            Key::Char('j') if !ctrl => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1).min(self.items.len() - 1);
                }
                DualModePaletteEvent::Consumed
            }

            // Backspace — delete last query character.
            Key::Named(NamedKey::Backspace) if !ctrl => {
                if !self.query.is_empty() {
                    self.query.pop();
                    self.cursor_char = self.query.chars().count();
                    DualModePaletteEvent::QueryChanged {
                        value: self.query.clone(),
                    }
                } else {
                    DualModePaletteEvent::Consumed
                }
            }

            // Printable characters — append to query.
            Key::Char(c) if !ctrl => {
                // In List mode typing appends to the search query (no cursor
                // movement — always appends to the end like a search box).
                self.query.push(*c);
                self.cursor_char = self.query.chars().count();
                // Reset selection when query changes.
                self.selected = 0;
                self.scroll_top = 0;
                DualModePaletteEvent::QueryChanged {
                    value: self.query.clone(),
                }
            }

            _ => DualModePaletteEvent::Ignored,
        }
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
    fn sync_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        }
        if self.selected >= self.scroll_top + visible_rows {
            self.scroll_top = self.selected + 1 - visible_rows;
        }
    }

    /// Build the `Palette` descriptor for the current state.
    fn build_palette(&self) -> Palette {
        // In Input mode, optionally augment the title with the input_label.
        let title = match (self.mode, &self.input_label) {
            (PaletteMode::Input, Some(label)) => format!("{} {}", self.title, label),
            _ => self.title.clone(),
        };

        // Cursor byte offset from char index.
        let query_cursor = self
            .query
            .char_indices()
            .nth(self.cursor_char)
            .map(|(b, _)| b)
            .unwrap_or(self.query.len());

        Palette {
            id: self.id.clone(),
            title,
            query: self.query.clone(),
            query_cursor,
            items: if self.mode == PaletteMode::Input {
                // Hide the item list in Input mode.
                vec![]
            } else {
                self.items.clone()
            },
            selected_idx: self.selected,
            scroll_offset: self.scroll_top,
            total_count: if self.mode == PaletteMode::Input {
                0
            } else {
                self.items.len()
            },
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: self.mode,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StyledSpan, StyledText};

    fn item(text: &str) -> PaletteItem {
        PaletteItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(text)],
            },
            detail: None,
            icon: None,
            match_positions: vec![],
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn key_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn ctrl_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    // ── Initial state ─────────────────────────────────────────────────

    #[test]
    fn starts_in_list_mode() {
        let ctrl = DualModePaletteController::new("T", None, vec![]);
        assert_eq!(ctrl.mode(), PaletteMode::List);
    }

    #[test]
    fn with_mode_overrides_initial_mode() {
        let ctrl = DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        assert_eq!(ctrl.mode(), PaletteMode::Input);
    }

    // ── Tab toggles mode ──────────────────────────────────────────────

    #[test]
    fn tab_toggles_list_to_input() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Tab)), 10);
        assert_eq!(ctrl.mode(), PaletteMode::Input);
        assert_eq!(
            ev,
            DualModePaletteEvent::ModeToggled {
                new_mode: PaletteMode::Input
            }
        );
    }

    #[test]
    fn tab_toggles_input_to_list() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Tab)), 10);
        assert_eq!(ctrl.mode(), PaletteMode::List);
        assert_eq!(
            ev,
            DualModePaletteEvent::ModeToggled {
                new_mode: PaletteMode::List
            }
        );
    }

    #[test]
    fn tab_preserves_query_text() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        ctrl.handle(&key_ev(Key::Char('a')), 10);
        ctrl.handle(&key_ev(Key::Char('b')), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Tab)), 10); // → Input
        assert_eq!(ctrl.query(), "ab");
    }

    // ── List mode key dispatch ────────────────────────────────────────

    #[test]
    fn escape_in_list_mode_cancels() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Escape)), 10);
        assert_eq!(ev, DualModePaletteEvent::Cancelled);
    }

    #[test]
    fn char_in_list_mode_appends_to_query() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        ctrl.handle(&key_ev(Key::Char('f')), 10);
        ctrl.handle(&key_ev(Key::Char('o')), 10);
        assert_eq!(ctrl.query(), "fo");
    }

    #[test]
    fn char_in_list_mode_emits_query_changed() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle(&key_ev(Key::Char('x')), 10);
        assert_eq!(ev, DualModePaletteEvent::QueryChanged { value: "x".into() });
    }

    #[test]
    fn backspace_in_list_mode_removes_last_char() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        ctrl.handle(&key_ev(Key::Char('h')), 10);
        ctrl.handle(&key_ev(Key::Char('i')), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Backspace)), 10);
        assert_eq!(ctrl.query(), "h");
    }

    #[test]
    fn down_moves_selection() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Down)), 10);
        assert_eq!(ctrl.selected(), 1);
    }

    #[test]
    fn up_clamps_at_zero() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Up)), 10);
        assert_eq!(ctrl.selected(), 0);
    }

    #[test]
    fn enter_in_list_mode_confirms_item() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Down)), 10);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(ev, DualModePaletteEvent::ItemConfirmed { idx: 1 });
    }

    #[test]
    fn enter_on_empty_list_is_consumed() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(ev, DualModePaletteEvent::Consumed);
    }

    #[test]
    fn ctrl_char_is_ignored_in_list_mode() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle(&ctrl_ev(Key::Char('c')), 10);
        assert_eq!(ev, DualModePaletteEvent::Ignored);
    }

    // ── Input mode key dispatch ───────────────────────────────────────

    #[test]
    fn escape_in_input_mode_cancels() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Escape)), 10);
        assert_eq!(ev, DualModePaletteEvent::Cancelled);
    }

    #[test]
    fn enter_in_input_mode_confirms_text() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        ctrl.handle(&key_ev(Key::Char('m')), 10);
        ctrl.handle(&key_ev(Key::Char('y')), 10);
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(
            ev,
            DualModePaletteEvent::TextConfirmed { value: "my".into() }
        );
    }

    #[test]
    fn char_in_input_mode_inserts_at_cursor() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        ctrl.handle(&key_ev(Key::Char('a')), 10);
        ctrl.handle(&key_ev(Key::Char('c')), 10);
        // Move cursor left (between 'a' and 'c'), then insert 'b'.
        ctrl.handle(&key_ev(Key::Named(NamedKey::Left)), 10);
        ctrl.handle(&key_ev(Key::Char('b')), 10);
        assert_eq!(ctrl.query(), "abc");
    }

    #[test]
    fn backspace_in_input_mode_removes_char_before_cursor() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        ctrl.handle(&key_ev(Key::Char('a')), 10);
        ctrl.handle(&key_ev(Key::Char('b')), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Backspace)), 10);
        assert_eq!(ctrl.query(), "a");
    }

    #[test]
    fn home_moves_cursor_to_start() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        ctrl.handle(&key_ev(Key::Char('a')), 10);
        ctrl.handle(&key_ev(Key::Char('b')), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Home)), 10);
        // Insert at start.
        ctrl.handle(&key_ev(Key::Char('x')), 10);
        assert_eq!(ctrl.query(), "xab");
    }

    #[test]
    fn end_moves_cursor_to_end() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        ctrl.handle(&key_ev(Key::Char('a')), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Home)), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::End)), 10);
        ctrl.handle(&key_ev(Key::Char('z')), 10);
        assert_eq!(ctrl.query(), "az");
    }

    // ── build_palette ─────────────────────────────────────────────────

    #[test]
    fn build_palette_list_mode_includes_items() {
        let items = vec![item("foo"), item("bar")];
        let ctrl = DualModePaletteController::new("T", None, items);
        let pal = ctrl.build_palette();
        assert_eq!(pal.mode, PaletteMode::List);
        assert_eq!(pal.items.len(), 2);
    }

    #[test]
    fn build_palette_input_mode_hides_items() {
        let items = vec![item("foo"), item("bar")];
        let ctrl = DualModePaletteController::new("T", None, items).with_mode(PaletteMode::Input);
        let pal = ctrl.build_palette();
        assert_eq!(pal.mode, PaletteMode::Input);
        assert!(pal.items.is_empty(), "items should be hidden in Input mode");
    }

    #[test]
    fn build_palette_input_mode_augments_title_with_label() {
        let ctrl = DualModePaletteController::new("Branches", Some("New branch:".into()), vec![])
            .with_mode(PaletteMode::Input);
        let pal = ctrl.build_palette();
        assert!(
            pal.title.contains("New branch:"),
            "input label should appear in title: {:?}",
            pal.title
        );
    }

    #[test]
    fn set_items_resets_selection_and_scroll() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Down)), 10);
        ctrl.handle(&key_ev(Key::Named(NamedKey::Down)), 10);
        assert_eq!(ctrl.selected(), 2);
        ctrl.set_items(vec![item("x")]);
        assert_eq!(ctrl.selected(), 0);
    }
}
