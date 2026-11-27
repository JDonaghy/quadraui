//! [`DualModePalette`] — a palette with two interaction modes on the
//! widget contract.
//!
//! - **List mode** — the standard search-and-select behaviour: typing
//!   filters the item list; `Enter` confirms the selected row.
//! - **Input mode** — the query field is a standalone free-text input
//!   (e.g. "create new branch name"); `Enter` confirms the raw text.
//!
//! Press `Tab` to switch between modes at any time without clearing the
//! query text.
//!
//! The widget composes [`Palette`]: it paints through the palette's own
//! [`Primitive`] and routes input through the palette-host key prelude
//! every palette host shares, which hands query editing and list
//! navigation to the palette's own [`Interactive`]. What it adds is the
//! mode switch and what Enter confirms in each mode.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // In AppLogic::render / frame_tree: paint and register the palette.
//! frame.push(Surface::Widget { rect, widget: Box::new(palette.clone()) });
//! tree.push(palette.id, rect, Box::new(palette.clone()));
//!
//! // In AppLogic::handle: store the change, act on the confirmations.
//! if let UiEvent::DualModePalette(_, action) = event {
//!     palette.apply(&action, visible_rows);
//!     match action {
//!         DualModePaletteAction::ItemConfirmed { idx } => { /* switch to branch */ }
//!         DualModePaletteAction::TextConfirmed { value } => { /* create branch */ }
//!         DualModePaletteAction::QueryChanged { value } => { /* refilter items */ }
//!         DualModePaletteAction::Cancelled => { /* close */ }
//!         _ => {}
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::compose::palette_host::{route_palette_host, PaletteHostInput};
use crate::compose::Handled;
use crate::interactive::{
    EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::palette::{PalettePrimitiveLayout, PaletteState};
use crate::{
    Key, NamedKey, Palette, PaletteHit, PaletteItem, PaletteMode, Rect, UiEvent, WidgetId,
};

/// What a [`DualModePalette`] reports. [`DualModePalette::apply`] stores
/// the query, mode and highlight changes; the confirmations and the
/// dismissal are the host's to act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DualModePaletteAction {
    /// In List mode: the user confirmed the highlighted item. `idx` is the
    /// index into the `items` the palette shows.
    ItemConfirmed { idx: usize },
    /// In Input mode: the user pressed `Enter` to confirm the free-text
    /// value. `value` is the current query string.
    TextConfirmed { value: String },
    /// The query text changed. The app refilters its source list and
    /// replaces the items ([`DualModePalette::set_items`]).
    QueryChanged { value: String },
    /// The active mode toggled between `List` and `Input`.
    ModeToggled { new_mode: PaletteMode },
    /// The palette was dismissed (`Escape`).
    Cancelled,
    /// The highlight moved to item `idx`. The host writes it onto
    /// [`DualModePalette::selected`].
    SelectionChanged { idx: usize },
}

impl WidgetAction for DualModePaletteAction {
    /// Rides [`UiEvent::DualModePalette`] under the palette's own id.
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::DualModePalette(id, self)
    }
}

// ── Widget ────────────────────────────────────────────────────────────────

/// A palette that switches between list search and free-text input.
///
/// Every field is visible state the host owns: it applies the
/// [`DualModePaletteAction`]s the palette reports.
#[derive(Debug, Clone, PartialEq)]
pub struct DualModePalette {
    /// The palette's id, also the painted [`Palette`]'s.
    pub id: WidgetId,
    /// Title shown in the palette chrome.
    pub title: String,
    /// Label appended to the title in `Input` mode (e.g. `"New branch:"`);
    /// `None` keeps the same title in both modes.
    pub input_label: Option<String>,
    /// The active mode.
    pub mode: PaletteMode,
    /// The query / text-input string.
    pub query: String,
    /// Caret byte offset in `query` — the palette's own editing moves its
    /// live caret from here.
    pub query_cursor: usize,
    /// Filtered, displayable items (app-managed). Hidden in `Input` mode.
    pub items: Vec<PaletteItem>,
    /// Index of the highlighted row in `items`.
    pub selected: usize,
    /// First visible row (scroll offset into `items`).
    pub scroll_top: usize,
    /// Whether an unmodified `j`/`k` moves the highlight in `List` mode
    /// instead of typing into the query. Off by default, so every letter
    /// can be typed into a search.
    pub vim_keys: bool,
}

