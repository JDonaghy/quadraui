//! [`DualModePalette`] — a palette with two interaction modes on the
//! widget contract, plus [`DualModePaletteController`], its thin
//! event-driven wrapper.
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
//! [`Primitive`] and routes input through the palette's [`Interactive`],
//! which owns the query editor (caret, selection, word moves) and the
//! list navigation. What it adds is the mode switch and what Enter
//! confirms in each mode.
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
//! match pal.handle_event(&event, visible_rows) {
//!     Handled::Event(DualModePaletteAction::ItemConfirmed { idx }) => { /* switch to branch */ }
//!     Handled::Event(DualModePaletteAction::TextConfirmed { value }) => { /* create branch */ }
//!     Handled::Event(DualModePaletteAction::QueryChanged { value }) => { /* refilter items */ }
//!     Handled::Event(DualModePaletteAction::ModeToggled { new_mode }) => { /* update hint */ }
//!     Handled::Event(DualModePaletteAction::Cancelled) => { /* close */ }
//!     _ => {}
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::compose::path_picker::is_chorded_typing;
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::interactive::{
    EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::palette::{PalettePrimitiveLayout, PaletteState};
use crate::{
    Backend, Key, MouseButton, NamedKey, Palette, PaletteEvent, PaletteHit, PaletteItem,
    PaletteMode, Rect, UiEvent, WidgetId,
};

/// What a [`DualModePalette`] reports, and the semantic payload of
/// [`Handled::Event`] returned by [`DualModePaletteController::handle_event`]
/// / `handle_mouse_event`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DualModePaletteAction {
    /// In List mode: the user confirmed the highlighted item. `idx` is the
    /// index into the `items` the palette shows.
    ItemConfirmed { idx: usize },
    /// In Input mode: the user pressed `Enter` to confirm the free-text
    /// value. `value` is the current query string.
    TextConfirmed { value: String },
    /// The query text changed. The app should refilter its source list and
    /// replace the items (the controller's
    /// [`DualModePaletteController::set_items`]).
    QueryChanged { value: String },
    /// The active mode toggled between `List` and `Input`.
    ModeToggled { new_mode: PaletteMode },
    /// The palette was dismissed (`Escape`).
    Cancelled,
    /// The highlight moved to item `idx`. The host writes it onto
    /// [`DualModePalette::selected`]; the controller applies it itself.
    SelectionChanged { idx: usize },
}

impl WidgetAction for DualModePaletteAction {
    /// Rides [`UiEvent::DualModePalette`] under the palette's own id.
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::DualModePalette(id, self)
    }
}

/// What happened after [`DualModePaletteController::handle`] processed an
/// event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[deprecated(
    since = "0.9.0",
    note = "use `DualModePaletteController::handle_event`'s `Handled<DualModePaletteAction>` instead — `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`, the other variants move to `DualModePaletteAction` under `Handled::Event`"
)]
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