/// [`Interactive::State`] for [`DualModePalette`]: the composed palette's
/// own editing state (the live caret and selection anchor).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DualModePaletteState {
    /// The composed palette's state.
    pub palette: PaletteState,
}

impl DualModePalette {
    /// A palette in `List` mode with an empty query.
    pub fn new(
        id: WidgetId,
        title: impl Into<String>,
        input_label: Option<String>,
        items: Vec<PaletteItem>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            input_label,
            mode: PaletteMode::List,
            query: String::new(),
            query_cursor: 0,
            items,
            selected: 0,
            scroll_top: 0,
            vim_keys: false,
        }
    }

    /// This palette starting in `mode`.
    pub fn with_mode(mut self, mode: PaletteMode) -> Self {
        self.mode = mode;
        self
    }

    /// This palette with vim-style `j`/`k` highlight moves on or off
    /// ([`Self::vim_keys`]).
    pub fn with_vim_keys(mut self, vim_keys: bool) -> Self {
        self.vim_keys = vim_keys;
        self
    }

    /// Replace the items (after refiltering the source list on a
    /// [`DualModePaletteAction::QueryChanged`]); the highlight and scroll
    /// return to the top.
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.items = items;
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Store a change this palette reported: a highlight move scrolls it
    /// into a `visible_rows`-tall list, a query edit moves the caret to
    /// the query's end and, in `List` mode, the highlight back to the top,
    /// and a mode toggle switches the mode. A confirmation or a dismissal
    /// changes nothing here.
    pub fn apply(&mut self, action: &DualModePaletteAction, visible_rows: usize) {
        match action {
            DualModePaletteAction::SelectionChanged { idx } => {
                self.selected = *idx;
                self.scroll_top =
                    crate::behaviour::scroll::ensure_visible(self.scroll_top, visible_rows, *idx);
            }
            DualModePaletteAction::QueryChanged { value } => {
                self.query = value.clone();
                self.query_cursor = value.len();
                if self.mode == PaletteMode::List {
                    self.selected = 0;
                    self.scroll_top = 0;
                }
            }
            DualModePaletteAction::ModeToggled { new_mode } => self.mode = *new_mode,
            _ => {}
        }
    }

    /// Move [`Self::query_cursor`] to `state`'s live caret — for a host
    /// that keeps the palette's state itself
    /// ([`crate::compose::drive_ui_event`]) and so paints without the
    /// runtime handing that state to the paint.
    pub fn sync_caret(&mut self, state: &DualModePaletteState) {
        self.query_cursor = self.caret(Some(&state.palette)).min(self.query.len());
    }

    /// The other mode.
    fn toggled(&self) -> PaletteMode {
        match self.mode {
            PaletteMode::List => PaletteMode::Input,
            PaletteMode::Input => PaletteMode::List,
        }
    }

    /// The [`Palette`] this widget paints, with the query caret at
    /// `caret`: `Input` mode hides the list and appends
    /// [`Self::input_label`] to the title.
    pub fn palette(&self, caret: usize) -> Palette {
        let input = self.mode == PaletteMode::Input;
        let title = match (input, &self.input_label) {
            (true, Some(label)) => format!("{} {}", self.title, label),
            _ => self.title.clone(),
        };
        Palette {
            id: self.id,
            title,
            query: self.query.clone(),
            query_cursor: caret.min(self.query.len()),
            items: if input {
                Vec::new()
            } else {
                self.items.clone()
            },
            selected_idx: self.selected,
            scroll_offset: self.scroll_top,
            total_count: if input { 0 } else { self.items.len() },
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: self.mode,
        }
    }

    /// The caret to paint: the palette's live one, else the descriptor's.
    fn caret(&self, state: Option<&PaletteState>) -> usize {
        state.and_then(|s| s.cursor).unwrap_or(self.query_cursor)
    }

    /// Report a move of the highlight to `idx`, or nothing when it is
    /// already there.
    fn select(&self, idx: usize) -> Handled<DualModePaletteAction> {
        if idx == self.selected || self.items.is_empty() {
            Handled::Consumed
        } else {
            Handled::Event(DualModePaletteAction::SelectionChanged { idx })
        }
    }
}

impl A11yPrimitive for DualModePalette {
    fn a11y_info(&self) -> A11yInfo {
        self.palette(self.query_cursor).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.palette(self.query_cursor).a11y_children()
    }
}

impl Primitive for DualModePalette {
    type Layout = PalettePrimitiveLayout;
    type Event = DualModePaletteAction;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.palette(0), style)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.palette(self.query_cursor), ctx, avail)
    }

    /// The palette's own layout for `rect`.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> PalettePrimitiveLayout {
        Primitive::layout(&self.palette(self.query_cursor), ctx, rect)
    }

    fn paint(
        &self,
        layout: &PalettePrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env
            .widget_state_as::<DualModePaletteState>()
            .map(|s| &s.palette);
        Primitive::paint(&self.palette(self.caret(state)), layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &PalettePrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env
            .widget_state_as::<DualModePaletteState>()
            .map(|s| &s.palette);
        Primitive::paint_cells(&self.palette(self.caret(state)), layout, cells, env);
    }
}

impl Interactive for DualModePalette {
    type State = DualModePaletteState;
    type Action = DualModePaletteAction;

    /// One stop: the palette is modal, so keys go to it as a whole.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn sub_regions(&self, layout: &PalettePrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.palette(self.query_cursor), layout)
    }

    /// Tab toggles the mode, Escape cancels, and Enter confirms the
    /// highlighted item (List) or the typed text (Input). With
    /// [`Self::vim_keys`], an unmodified `j`/`k` moves the highlight in
    /// List mode; the pointer hovering a row highlights it. Everything
    /// else is the palette's own input — query editing and list
    /// navigation — reported as the change it names.
    fn handle(
        &self,
        state: &mut DualModePaletteState,
        event: &RoutedEvent,
        layout: &PalettePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<DualModePaletteAction> {
        let list = self.mode == PaletteMode::List;
        match event {
            RoutedEvent::Key(key) => {
                let modified = key.modifiers.ctrl || key.modifiers.cmd || key.modifiers.alt;
                if matches!(key.key, Key::Named(NamedKey::Tab | NamedKey::BackTab)) && !modified {
                    return Handled::Event(DualModePaletteAction::ModeToggled {
                        new_mode: self.toggled(),
                    });
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => {
                return match layout.hit_test_local(local.x, local.y) {
                    PaletteHit::Item(idx) if idx < self.items.len() && idx != self.selected => {
                        Handled::Event(DualModePaletteAction::SelectionChanged { idx })
                    }
                    _ => Handled::Ignored,
                };
            }
            _ => {}
        }
        let palette = self.palette(self.caret(Some(&state.palette)));
        route_palette_host(
            &palette,
            &mut state.palette,
            event,
            layout,
            cx,
            (self.items.len(), self.selected),
            list && self.vim_keys,
            |input| match input {
                PaletteHostInput::Cancel => Handled::Event(DualModePaletteAction::Cancelled),
                PaletteHostInput::Confirm(None) if !list => {
                    Handled::Event(DualModePaletteAction::TextConfirmed {
                        value: self.query.clone(),
                    })
                }
                PaletteHostInput::Confirm(None) if self.items.is_empty() => Handled::Consumed,
                PaletteHostInput::Confirm(None) => {
                    Handled::Event(DualModePaletteAction::ItemConfirmed { idx: self.selected })
                }
                PaletteHostInput::Confirm(Some(idx)) if list => {
                    Handled::Event(DualModePaletteAction::ItemConfirmed { idx })
                }
                PaletteHostInput::Confirm(Some(_)) => Handled::Consumed,
                PaletteHostInput::Select(idx) => self.select(idx),
                PaletteHostInput::QueryChanged(value) => {
                    Handled::Event(DualModePaletteAction::QueryChanged { value })
                }
            },
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{drive_ui_event, widget_host};
    use crate::{Backend, Modifiers, MouseButton, PALETTE_CHROME_ROWS};
    use crate::{StyledSpan, StyledText};

    /// List rows the tests' palette shows.
    const VISIBLE_ROWS: usize = 10;

    /// A host that keeps a [`DualModePalette`] and its state, drives it
    /// through its own [`Interactive`] and applies what it reports, the
    /// way an app does. Highlight moves are stored and answer
    /// [`Handled::Consumed`]; every other report is returned.
    struct Host {
        palette: DualModePalette,
        state: DualModePaletteState,
    }

    impl Host {
        fn new(input_label: Option<String>, items: Vec<PaletteItem>) -> Self {
            Self {
                palette: DualModePalette::new(WidgetId::new("dual"), "T", input_label, items),
                state: DualModePaletteState::default(),
            }
        }

        fn with_mode(mut self, mode: PaletteMode) -> Self {
            self.palette = self.palette.with_mode(mode);
            self
        }

        fn with_vim_keys(mut self) -> Self {
            self.palette = self.palette.with_vim_keys(true);
            self
        }

        /// A keystroke or paste, on a cell grid tall enough for
        /// [`VISIBLE_ROWS`] rows.
        fn key(&mut self, event: &UiEvent) -> Handled<DualModePaletteAction> {
            let rect = Rect::new(0.0, 0.0, 80.0, (VISIBLE_ROWS + PALETTE_CHROME_ROWS) as f32);
            let handled = widget_host::drive_in_cells(&self.palette, &mut self.state, event, rect)
                .map_or(Handled::Ignored, |driven| driven.handled);
            self.settle(handled)
        }

        /// A pointer event against the palette laid out at `rect`.
        fn pointer(
            &mut self,
            event: &UiEvent,
            backend: &dyn Backend,
            rect: Rect,
        ) -> Handled<DualModePaletteAction> {
            let handled = drive_ui_event(&self.palette, &mut self.state, event, backend, rect)
                .map_or(Handled::Ignored, |driven| driven.handled);
            self.settle(handled)
        }

        fn settle(
            &mut self,
            handled: Handled<DualModePaletteAction>,
        ) -> Handled<DualModePaletteAction> {
            if let Handled::Event(action) = &handled {
                self.palette.apply(action, VISIBLE_ROWS);
                self.palette.sync_caret(&self.state);
            }
            match handled {
                Handled::Event(DualModePaletteAction::SelectionChanged { .. }) => Handled::Consumed,
                other => other,
            }
        }
    }

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

    fn cmd_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers {
                cmd: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    // ── Initial state ─────────────────────────────────────────────────

    #[test]
    fn starts_in_list_mode() {
        let ctrl = Host::new(None, vec![]);
        assert_eq!(ctrl.palette.mode, PaletteMode::List);
    }

    #[test]
    fn with_mode_overrides_initial_mode() {
        let ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        assert_eq!(ctrl.palette.mode, PaletteMode::Input);
    }

    // ── Tab toggles mode ──────────────────────────────────────────────

    #[test]
    fn tab_toggles_list_to_input() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Tab)));
        assert_eq!(ctrl.palette.mode, PaletteMode::Input);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ModeToggled {
                new_mode: PaletteMode::Input
            })
        );
    }

    #[test]
    fn tab_toggles_input_to_list() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Tab)));
        assert_eq!(ctrl.palette.mode, PaletteMode::List);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ModeToggled {
                new_mode: PaletteMode::List
            })
        );
    }

    #[test]
    fn tab_preserves_query_text() {
        let mut ctrl = Host::new(None, vec![]);
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Char('b')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Tab))); // → Input
        assert_eq!(ctrl.palette.query, "ab");
    }

    // ── List mode key dispatch ────────────────────────────────────────

    #[test]
    fn escape_in_list_mode_cancels() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Escape)));
        assert_eq!(ev, Handled::Event(DualModePaletteAction::Cancelled));
    }

    #[test]
    fn char_in_list_mode_appends_to_query() {
        let mut ctrl = Host::new(None, vec![]);
        let _ = ctrl.key(&key_ev(Key::Char('f')));
        let _ = ctrl.key(&key_ev(Key::Char('o')));
        assert_eq!(ctrl.palette.query, "fo");
    }

    #[test]
    fn char_in_list_mode_emits_query_changed() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&key_ev(Key::Char('x')));
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged { value: "x".into() })
        );
    }

    #[test]
    fn backspace_in_list_mode_removes_last_char() {
        let mut ctrl = Host::new(None, vec![]);
        let _ = ctrl.key(&key_ev(Key::Char('h')));
        let _ = ctrl.key(&key_ev(Key::Char('i')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Backspace)));
        assert_eq!(ctrl.palette.query, "h");
    }

    #[test]
    fn down_moves_selection() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Down)));
        assert_eq!(ctrl.palette.selected, 1);
    }

    #[test]
    fn up_clamps_at_zero() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = Host::new(None, items);
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Up)));
        assert_eq!(ctrl.palette.selected, 0);
    }

    #[test]
    fn enter_in_list_mode_confirms_item() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = Host::new(None, items);
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Down)));
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Enter)));
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ItemConfirmed { idx: 1 })
        );
    }

    #[test]
    fn enter_on_empty_list_is_consumed() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Enter)));
        assert_eq!(ev, Handled::Consumed);
    }

    #[test]
    fn ctrl_char_is_ignored_in_list_mode() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&ctrl_ev(Key::Char('w')));
        assert_eq!(ev, Handled::Ignored);
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must be ignored the same way.
    #[test]
    fn cmd_char_is_ignored_in_list_mode() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&cmd_ev(Key::Char('w')));
        assert_eq!(ev, Handled::Ignored);
    }

    // ── Input mode key dispatch ───────────────────────────────────────

    #[test]
    fn escape_in_input_mode_cancels() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Escape)));
        assert_eq!(ev, Handled::Event(DualModePaletteAction::Cancelled));
    }

    #[test]
    fn enter_in_input_mode_confirms_text() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('m')));
        let _ = ctrl.key(&key_ev(Key::Char('y')));
        let ev = ctrl.key(&key_ev(Key::Named(NamedKey::Enter)));
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::TextConfirmed { value: "my".into() })
        );
    }

    #[test]
    fn char_in_input_mode_inserts_at_cursor() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Char('c')));
        // Move cursor left (between 'a' and 'c'), then insert 'b'.
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Left)));
        let _ = ctrl.key(&key_ev(Key::Char('b')));
        assert_eq!(ctrl.palette.query, "abc");
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be inserted into the query in Input mode.
    #[test]
    fn cmd_char_is_ignored_in_input_mode() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.key(&cmd_ev(Key::Char('f')));
        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.palette.query, "");
    }

    #[test]
    fn backspace_in_input_mode_removes_char_before_cursor() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Char('b')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Backspace)));
        assert_eq!(ctrl.palette.query, "a");
    }

    #[test]
    fn home_moves_cursor_to_start() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Char('b')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Home)));
        // Insert at start.
        let _ = ctrl.key(&key_ev(Key::Char('x')));
        assert_eq!(ctrl.palette.query, "xab");
    }

    #[test]
    fn end_moves_cursor_to_end() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Home)));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::End)));
        let _ = ctrl.key(&key_ev(Key::Char('z')));
        assert_eq!(ctrl.palette.query, "az");
    }

    // ── build_palette ─────────────────────────────────────────────────

    #[test]
    fn build_palette_list_mode_includes_items() {
        let items = vec![item("foo"), item("bar")];
        let ctrl = Host::new(None, items);
        let pal = ctrl.palette.palette(ctrl.palette.query_cursor);
        assert_eq!(pal.mode, PaletteMode::List);
        assert_eq!(pal.items.len(), 2);
    }

    #[test]
    fn build_palette_input_mode_hides_items() {
        let items = vec![item("foo"), item("bar")];
        let ctrl = Host::new(None, items).with_mode(PaletteMode::Input);
        let pal = ctrl.palette.palette(ctrl.palette.query_cursor);
        assert_eq!(pal.mode, PaletteMode::Input);
        assert!(pal.items.is_empty(), "items should be hidden in Input mode");
    }

    #[test]
    fn build_palette_input_mode_augments_title_with_label() {
        let ctrl = Host::new(Some("New branch:".into()), vec![]).with_mode(PaletteMode::Input);
        let pal = ctrl.palette.palette(ctrl.palette.query_cursor);
        assert!(
            pal.title.contains("New branch:"),
            "input label should appear in title: {:?}",
            pal.title
        );
    }

    // ── ClipboardPaste in Input mode ──────────────────────────────────

    #[test]
    fn clipboard_paste_in_input_mode_inserts_at_cursor() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        // Type "ac", move cursor left, then paste "b" — result should be "abc".
        let _ = ctrl.key(&key_ev(Key::Char('a')));
        let _ = ctrl.key(&key_ev(Key::Char('c')));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Left)));
        let ev = ctrl.key(&UiEvent::ClipboardPaste("b".into()));
        assert_eq!(ctrl.palette.query, "abc");
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged {
                value: "abc".into()
            })
        );
    }

    #[test]
    fn clipboard_paste_appends_when_cursor_at_end() {
        let mut ctrl = Host::new(None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('h')));
        let _ = ctrl.key(&key_ev(Key::Char('i')));
        let ev = ctrl.key(&UiEvent::ClipboardPaste("!".into()));
        assert_eq!(ctrl.palette.query, "hi!");
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged {
                value: "hi!".into()
            })
        );
    }

    #[test]
    fn clipboard_paste_in_list_mode_refilters() {
        let mut ctrl = Host::new(None, vec![]);
        let ev = ctrl.key(&UiEvent::ClipboardPaste("xyz".into()));
        assert_eq!(ctrl.palette.query, "xyz");
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged {
                value: "xyz".into()
            })
        );
    }

    #[test]
    fn set_items_resets_selection_and_scroll() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Down)));
        let _ = ctrl.key(&key_ev(Key::Named(NamedKey::Down)));
        assert_eq!(ctrl.palette.selected, 2);
        ctrl.palette.set_items(vec![item("x")]);
        assert_eq!(ctrl.palette.selected, 0);
    }

    fn mouse_down(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x, y },
            modifiers: Modifiers::default(),
        }
    }

    fn mouse_moved(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseMoved {
            position: crate::Point { x, y },
            buttons: crate::event::ButtonMask::default(),
        }
    }

    /// `RecordingBackend::new()` measures in one-unit cells, so the
    /// palette resolves its cell layout: title at row 0, query at row 1,
    /// the query separator at row 2, items from row 3.
    fn recording_backend() -> crate::testing::RecordingBackend {
        crate::testing::RecordingBackend::new()
    }

    #[test]
    fn mouse_click_on_item_selects_it() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        assert_eq!(ctrl.palette.selected, 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // title(row 0) + query(row 1) + separator(row 2) -> items start
        // at row 3; item 2 is at row 5.
        let ev = ctrl.pointer(&mouse_down(5.0, 5.5), &backend, rect);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(
            ctrl.palette.selected, 2,
            "clicking item 2's row must update the selection"
        );
    }

    #[test]
    fn mouse_click_on_item_selects_it_at_nonzero_rect_origin() {
        // Regression guard (LESSONS.md): a LOCAL/ABSOLUTE frame mixup is
        // invisible when `rect`'s origin is `(0, 0)` — this test's rect
        // starts elsewhere so a missing `rect.x`/`rect.y` subtraction
        // would fail it.
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);

        let backend = recording_backend();
        let rect = Rect::new(5.0, 3.0, 40.0, 20.0);
        // Item 2's row is local y=5, so absolute y = rect.y + 5 = 8.
        let ev = ctrl.pointer(&mouse_down(10.0, 8.5), &backend, rect);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(ctrl.palette.selected, 2);
    }

    #[test]
    fn mouse_click_outside_every_row_leaves_the_highlight() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = Host::new(None, items);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.pointer(&mouse_down(5.0, 99.0), &backend, rect);

        // The palette is modal: it swallows the press without a change.
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(ctrl.palette.selected, 0);
    }

    #[test]
    fn mouse_moved_over_item_selects_it() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        assert_eq!(ctrl.palette.selected, 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // Same row geometry as `mouse_click_on_item_selects_it`: item 2
        // sits at row 5.
        let ev = ctrl.pointer(&mouse_moved(5.0, 5.5), &backend, rect);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(
            ctrl.palette.selected, 2,
            "hovering item 2's row must update the selection, same as a click"
        );
    }

    #[test]
    fn mouse_moved_over_the_already_selected_row_is_ignored() {
        // A stationary pointer must not churn `Handled::Consumed` (and so
        // a redraw) every frame it's polled.
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        assert_eq!(ctrl.palette.selected, 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // Item 0's row is at local y=2 (title + query rows above it).
        let ev = ctrl.pointer(&mouse_moved(5.0, 2.5), &backend, rect);

        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.palette.selected, 0);
    }

    #[test]
    fn mouse_moved_outside_any_item_row_is_ignored() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = Host::new(None, items);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.pointer(&mouse_moved(5.0, 99.0), &backend, rect);

        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.palette.selected, 0);
    }

    #[test]
    fn right_click_is_ignored() {
        let mut ctrl = Host::new(None, vec![item("a")]);
        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.pointer(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Right,
                position: crate::Point { x: 5.0, y: 2.5 },
                modifiers: Modifiers::default(),
            },
            &backend,
            rect,
        );
        assert_eq!(ev, Handled::Ignored);
    }

    // ── vim-style j/k ─────────────────────────────────────────────────

    #[test]
    fn j_and_k_type_into_the_list_query_by_default() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items);
        let _ = ctrl.key(&key_ev(Key::Char('j')));
        let ev = ctrl.key(&key_ev(Key::Char('k')));
        assert_eq!(ctrl.palette.query, "jk");
        assert_eq!(ctrl.palette.selected, 0);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged { value: "jk".into() })
        );
    }

    #[test]
    fn vim_keys_j_and_k_move_the_highlight_in_list_mode() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = Host::new(None, items).with_vim_keys();
        let _ = ctrl.key(&key_ev(Key::Char('j')));
        let _ = ctrl.key(&key_ev(Key::Char('j')));
        assert_eq!(ctrl.palette.selected, 2);
        let _ = ctrl.key(&key_ev(Key::Char('j')));
        assert_eq!(ctrl.palette.selected, 2, "j clamps at the last row");
        let _ = ctrl.key(&key_ev(Key::Char('k')));
        assert_eq!(ctrl.palette.selected, 1);
        assert_eq!(ctrl.palette.query, "");
    }

    #[test]
    fn vim_keys_type_in_input_mode() {
        let mut ctrl = Host::new(None, vec![item("a"), item("b")])
            .with_vim_keys()
            .with_mode(PaletteMode::Input);
        let _ = ctrl.key(&key_ev(Key::Char('j')));
        let _ = ctrl.key(&key_ev(Key::Char('k')));
        assert_eq!(ctrl.palette.query, "jk");
    }

    #[test]
    fn modified_j_never_moves_the_highlight() {
        let mut ctrl = Host::new(None, vec![item("a"), item("b")]).with_vim_keys();
        let ev = ctrl.key(&ctrl_ev(Key::Char('j')));
        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.palette.selected, 0);
    }

    // ── apply ─────────────────────────────────────────────────────────

    #[test]
    fn apply_scrolls_a_highlight_move_into_view() {
        let items = (0..30).map(|i| item(&format!("row{i}"))).collect();
        let mut palette = DualModePalette::new(WidgetId::new("p"), "T", None, items);
        palette.apply(&DualModePaletteAction::SelectionChanged { idx: 12 }, 5);
        assert_eq!(palette.selected, 12);
        assert_eq!(palette.scroll_top, 8);
    }

    #[test]
    fn apply_query_change_resets_the_list_highlight_only_in_list_mode() {
        let items = vec![item("a"), item("b")];
        let mut palette = DualModePalette::new(WidgetId::new("p"), "T", None, items);
        palette.selected = 1;
        palette.apply(
            &DualModePaletteAction::QueryChanged { value: "ab".into() },
            5,
        );
        assert_eq!((palette.query.as_str(), palette.query_cursor), ("ab", 2));
        assert_eq!(palette.selected, 0);

        palette.mode = PaletteMode::Input;
        palette.selected = 1;
        palette.apply(
            &DualModePaletteAction::QueryChanged { value: "x".into() },
            5,
        );
        assert_eq!(palette.selected, 1);
    }
}