/// Adapts a [`Handled<DualModePaletteAction>`] back to the deprecated
/// [`DualModePaletteEvent`] shape — shared by `handle`/`handle_mouse`'s
/// own thin-adapter bodies.
#[allow(deprecated)] // `DualModePaletteEvent` is `#[deprecated]`; this helper is its own shim.
fn handled_to_event(handled: Handled<DualModePaletteAction>) -> DualModePaletteEvent {
    match handled {
        Handled::Event(DualModePaletteAction::ItemConfirmed { idx }) => {
            DualModePaletteEvent::ItemConfirmed { idx }
        }
        Handled::Event(DualModePaletteAction::TextConfirmed { value }) => {
            DualModePaletteEvent::TextConfirmed { value }
        }
        Handled::Event(DualModePaletteAction::QueryChanged { value }) => {
            DualModePaletteEvent::QueryChanged { value }
        }
        Handled::Event(DualModePaletteAction::ModeToggled { new_mode }) => {
            DualModePaletteEvent::ModeToggled { new_mode }
        }
        Handled::Event(DualModePaletteAction::Cancelled) => DualModePaletteEvent::Cancelled,
        Handled::Event(_) | Handled::Consumed => DualModePaletteEvent::Consumed,
        Handled::Ignored => DualModePaletteEvent::Ignored,
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
        }
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
    /// highlighted item (List) or the typed text (Input). In List mode an
    /// unmodified `j`/`k` moves the highlight, and the pointer hovering a
    /// row highlights it. Everything else is the palette's own input —
    /// query editing and list navigation — reported as the change it
    /// names.
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
                match key.key {
                    Key::Named(NamedKey::Tab | NamedKey::BackTab) if !modified => {
                        return Handled::Event(DualModePaletteAction::ModeToggled {
                            new_mode: self.toggled(),
                        });
                    }
                    Key::Named(NamedKey::Escape) => {
                        return Handled::Event(DualModePaletteAction::Cancelled);
                    }
                    Key::Named(NamedKey::Enter) if list => {
                        return if self.items.is_empty() {
                            Handled::Consumed
                        } else {
                            Handled::Event(DualModePaletteAction::ItemConfirmed {
                                idx: self.selected,
                            })
                        };
                    }
                    Key::Named(NamedKey::Enter) => {
                        return Handled::Event(DualModePaletteAction::TextConfirmed {
                            value: self.query.clone(),
                        });
                    }
                    Key::Char('k') if list && !modified => {
                        return self.select(self.selected.saturating_sub(1));
                    }
                    Key::Char('j') if list && !modified => {
                        let last = self.items.len().saturating_sub(1);
                        return self.select((self.selected + 1).min(last));
                    }
                    _ if is_chorded_typing(key) => return Handled::Ignored,
                    _ => {}
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
        match Interactive::handle(&palette, &mut state.palette, event, layout, cx) {
            Handled::Event(PaletteEvent::QueryChanged { value }) => {
                Handled::Event(DualModePaletteAction::QueryChanged { value })
            }
            Handled::Event(PaletteEvent::SelectionChanged { idx }) => self.select(idx),
            Handled::Event(PaletteEvent::ItemConfirmed { idx }) if list => {
                Handled::Event(DualModePaletteAction::ItemConfirmed { idx })
            }
            Handled::Event(PaletteEvent::Closed) => {
                Handled::Event(DualModePaletteAction::Cancelled)
            }
            Handled::Event(_) | Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

// ── Controller ────────────────────────────────────────────────────────────

/// Event-driven wrapper over a [`DualModePalette`] and its state, for a
/// host that calls it with raw [`UiEvent`]s. It applies the highlight,
/// mode and query changes itself, and still reports the query and mode
/// changes so the app can refilter and update hints.
pub struct DualModePaletteController {
    widget: DualModePalette,
    state: DualModePaletteState,
}

impl DualModePaletteController {
    /// Create a new controller in `List` mode.
    ///
    /// - `title` — palette title string (shown in the title chrome).
    /// - `input_label` — optional label appended to the title bar when in
    ///   `Input` mode (e.g. `"New branch:"`). Pass `None` to keep the same
    ///   title in both modes.
    /// - `items` — initial item list (may be empty; replace with
    ///   [`set_items`](Self::set_items) as the app filters its source).
    pub fn new(
        title: impl Into<String>,
        input_label: impl Into<Option<String>>,
        items: Vec<PaletteItem>,
    ) -> Self {
        Self {
            widget: DualModePalette::new(
                WidgetId::new("dual_mode_palette"),
                title,
                input_label.into(),
                items,
            ),
            state: DualModePaletteState::default(),
        }
    }

    // ── Builder setters ───────────────────────────────────────────────

    /// Override the `WidgetId` used for the rendered `Palette`.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.widget.id = WidgetId::new(id.into());
        self
    }

    /// Start in `Input` mode instead of `List` mode.
    pub fn with_mode(mut self, mode: PaletteMode) -> Self {
        self.widget.mode = mode;
        self
    }

    /// The [`DualModePalette`] widget this controller drives.
    pub fn widget(&self) -> &DualModePalette {
        &self.widget
    }

    // ── State accessors ───────────────────────────────────────────────

    /// Current interaction mode.
    pub fn mode(&self) -> PaletteMode {
        self.widget.mode
    }

    /// Current query / input text.
    pub fn query(&self) -> &str {
        &self.widget.query
    }

    /// Index of the highlighted item in the current `items` slice.
    pub fn selected(&self) -> usize {
        self.widget.selected
    }

    // ── Mutators ──────────────────────────────────────────────────────

    /// Replace the item list (call this after refiltering the source list
    /// in response to a [`DualModePaletteAction::QueryChanged`]).
    ///
    /// Selection and scroll offset are reset to 0 on each call.
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.widget.items = items;
        self.widget.selected = 0;
        self.widget.scroll_top = 0;
    }

    /// Toggle between `List` and `Input` modes without clearing the query.
    pub fn toggle_mode(&mut self) -> PaletteMode {
        self.widget.mode = self.widget.toggled();
        self.widget.mode
    }

    // ── Render ────────────────────────────────────────────────────────

    /// The palette as painted, with the live caret.
    fn build_palette(&self) -> Palette {
        self.widget
            .palette(self.widget.caret(Some(&self.state.palette)))
    }

    /// Paint the palette modal inside `rect`.
    ///
    /// Call this from `AppLogic::render` while the controller is open.
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        crate::backend::DrawsPrimitives::draw_primitive(backend, rect, &self.build_palette());
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Apply one widget answer: the highlight, mode and query land on the
    /// stored palette; the highlight is the controller's alone, the query
    /// and mode changes are still reported.
    fn settle(
        &mut self,
        handled: Handled<DualModePaletteAction>,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        match handled {
            Handled::Event(DualModePaletteAction::SelectionChanged { idx }) => {
                self.widget.selected = idx;
                self.widget.scroll_top = crate::behaviour::scroll::ensure_visible(
                    self.widget.scroll_top,
                    visible_rows,
                    idx,
                );
                Handled::Consumed
            }
            Handled::Event(DualModePaletteAction::QueryChanged { value }) => {
                self.widget.query = value.clone();
                self.widget.query_cursor = self
                    .state
                    .palette
                    .cursor
                    .unwrap_or(value.len())
                    .min(value.len());
                if self.widget.mode == PaletteMode::List {
                    self.widget.selected = 0;
                    self.widget.scroll_top = 0;
                }
                Handled::Event(DualModePaletteAction::QueryChanged { value })
            }
            Handled::Event(DualModePaletteAction::ModeToggled { new_mode }) => {
                self.widget.mode = new_mode;
                Handled::Event(DualModePaletteAction::ModeToggled { new_mode })
            }
            other => other,
        }
    }

    /// Drive the palette with a backend-neutral `UiEvent`: keystrokes, and
    /// a bracketed paste while in `Input` mode.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)`.
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        let routed = match event {
            UiEvent::KeyPressed { .. } => true,
            UiEvent::ClipboardPaste(_) => self.widget.mode == PaletteMode::Input,
            _ => false,
        };
        if !routed {
            return Handled::Ignored;
        }
        let rect = Rect::new(
            0.0,
            0.0,
            80.0,
            (visible_rows + crate::compose::PALETTE_CHROME_ROWS) as f32,
        );
        let handled = widget_host::drive_in_cells(&self.widget, &mut self.state, event, rect)
            .map_or(Handled::Ignored, |driven| driven.handled);
        self.settle(handled, visible_rows)
    }

    /// Drive the state machine with a backend-neutral `UiEvent`.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<DualModePaletteAction>` instead of `DualModePaletteEvent`"
    )]
    #[allow(deprecated)] // `DualModePaletteEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> DualModePaletteEvent {
        handled_to_event(self.handle_event(event, visible_rows))
    }

    /// Route one pointer event through the widget at `rect`, reporting
    /// [`Handled::Consumed`] only when it landed on an item row.
    fn handle_pointer(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        let layout = crate::backend::DrawsPrimitives::layout_primitive(backend, rect, &self.widget);
        let Some(routed) = widget_host::routed_event(event, rect, &[]) else {
            return Handled::Ignored;
        };
        let RoutedEvent::Pointer { local, .. } = &routed else {
            return Handled::Ignored;
        };
        if !matches!(layout.hit_test_local(local.x, local.y), PaletteHit::Item(idx) if idx < self.widget.items.len())
        {
            return Handled::Ignored;
        }
        let units = crate::backend::primitive_layout_ctx(backend).units();
        let driven = widget_host::drive(
            &self.widget,
            self.widget.id,
            &mut self.state,
            &routed,
            &layout,
            units,
        );
        self.settle(driven.handled, visible_rows)
    }

    /// Drive the palette with a left `MouseDown` — the click-routing twin
    /// of [`Self::handle_event`], which only understands keyboard and
    /// paste events. A click on an item row selects it (it does not
    /// confirm it — that still takes `Enter`); every other event, or a
    /// press that misses every item row, is [`Handled::Ignored`].
    ///
    /// `rect` must be the same popup rect passed to [`Self::render`] this
    /// frame, and `backend` supplies the same metrics `render` painted
    /// with. `position` is **absolute**; the palette's layout is local to
    /// `rect`.
    pub fn handle_mouse_event(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        if !matches!(
            event,
            UiEvent::MouseDown {
                button: MouseButton::Left,
                ..
            }
        ) {
            return Handled::Ignored;
        }
        self.handle_pointer(event, backend, rect, visible_rows)
    }

    /// Drive the palette with a `MouseMoved` — the hover-tracking twin of
    /// [`Self::handle_mouse_event`]. Moving the pointer over an item row
    /// highlights it the same way a click does; a move that misses every
    /// row, or re-hits the already-highlighted one, is
    /// [`Handled::Ignored`], so a stationary pointer doesn't churn a
    /// redraw every frame.
    ///
    /// Same `rect`/`backend` contract as [`Self::handle_mouse_event`].
    pub fn handle_mouse_move_event(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        if !matches!(event, UiEvent::MouseMoved { .. }) {
            return Handled::Ignored;
        }
        self.handle_pointer(event, backend, rect, visible_rows)
    }

    /// Drive the state machine with a mouse `UiEvent`.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_mouse_event`, which returns `Handled<DualModePaletteAction>` instead of `DualModePaletteEvent`"
    )]
    #[allow(deprecated)] // `DualModePaletteEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle_mouse(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
        visible_rows: usize,
    ) -> DualModePaletteEvent {
        handled_to_event(self.handle_mouse_event(event, backend, rect, visible_rows))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;
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
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Tab)), 10);
        assert_eq!(ctrl.mode(), PaletteMode::Input);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ModeToggled {
                new_mode: PaletteMode::Input
            })
        );
    }

    #[test]
    fn tab_toggles_input_to_list() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Tab)), 10);
        assert_eq!(ctrl.mode(), PaletteMode::List);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ModeToggled {
                new_mode: PaletteMode::List
            })
        );
    }

    #[test]
    fn tab_preserves_query_text() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('b')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Tab)), 10); // → Input
        assert_eq!(ctrl.query(), "ab");
    }

    // ── List mode key dispatch ────────────────────────────────────────

    #[test]
    fn escape_in_list_mode_cancels() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Escape)), 10);
        assert_eq!(ev, Handled::Event(DualModePaletteAction::Cancelled));
    }

    #[test]
    fn char_in_list_mode_appends_to_query() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let _ = ctrl.handle_event(&key_ev(Key::Char('f')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('o')), 10);
        assert_eq!(ctrl.query(), "fo");
    }

    #[test]
    fn char_in_list_mode_emits_query_changed() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle_event(&key_ev(Key::Char('x')), 10);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged { value: "x".into() })
        );
    }

    #[test]
    fn backspace_in_list_mode_removes_last_char() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let _ = ctrl.handle_event(&key_ev(Key::Char('h')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('i')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Backspace)), 10);
        assert_eq!(ctrl.query(), "h");
    }

    #[test]
    fn down_moves_selection() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Down)), 10);
        assert_eq!(ctrl.selected(), 1);
    }

    #[test]
    fn up_clamps_at_zero() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Up)), 10);
        assert_eq!(ctrl.selected(), 0);
    }

    #[test]
    fn enter_in_list_mode_confirms_item() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Down)), 10);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::ItemConfirmed { idx: 1 })
        );
    }

    #[test]
    fn enter_on_empty_list_is_consumed() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(ev, Handled::Consumed);
    }

    #[test]
    fn ctrl_char_is_ignored_in_list_mode() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle_event(&ctrl_ev(Key::Char('c')), 10);
        assert_eq!(ev, Handled::Ignored);
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must be ignored the same way.
    #[test]
    fn cmd_char_is_ignored_in_list_mode() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        let ev = ctrl.handle_event(&cmd_ev(Key::Char('c')), 10);
        assert_eq!(ev, Handled::Ignored);
    }

    // ── Input mode key dispatch ───────────────────────────────────────

    #[test]
    fn escape_in_input_mode_cancels() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Escape)), 10);
        assert_eq!(ev, Handled::Event(DualModePaletteAction::Cancelled));
    }

    #[test]
    fn enter_in_input_mode_confirms_text() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('m')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('y')), 10);
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Enter)), 10);
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::TextConfirmed { value: "my".into() })
        );
    }

    #[test]
    fn char_in_input_mode_inserts_at_cursor() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('c')), 10);
        // Move cursor left (between 'a' and 'c'), then insert 'b'.
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Left)), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('b')), 10);
        assert_eq!(ctrl.query(), "abc");
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be inserted into the query in Input mode.
    #[test]
    fn cmd_char_is_ignored_in_input_mode() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let ev = ctrl.handle_event(&cmd_ev(Key::Char('f')), 10);
        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.query(), "");
    }

    #[test]
    fn backspace_in_input_mode_removes_char_before_cursor() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('b')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Backspace)), 10);
        assert_eq!(ctrl.query(), "a");
    }

    #[test]
    fn home_moves_cursor_to_start() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('b')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Home)), 10);
        // Insert at start.
        let _ = ctrl.handle_event(&key_ev(Key::Char('x')), 10);
        assert_eq!(ctrl.query(), "xab");
    }

    #[test]
    fn end_moves_cursor_to_end() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Home)), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::End)), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('z')), 10);
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

    // ── ClipboardPaste in Input mode ──────────────────────────────────

    #[test]
    fn clipboard_paste_in_input_mode_inserts_at_cursor() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        // Type "ac", move cursor left, then paste "b" — result should be "abc".
        let _ = ctrl.handle_event(&key_ev(Key::Char('a')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('c')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Left)), 10);
        let ev = ctrl.handle_event(&UiEvent::ClipboardPaste("b".into()), 10);
        assert_eq!(ctrl.query(), "abc");
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged {
                value: "abc".into()
            })
        );
    }

    #[test]
    fn clipboard_paste_appends_when_cursor_at_end() {
        let mut ctrl =
            DualModePaletteController::new("T", None, vec![]).with_mode(PaletteMode::Input);
        let _ = ctrl.handle_event(&key_ev(Key::Char('h')), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Char('i')), 10);
        let ev = ctrl.handle_event(&UiEvent::ClipboardPaste("!".into()), 10);
        assert_eq!(ctrl.query(), "hi!");
        assert_eq!(
            ev,
            Handled::Event(DualModePaletteAction::QueryChanged {
                value: "hi!".into()
            })
        );
    }

    #[test]
    fn clipboard_paste_ignored_in_list_mode() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![]);
        // List mode — paste should be ignored (not the focused text input path).
        let ev = ctrl.handle_event(&UiEvent::ClipboardPaste("xyz".into()), 10);
        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.query(), "");
    }

    #[test]
    fn set_items_resets_selection_and_scroll() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Down)), 10);
        let _ = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Down)), 10);
        assert_eq!(ctrl.selected(), 2);
        ctrl.set_items(vec![item("x")]);
        assert_eq!(ctrl.selected(), 0);
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
        let mut ctrl = DualModePaletteController::new("T", None, items);
        assert_eq!(ctrl.selected(), 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // title(row 0) + query(row 1) + separator(row 2) -> items start
        // at row 3; item 2 is at row 5.
        let ev = ctrl.handle_mouse_event(&mouse_down(5.0, 5.5), &backend, rect, 10);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(
            ctrl.selected(),
            2,
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
        let mut ctrl = DualModePaletteController::new("T", None, items);

        let backend = recording_backend();
        let rect = Rect::new(5.0, 3.0, 40.0, 20.0);
        // Item 2's row is local y=5, so absolute y = rect.y + 5 = 8.
        let ev = ctrl.handle_mouse_event(&mouse_down(10.0, 8.5), &backend, rect, 10);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(ctrl.selected(), 2);
    }

    #[test]
    fn mouse_click_outside_any_region_is_ignored() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.handle_mouse_event(&mouse_down(5.0, 99.0), &backend, rect, 10);

        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.selected(), 0);
    }

    #[test]
    fn mouse_moved_over_item_selects_it() {
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        assert_eq!(ctrl.selected(), 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // Same row geometry as `mouse_click_on_item_selects_it`: item 2
        // sits at row 5.
        let ev = ctrl.handle_mouse_move_event(&mouse_moved(5.0, 5.5), &backend, rect, 10);

        assert_eq!(ev, Handled::Consumed);
        assert_eq!(
            ctrl.selected(),
            2,
            "hovering item 2's row must update the selection, same as a click"
        );
    }

    #[test]
    fn mouse_moved_over_the_already_selected_row_is_ignored() {
        // A stationary pointer must not churn `Handled::Consumed` (and so
        // a redraw) every frame it's polled.
        let items = vec![item("a"), item("b"), item("c")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        assert_eq!(ctrl.selected(), 0);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        // Item 0's row is at local y=2 (title + query rows above it).
        let ev = ctrl.handle_mouse_move_event(&mouse_moved(5.0, 2.5), &backend, rect, 10);

        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.selected(), 0);
    }

    #[test]
    fn mouse_moved_outside_any_item_row_is_ignored() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.handle_mouse_move_event(&mouse_moved(5.0, 99.0), &backend, rect, 10);

        assert_eq!(ev, Handled::Ignored);
        assert_eq!(ctrl.selected(), 0);
    }

    #[test]
    fn non_mouse_moved_event_is_ignored_by_handle_mouse_move() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![item("a")]);
        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.handle_mouse_move_event(&mouse_down(5.0, 4.5), &backend, rect, 10);
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn non_mouse_event_is_ignored_by_handle_mouse() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![item("a")]);
        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.handle_mouse_event(&key_ev(Key::Char('a')), &backend, rect, 10);
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn right_click_is_ignored() {
        let mut ctrl = DualModePaletteController::new("T", None, vec![item("a")]);
        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let ev = ctrl.handle_mouse_event(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Right,
                position: crate::Point { x: 5.0, y: 2.5 },
                modifiers: Modifiers::default(),
            },
            &backend,
            rect,
            10,
        );
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    #[allow(deprecated)] // covers the `handle`/`handle_mouse` shims kept for pre-`Handled` callers.
    fn deprecated_methods_still_adapt_to_dual_mode_palette_event() {
        let items = vec![item("a"), item("b")];
        let mut ctrl = DualModePaletteController::new("T", None, items);
        assert_eq!(
            ctrl.handle(&key_ev(Key::Named(NamedKey::Down)), 10),
            DualModePaletteEvent::Consumed
        );
        assert_eq!(
            ctrl.handle(&key_ev(Key::Named(NamedKey::Enter)), 10),
            DualModePaletteEvent::ItemConfirmed { idx: 1 }
        );

        let backend = recording_backend();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        assert_eq!(
            ctrl.handle_mouse(&mouse_down(5.0, 99.0), &backend, rect, 10),
            DualModePaletteEvent::Ignored
        );
    }
}
