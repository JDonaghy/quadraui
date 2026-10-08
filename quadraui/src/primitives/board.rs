//! `Board` primitive: a kanban/pipeline widget that renders columns of cards
//! with inline status badges, supports keyboard + mouse navigation, and is
//! **pure render + input** — no data fetching, no business logic. The host
//! defines its own badge vocabulary (labels, order, meaning); the primitive
//! only knows how to lay them out and colour them by [`BadgeStatus`].
//!
//! Data-in (`BoardModel`), actions-out (`BoardAction`).
//!
//! ## Layout model
//!
//! Column titles and badge vocabulary below are illustrative only — the
//! primitive has no opinion on either; a host picks whatever columns and
//! badge labels/order fit its own workflow.
//!
//! ```text
//! ┌─ Board ────────────────────────────────────────────────────────┐
//! │  Column A         Column B       Column C        Column D  ... │
//! │ ┌──────────────┐ ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌─┐│
//! │ │#123 Card one │ │#456 card   │ │#789 Item   │ │#012 Item │ │…││
//! │ │✓A ●B ·C ·D ·E│ │✓A ·B ·C ·D │ │✓A ✓B ✓C ●D │ │✓A✓B✓C✓D…│ │ ││
//! │ └──────────────┘ └────────────┘ └────────────┘ └──────────┘ └─┘│
//! └────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Event routing
//!
//! `Board` owns layout and hit-testing; backends own painting.
//! After each paint the backend returns a [`BoardLayout`] which the
//! host holds. On mouse events the host calls [`BoardLayout::hit_test`]
//! to translate `(x, y)` into a [`BoardHit`]. Keyboard events are
//! handled by the host via [`BoardModel::handle_key`].
//!
//! ## Portability
//!
//! Part of the `Backend` trait: `draw_board` is required (no default
//! impl — quadraui#600), so every backend implementer supplies a real
//! rasteriser (or an explicit `todo!()` stub, per convention). Apps
//! write `backend.draw_board(rect, &model)` once and pick up each
//! backend's rasteriser as it lands.

use crate::event::{Point, Rect};
use crate::theme::Theme;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

// ── Shared layout constants ─────────────────────────────────────────────────
//
// #736: `gtk::board` and `macos::board` each carried their own copy of these
// seven geometry constants (`GTK_BOARD_*_PX` / `MAC_BOARD_*_PX`), byte-for-
// byte identical in every value. Per #713's primitive-first rule, a third
// (`win::board`) or fourth copy is exactly the duplication that rule exists
// to stop, so all three pixel/DIP-unit backends (gtk, macos, win — a DIP is
// a pixel at 100% display scale, the same convention `win::board`'s module
// doc uses) now share one definition. `tui::board` keeps its own
// `TUI_BOARD_*_CELLS`/`TUI_BOARD_CARD_H` constants — cells are a genuinely
// different unit with different natural values, not a fourth copy of these.

/// Minimum column width in pixel/DIP-unit backends' native units.
pub const BOARD_COL_MIN_PX: f32 = 200.0;
/// Gap between adjacent columns.
pub const BOARD_COL_GAP_PX: f32 = 8.0;
/// Column header height (title row).
pub const BOARD_HEADER_H_PX: f32 = 24.0;
/// Card height (title + badge row + optional hint).
pub const BOARD_CARD_H_PX: f32 = 64.0;
/// Vertical gap between adjacent cards within a column.
pub const BOARD_CARD_GAP_PX: f32 = 6.0;
/// Corner radius for card boxes. `f64` — matches Cairo's/Core Graphics'
/// path-construction APIs, which both `gtk::board` and `macos::board` feed
/// this straight into. `win::board` has no rounded-rect primitive (see its
/// module doc) so it doesn't consume this constant.
pub const BOARD_CARD_CORNER_RADIUS_PX: f64 = 4.0;
/// Horizontal text padding inside a card. `f64` for the same Cairo/Core
/// Graphics reason as [`BOARD_CARD_CORNER_RADIUS_PX`].
pub const BOARD_CARD_H_PAD_PX: f64 = 8.0;

// ── Identifiers ───────────────────────────────────────────────────────────────

/// Identifier for a board card (alias of [`WidgetId`]).
pub type CardId = WidgetId;

// ── Badge model ───────────────────────────────────────────────────────────────

/// Status of a single inline badge on a card.
///
/// Distinct from `PipelineView::StageStatus` — the board variant adds
/// `Warning` and `Blocked`, which don't map cleanly onto a CI pipeline's
/// pass/fail/skip vocabulary but are common in review/approval workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BadgeStatus {
    /// Not yet started — rendered dim (·).
    Pending,
    /// Currently in progress — rendered with accent colour (●).
    Running,
    /// Completed successfully — rendered green (✓).
    Passed,
    /// Needs attention before it can proceed — rendered with warning tint (↩).
    Warning,
    /// Blocked on an upstream condition — rendered error colour (✗).
    Blocked,
}

/// The `status → icon` table for a badge — shared by every backend's
/// rasteriser (gtk, macos, tui, win). #736: previously duplicated
/// verbatim as a private `badge_icon` in `gtk::board`, `macos::board`, and
/// `tui::board`; #713's primitive-first rule forbids a fourth copy in
/// `win::board`, so this is the one definition all four call.
pub fn badge_icon(status: BadgeStatus) -> char {
    match status {
        BadgeStatus::Passed => '✓',
        BadgeStatus::Running => '●',
        BadgeStatus::Warning => '↩',
        BadgeStatus::Blocked => '✗',
        BadgeStatus::Pending => '·',
    }
}

/// The `status → colour` table for a badge icon — shared by every
/// backend's rasteriser (#736, same rationale as [`badge_icon`]).
pub fn badge_fg_color(status: BadgeStatus, theme: &Theme) -> Color {
    match status {
        BadgeStatus::Passed => theme.badge_passed,
        BadgeStatus::Running => theme.badge_running,
        BadgeStatus::Warning => theme.badge_warning,
        BadgeStatus::Blocked => theme.badge_blocked,
        BadgeStatus::Pending => theme.muted_fg,
    }
}

/// A single inline badge on a card (e.g. one step of a host-defined
/// workflow).
///
/// The board primitive has no notion of what a badge *means* — the host
/// supplies both the short display `label` (typically one character,
/// e.g. `"P"`) and the `status` driving its icon + colour. Order and
/// vocabulary are entirely host-defined; render as many or as few
/// badges per card as the host's workflow needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardBadge {
    /// Short display label rendered next to the status icon.
    pub label: String,
    /// Status driving the badge's icon + colour.
    pub status: BadgeStatus,
}

// ── Data model ────────────────────────────────────────────────────────────────

/// A single card displayed in a board column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCard {
    /// Unique identifier for this card (used for selection + actions).
    pub id: CardId,
    /// Display title (truncated by the rasteriser if too wide).
    pub title: String,
    /// Short prefix labels (e.g. `"#362"` or `"quadraui"`). The rasteriser
    /// renders them before the title, separated by a space.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Inline badge row — one entry per host-defined workflow step.
    /// Rendered as a compact icon row: `✓P ●W ·T`. Informational-only
    /// in v1 (no per-badge hit target).
    #[serde(default)]
    pub badges: Vec<CardBadge>,
    /// One-line callout rendered at the bottom of the card (host-defined
    /// content, e.g. a note or blocking reason). `None` = omitted.
    #[serde(default)]
    pub hint: Option<String>,
}

/// A single kanban column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardColumn {
    /// Unique identifier for this column (used in [`BoardHit::ColumnHeader`]).
    pub id: WidgetId,
    /// Column header title (e.g. `"Backlog"`, `"Pipeline"`).
    pub title: String,
    /// Cards in this column (full list; the rasteriser slices by
    /// `scroll_offset` and available height).
    pub cards: Vec<BoardCard>,
    /// Vertical scroll offset: index of the first card to show.
    /// Host-owned; updated in response to [`BoardAction::MoveSelection`].
    #[serde(default)]
    pub scroll_offset: usize,
}

/// Top-level model for a [`Board`] widget.
///
/// The host constructs this from its own state each frame and passes it
/// to `backend.draw_board(rect, &model)`.
///
/// # Examples
///
/// ```
/// use quadraui::{board_layout, BoardCard, BoardColumn, BoardHit, BoardMeasure, BoardModel, WidgetId};
///
/// let model = BoardModel {
///     id: WidgetId::new("board:issues"),
///     columns: vec![BoardColumn {
///         id: WidgetId::new("column:backlog"),
///         title: "Backlog".to_string(),
///         cards: vec![BoardCard {
///             id: WidgetId::new("card:1"),
///             title: "Fix flaky test".to_string(),
///             labels: vec![],
///             badges: vec![],
///             hint: None,
///         }],
///         scroll_offset: 0,
///     }],
///     selected_card_id: None,
///     col_scroll_offset: 0,
/// };
///
/// let measure = BoardMeasure::new(20.0, 1.0, 1.0, 3.0, 1.0);
/// let layout = board_layout(&model, 0.0, 0.0, 80.0, 24.0, measure);
///
/// assert_eq!(layout.hit_test(1.0, 2.0), BoardHit::Card(WidgetId::new("card:1")));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardModel {
    /// Unique identifier for this board widget.
    pub id: WidgetId,
    /// Ordered list of columns rendered left-to-right.
    pub columns: Vec<BoardColumn>,
    /// The currently selected card, if any (host-owned).
    #[serde(default)]
    pub selected_card_id: Option<CardId>,
    /// Horizontal scroll: index of the first visible column.
    /// Updated by the host in response to `h`/`l` navigation when
    /// the board overflows the viewport.
    #[serde(default)]
    pub col_scroll_offset: usize,
}

// ── Actions ───────────────────────────────────────────────────────────────────

/// Semantic direction for keyboard navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveDir {
    Up,
    Down,
    Left,
    Right,
}

/// Actions emitted by the `Board` widget back to the host.
///
/// The host decides the meaning — these are semantic intents, not
/// mutations. Example: `OpenIssue(id)` might open a detail panel; the
/// board does not navigate itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BoardAction {
    /// Selection cursor moved to this card (click or keyboard).
    SelectCard(CardId),
    /// Enter / double-click: open the card's issue detail.
    OpenIssue(CardId),
    /// Right-click: show context menu at pixel anchor.
    ContextMenu(CardId, Point),
    /// Keyboard navigation request — host updates `selected_card_id`,
    /// per-column `scroll_offset`, and `col_scroll_offset` accordingly.
    MoveSelection(MoveDir),
    /// `gg` — host should scroll the focused column to the top.
    JumpToTop,
    /// `G` — host should scroll the focused column to the bottom.
    JumpToBottom,
    /// Open the review for a card.
    OpenReview(CardId),
}

// ── Keyboard handling ─────────────────────────────────────────────────────────

impl BoardModel {
    /// Handle a keyboard event on the focused board.
    ///
    /// Returns a [`BoardAction`] if the key is consumed, `None` if the
    /// caller should handle it.
    ///
    /// ### Keybindings (v1 hardcoded, generic navigation only)
    ///
    /// | Key | Action |
    /// |-----|--------|
    /// | `j` / `↓` | Move selection down |
    /// | `k` / `↑` | Move selection up |
    /// | `h` / `←` | Move selection left (between columns) |
    /// | `l` / `→` | Move selection right (between columns) |
    /// | `Enter` | Open issue |
    /// | `g` | Jump to top of focused column |
    /// | `G` | Jump to bottom of focused column |
    ///
    /// Hosts that need workflow-specific verbs (e.g. merge, dispatch,
    /// review) should handle those keys themselves before or after calling
    /// this method — it only ever consumes the generic navigation keys
    /// above.
    /// Move the selection in the given direction, updating `selected_card_id`
    /// and `col_scroll_offset` as needed.
    ///
    /// Callers that want the `BoardAction` for external routing should use
    /// [`handle_key`] instead. This method is for apps (like `BoardApp`) that
    /// just want the state to update immediately.
    pub fn move_selection(&mut self, dir: MoveDir) {
        let (cur_col, cur_card) = match self.selected_position() {
            Some((ci, ri)) => (Some(ci), Some(ri)),
            None => (None, None),
        };
        match dir {
            MoveDir::Down => {
                if let Some(ci) = cur_col {
                    let next = cur_card.map(|c| c + 1).unwrap_or(0);
                    if next < self.columns[ci].cards.len() {
                        self.selected_card_id = Some(self.columns[ci].cards[next].id.clone());
                    }
                }
            }
            MoveDir::Up => {
                if let Some(ci) = cur_col {
                    if let Some(cc) = cur_card {
                        if cc > 0 {
                            self.selected_card_id = Some(self.columns[ci].cards[cc - 1].id.clone());
                        }
                    }
                }
            }
            MoveDir::Right => {
                if let Some(ci) = cur_col {
                    if ci + 1 < self.columns.len() {
                        let next_ci = ci + 1;
                        let next_card = cur_card
                            .map(|c| c.min(self.columns[next_ci].cards.len().saturating_sub(1)));
                        self.selected_card_id = next_card
                            .and_then(|c| self.columns[next_ci].cards.get(c))
                            .map(|card| card.id.clone());
                    }
                } else if !self.columns.is_empty() && !self.columns[0].cards.is_empty() {
                    self.selected_card_id = Some(self.columns[0].cards[0].id.clone());
                }
            }
            MoveDir::Left => {
                if let Some(ci) = cur_col {
                    if ci > 0 {
                        let prev_ci = ci - 1;
                        let next_card = cur_card
                            .map(|c| c.min(self.columns[prev_ci].cards.len().saturating_sub(1)));
                        self.selected_card_id = next_card
                            .and_then(|c| self.columns[prev_ci].cards.get(c))
                            .map(|card| card.id.clone());
                    }
                }
            }
        }
    }

    /// Select the first card in the currently-focused column.
    pub fn jump_to_top(&mut self) {
        if let Some(ci) = self.selected_col_index() {
            if let Some(card) = self.columns[ci].cards.first() {
                self.selected_card_id = Some(card.id.clone());
            }
        }
    }

    /// Select the last card in the currently-focused column.
    pub fn jump_to_bottom(&mut self) {
        if let Some(ci) = self.selected_col_index() {
            if let Some(card) = self.columns[ci].cards.last() {
                self.selected_card_id = Some(card.id.clone());
            }
        }
    }

    /// Return the column index of the currently-selected card, if any.
    pub fn selected_col_index(&self) -> Option<usize> {
        let id = self.selected_card_id.as_ref()?;
        self.columns
            .iter()
            .position(|col| col.cards.iter().any(|c| &c.id == id))
    }

    /// Return `(col_index, card_index)` of the currently-selected card, or
    /// `None` if no card is selected (or the selected id is no longer
    /// present on the board).
    pub fn selected_position(&self) -> Option<(usize, usize)> {
        let id = self.selected_card_id.as_ref()?;
        for (ci, col) in self.columns.iter().enumerate() {
            if let Some(ri) = col.cards.iter().position(|c| &c.id == id) {
                return Some((ci, ri));
            }
        }
        None
    }

    pub fn handle_key(&self, key: &str, _modifiers: Modifiers) -> Option<BoardAction> {
        let selected = self.selected_card_id.as_ref();
        match key {
            "j" | "Down" | "ArrowDown" => Some(BoardAction::MoveSelection(MoveDir::Down)),
            "k" | "Up" | "ArrowUp" => Some(BoardAction::MoveSelection(MoveDir::Up)),
            "h" | "Left" | "ArrowLeft" => Some(BoardAction::MoveSelection(MoveDir::Left)),
            "l" | "Right" | "ArrowRight" => Some(BoardAction::MoveSelection(MoveDir::Right)),
            "Enter" => selected.map(|id| BoardAction::OpenIssue(id.clone())),
            "g" => Some(BoardAction::JumpToTop),
            "G" => Some(BoardAction::JumpToBottom),
            _ => None,
        }
    }
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// Backend-native dimensions needed to compute a [`BoardLayout`].
///
/// Each backend supplies these from its own metrics (cells for TUI,
/// pixels for GTK). The layout algorithm is backend-agnostic given
/// these values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardMeasure {
    /// Minimum column width in surface-native units (clamped lower bound
    /// when equal-split produces a narrower column).
    pub col_min_width: f32,
    /// Horizontal gap between adjacent columns.
    pub col_gap: f32,
    /// Height of the column header strip (title row).
    pub header_height: f32,
    /// Height of one card box (title + badge row + optional hint).
    pub card_height: f32,
    /// Vertical gap between adjacent cards within a column.
    pub card_gap: f32,
}

impl BoardMeasure {
    /// Create a new [`BoardMeasure`].
    pub fn new(
        col_min_width: f32,
        col_gap: f32,
        header_height: f32,
        card_height: f32,
        card_gap: f32,
    ) -> Self {
        Self {
            col_min_width,
            col_gap,
            header_height,
            card_height,
            card_gap,
        }
    }
}

/// Resolved bounds for a single card in the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLayout {
    /// The card identifier (cloned from [`BoardCard::id`]).
    pub id: CardId,
    /// Column index in the full `BoardModel::columns` list.
    pub col_index: usize,
    /// Card index within the full column's `cards` list (absolute, not
    /// relative to `scroll_offset`).
    pub card_index: usize,
    /// Full card bounds in surface-native units.
    pub bounds: Rect,
}

/// Resolved layout for a single visible column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnLayout {
    /// Index into `BoardModel::columns`.
    pub col_index: usize,
    /// Column identifier (copied from [`BoardColumn::id`]).
    pub col_id: WidgetId,
    /// Full column bounds (header + body).
    pub bounds: Rect,
    /// Header strip bounds (column title text).
    pub header_bounds: Rect,
    /// Body bounds (card area below the header).
    pub body_bounds: Rect,
    /// How many cards fit vertically in the body at this card_height.
    pub visible_cards: usize,
    /// Resolved card positions (only the `scroll_offset..` visible slice).
    pub cards: Vec<CardLayout>,
}

/// Fully-resolved board layout returned by `draw_board`.
///
/// The host holds this between frames. On mouse events, call
/// [`Self::hit_test`] to route clicks. `visible_cards` drives
/// selection-follow clamping (DataTable pattern).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardLayout {
    /// Overall widget bounds.
    pub bounds: Rect,
    /// Visible columns (the slice of `BoardModel::columns` starting at
    /// `col_scroll_offset` that fits in the viewport).
    pub columns: Vec<ColumnLayout>,
}

/// Result of a [`BoardLayout::hit_test`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardHit {
    /// Click landed on a card body.
    Card(CardId),
    /// Click landed on a column header.
    ColumnHeader(WidgetId),
    /// Click missed all interactive regions.
    Empty,
}

impl BoardLayout {
    /// Hit-test a click at surface-native coordinates `(x, y)`.
    ///
    /// Returns [`BoardHit::Card`] when the click falls inside a card,
    /// [`BoardHit::ColumnHeader`] when it falls on a column header,
    /// or [`BoardHit::Empty`] otherwise.
    pub fn hit_test(&self, x: f32, y: f32) -> BoardHit {
        let point = Point::new(x, y);
        for col in &self.columns {
            if col.header_bounds.contains(point) {
                return BoardHit::ColumnHeader(col.col_id.clone());
            }
            for card in &col.cards {
                if card.bounds.contains(point) {
                    return BoardHit::Card(card.id.clone());
                }
            }
        }
        BoardHit::Empty
    }
}

/// Compute the layout for a [`BoardModel`].
///
/// Column widths are determined by equal-splitting the available width;
/// the result is clamped to `measure.col_min_width`. When the clamped
/// width causes all columns to overflow the viewport, only the columns
/// starting from `model.col_scroll_offset` that fit are shown.
///
/// Per-column vertical scroll is driven by `BoardColumn::scroll_offset`;
/// `visible_cards` in the returned [`ColumnLayout`] tells the host the
/// scroll ceiling.
pub fn board_layout(
    model: &BoardModel,
    origin_x: f32,
    origin_y: f32,
    total_width: f32,
    total_height: f32,
    measure: BoardMeasure,
) -> BoardLayout {
    let bounds = Rect::new(origin_x, origin_y, total_width, total_height);
    let n_total = model.columns.len();

    if n_total == 0 || total_width <= 0.0 || total_height <= 0.0 {
        return BoardLayout {
            bounds,
            columns: vec![],
        };
    }

    // Equal-split width, clamped to the backend-supplied minimum.
    let gap_total = measure.col_gap * (n_total as f32 - 1.0).max(0.0);
    let natural = (total_width - gap_total) / n_total as f32;
    let col_w = natural.max(measure.col_min_width).max(1.0);

    // Count how many columns fit in the viewport from col_scroll_offset.
    let start = crate::primitives::scrollbar::clamp_scroll_offset(model.col_scroll_offset, n_total);
    let n_fit = count_fitting_columns(n_total - start, col_w, measure.col_gap, total_width);
    let end = (start + n_fit).min(n_total);

    // Vertical card capacity per column.
    let body_h = (total_height - measure.header_height).max(0.0);
    let visible_cards_per_col = if measure.card_height > 0.0 {
        let step = measure.card_height + measure.card_gap;
        // At least 1 card always visible even if it doesn't fully fit.
        ((body_h / step).floor() as usize).max(1)
    } else {
        0
    };

    let mut columns = Vec::with_capacity(end - start);
    let mut cx = origin_x;

    for col_i in start..end {
        let col = &model.columns[col_i];

        let col_bounds = Rect::new(cx, origin_y, col_w, total_height);
        let header_bounds = Rect::new(cx, origin_y, col_w, measure.header_height);
        let body_bounds = Rect::new(cx, origin_y + measure.header_height, col_w, body_h);

        let (scroll_off, card_end) = crate::primitives::scrollbar::visible_window(
            col.scroll_offset,
            col.cards.len(),
            visible_cards_per_col,
        );

        let mut card_layouts = Vec::with_capacity(card_end.saturating_sub(scroll_off));
        let mut cy = origin_y + measure.header_height;
        for (ci, card) in col.cards[scroll_off..card_end].iter().enumerate() {
            let card_bounds = Rect::new(cx, cy, col_w, measure.card_height);
            card_layouts.push(CardLayout {
                id: card.id.clone(),
                col_index: col_i,
                card_index: scroll_off + ci,
                bounds: card_bounds,
            });
            cy += measure.card_height + measure.card_gap;
        }

        columns.push(ColumnLayout {
            col_index: col_i,
            col_id: col.id.clone(),
            bounds: col_bounds,
            header_bounds,
            body_bounds,
            visible_cards: visible_cards_per_col,
            cards: card_layouts,
        });

        cx += col_w + measure.col_gap;
    }

    BoardLayout { bounds, columns }
}

/// Returns how many columns (starting from the scroll offset) fit in
/// `total_width` given per-column `col_w` and inter-column `gap`.
fn count_fitting_columns(available: usize, col_w: f32, gap: f32, total_width: f32) -> usize {
    if available == 0 || col_w <= 0.0 {
        return 0;
    }
    let mut count = 0usize;
    let mut used = 0.0_f32;
    for i in 0..available {
        let needed = if i == 0 { col_w } else { gap + col_w };
        if used + needed > total_width + 0.5 {
            break;
        }
        used += needed;
        count += 1;
    }
    // Always show at least 1 column (even if it overflows).
    count.max(1)
}

// ── PaintSurface paint (shared gtk/macos/win implementation, issue #1085,
// PaintSurface Phase 4 8/8) ────────────────────────────────────────────
//
// Before this, `gtk::board::draw_board` (Cairo + Pango),
// `macos::board::draw_board` (Core Graphics + Core Text) and
// `win::board::draw_board` (Direct2D + DirectWrite) each independently
// painted the same column/card geometry (already unified by #736's
// `BoardLayout`/[`pixel_board_layout`]) with their own drawing API.
// `paint` below is the one shared implementation, written against
// [`crate::paint_surface::PaintSurface`] (#807, Phase 1) instead of any
// one backend's drawing API — same shape as `diff_view`'s #866 migration.
//
// ## Divergences found (re-verified, reported here rather than silently
// resolved — CLAUDE.md's "re-verify before you implement" + this issue's
// acceptance bar)
//
// 1. **Column header text overflow.** `gtk::board` painted the header
//    title with no clip at all — an overlong title could bleed into the
//    next column. `macos::board` wrapped its header draw in
//    `CGContextSaveGState`/`CGContextClipToRect(header_bounds)`.
//    `win::board` relied on `DWrite::draw_text`'s own
//    `D2D1_DRAW_TEXT_OPTIONS_CLIP`. `PaintSurface` has no wrap/ellipsize
//    verb (see `diff_view`'s #866 "Header-label overflow handling"
//    divergence for the identical tradeoff), so `paint` below hard-clips
//    every column header to its own bounds on every backend — the
//    macOS/Windows behaviour. An overlong header on GTK now hard-clips
//    instead of silently bleeding into the next column (a real bug fix).
// 2. **Card title wrapping.** `gtk::board` word-wrapped an overlong title
//    across multiple lines (`pango::Layout::set_width`, no explicit max
//    line count). `macos::board` and `win::board` both painted a single
//    line, clipped to the card box (macOS: `CGContextClipToRect`;
//    Windows: `D2D1_DRAW_TEXT_OPTIONS_CLIP`). Per the same "no wrap verb"
//    constraint as divergence 1, `paint` hard-clips the (single-line)
//    title to the card box on every backend — the macOS/Windows
//    behaviour. A long title on GTK now clips instead of wrapping to a
//    second line (`BOARD_CARD_H_PX` is a fixed 64px regardless of
//    wrapping, so a wrapped second line was never guaranteed to avoid
//    colliding with the badge row 26px below the card top anyway).
// 3. **Card border shape.** `gtk::board` and `macos::board` stroked a
//    *rounded*-rect border (`BOARD_CARD_CORNER_RADIUS_PX`); `win::board`
//    could only stroke a straight rectangle (`win::text` has no
//    `ID2D1RoundedRectangleGeometry`-backed rounded stroke — see that
//    module's former doc). `PaintSurface::surface_stroke_rect` is
//    axis-aligned only (there is no `surface_stroke_rounded_rect`), so
//    `paint` strokes every card border as a straight rectangle on every
//    backend — the Windows behaviour. Card fills stay flat rectangles
//    too (not [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`])
//    so the fill and its border never mismatch in shape. GTK/macOS cards
//    lose their rounded-pill corners; `BOARD_CARD_CORNER_RADIUS_PX` is no
//    longer consumed by any rasteriser (TUI has no notion of rounded
//    corners either, so this makes all four backends agree).
// 4. **Per-element font size.** Pre-port `gtk::board::draw_board` varied
//    Pango point size per text run (`TITLE_FONT_SIZE = 11.0` for the
//    column header and card title, `BADGE_FONT_SIZE = 9.0` /
//    `HINT_FONT_SIZE = 9.0` for the badge row and hint strip, each set
//    via a `set_pango_size` helper that mutated a fresh
//    `pango::FontDescription` per call). `macos::board::draw_board` and
//    `win::board::draw_board` never had that: both modules' own
//    pre-port doc comments already disclosed painting every element
//    with the backend's single ambient font/`CTFont`/`DWrite` text
//    format as a "divergence from the GTK twin (deliberate)", because
//    per-element sizing needs a format/attribute cache neither
//    rasteriser had. `PaintSurface` has no per-call font-size verb at
//    all (only `scale_x`/bold/italic/underline via
//    `surface_draw_text_run_styled` — confirmed against
//    `paint_surface.rs`), so `paint` below paints every run — header,
//    title, badges, hint — at the ambient surface font on every
//    backend: the macOS/Windows (2-of-3) behaviour, GTK now matching
//    rather than the reverse. GTK's default ambient font at the point
//    `draw_board` is called is `editor_font` (11pt Monospace by
//    default — see `GtkBackend::enter_frame_scope`), which happens to
//    equal the old `TITLE_FONT_SIZE`, so the header and title are
//    visually unchanged at defaults; only the badge/hint text grows
//    from 9pt to the ambient size. Because a *user* can set an
//    arbitrarily large `editor_font`/`ui_font` (accessibility, demo
//    projection, etc.), `paint` no longer assumes any text run fits
//    the fixed `BADGE_Y_OFF`/`HINT_Y_OFF`/`HINT_H` pixel geometry those
//    constants were tuned for at 9pt: the badge row and the hint strip
//    are each now wrapped in their own `surface_push_clip`/
//    `surface_pop_clip` bracket (nested inside the card's own bracket
//    from divergences 1/2), so oversized text is clipped to its own
//    band instead of bleeding into the title or past the bottom of the
//    hint's background strip. See
//    `badge_row_and_hint_strip_are_each_clipped_to_their_own_band`
//    below for the regression (observed RED before the extra clip
//    brackets were added — a tall badge/hint measurement painted past
//    its intended strip with only the outer card clip in place).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        badge_fg_color, badge_icon, BoardCard, BoardLayout, BoardModel, BOARD_CARD_H_PAD_PX,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::layout_metrics::pixel_board_layout;
    use crate::theme::Theme;

    /// Vertical offset of the card title baseline from the card top.
    const TITLE_Y_OFF: f32 = 6.0;
    /// Vertical offset of the badge row from the card top.
    const BADGE_Y_OFF: f32 = 26.0;
    /// Height of the badge-row clip band — see divergence 4 above. Tuned
    /// generously past the 9pt badge text these offsets were originally
    /// measured for, so the default 11pt ambient font (divergence 4)
    /// still fits with headroom; a much larger user-configured ambient
    /// font clips rather than bleeding into the title above it.
    const BADGE_ROW_H: f32 = 18.0;
    /// Vertical offset of the hint strip from the card bottom.
    const HINT_Y_OFF: f32 = 18.0;
    /// Height of the hint strip.
    const HINT_H: f32 = 14.0;
    /// Vertical offset of the column header text from the header top.
    const HEADER_Y_OFF: f32 = 4.0;
    /// Card border stroke width — see divergence 3 above for why this is
    /// always a straight rect, never a rounded one.
    const CARD_BORDER_WIDTH: f32 = 1.0;

    /// Paint a [`BoardModel`] into `rect` on `surface`, returning
    /// [`BoardLayout`] for host click dispatch — same contract as every
    /// deleted per-backend `draw_board`.
    pub(crate) fn paint(
        model: &BoardModel,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
    ) -> BoardLayout {
        let layout = pixel_board_layout(model, rect.x, rect.y, rect.width, rect.height);

        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        let h_pad = BOARD_CARD_H_PAD_PX as f32;

        for col_layout in &layout.columns {
            let col = &model.columns[col_layout.col_index];

            // ── Column header ────────────────────────────────────────────
            let hb = col_layout.header_bounds;
            surface.surface_fill_rect(hb, theme.board_col_header_bg);
            surface.surface_push_clip(hb);
            surface.surface_draw_text_run(
                Rect::new(
                    hb.x + h_pad,
                    hb.y + HEADER_Y_OFF,
                    (hb.width - h_pad).max(0.0),
                    (hb.height - HEADER_Y_OFF).max(0.0),
                ),
                &col.title,
                theme.header_fg,
            );
            surface.surface_pop_clip();

            // ── Cards ────────────────────────────────────────────────────
            for card_layout in &col_layout.cards {
                let card: &BoardCard = &col.cards[card_layout.card_index];
                let is_selected = model
                    .selected_card_id
                    .as_ref()
                    .map(|id| id == &card.id)
                    .unwrap_or(false);

                let cb = card_layout.bounds;
                if cb.width <= 0.0 || cb.height <= 0.0 {
                    continue;
                }

                // Card background.
                let card_bg = if is_selected {
                    theme.board_selected_card_bg
                } else {
                    theme.surface_bg
                };
                surface.surface_fill_rect(cb, card_bg);

                // Card border.
                let border_col = if is_selected {
                    theme.accent_bg
                } else {
                    theme.border_fg
                };
                surface.surface_stroke_rect(cb, border_col, CARD_BORDER_WIDTH);

                // Everything below is clipped to the card box — see
                // divergences 1/2 above for why every backend now clips
                // rather than wrapping or silently overflowing.
                surface.surface_push_clip(cb);

                // ── Title line ───────────────────────────────────────────
                let prefix = if card.labels.is_empty() {
                    String::new()
                } else {
                    format!("{} ", card.labels.join(" "))
                };
                let full_title = format!("{}{}", prefix, card.title);
                surface.surface_draw_text_run(
                    Rect::new(
                        cb.x + h_pad,
                        cb.y + TITLE_Y_OFF,
                        (cb.width - h_pad * 2.0).max(0.0),
                        (cb.height - TITLE_Y_OFF).max(0.0),
                    ),
                    &full_title,
                    theme.surface_fg,
                );

                // ── Badge row ────────────────────────────────────────────
                // Divergence 4: clipped to its own band (not just the
                // outer card bracket above) so an ambient font larger
                // than the 9pt these fixed offsets were tuned for clips
                // instead of bleeding up into the title.
                let badge_y = cb.y + BADGE_Y_OFF;
                if !card.badges.is_empty() {
                    let badge_band = Rect::new(
                        cb.x,
                        badge_y - 2.0,
                        cb.width,
                        BADGE_ROW_H.min((cb.height - (badge_y - cb.y) + 2.0).max(0.0)),
                    );
                    surface.surface_push_clip(badge_band);
                    let mut badge_x = cb.x + h_pad;
                    for badge in &card.badges {
                        let badge_str = format!("{}{} ", badge_icon(badge.status), badge.label);
                        let (bw, bh) = surface.surface_measure_text(&badge_str);
                        let color = badge_fg_color(badge.status, theme);
                        surface.surface_draw_text_run(
                            Rect::new(badge_x, badge_y, bw.max(1.0), bh.max(1.0)),
                            &badge_str,
                            color,
                        );
                        badge_x += bw;
                        if badge_x > cb.x + cb.width - h_pad {
                            break;
                        }
                    }
                    surface.surface_pop_clip();
                }

                // ── Hint ─────────────────────────────────────────────────
                // Divergence 4: the strip's own clip bracket keeps a
                // taller-than-9pt ambient-font hint from painting past
                // the bottom of its own background fill.
                if let Some(hint) = &card.hint {
                    let hint_y = cb.y + cb.height - HINT_Y_OFF;
                    if hint_y > badge_y + 10.0 {
                        // Background strip.
                        let strip =
                            Rect::new(cb.x + 2.0, hint_y - 2.0, (cb.width - 4.0).max(0.0), HINT_H);
                        surface.surface_fill_rect(strip, theme.card_hint_bg);
                        // Text, clipped to the strip it's painted over.
                        surface.surface_push_clip(strip);
                        surface.surface_draw_text_run(
                            Rect::new(
                                cb.x + h_pad,
                                hint_y,
                                (cb.width - h_pad * 2.0).max(0.0),
                                HINT_H,
                            ),
                            hint,
                            theme.card_hint_fg,
                        );
                        surface.surface_pop_clip();
                    }
                }

                surface.surface_pop_clip();
            }
        }

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::board::{BadgeStatus, BoardColumn, CardBadge};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::diff_view`'s own `RecordingSurface` (#810/#865/#866).
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(460.0, 300.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn card(id: &str, title: &str) -> BoardCard {
            BoardCard {
                id: WidgetId::new(id),
                title: title.into(),
                labels: vec!["#1".into()],
                badges: vec![CardBadge {
                    label: "P".into(),
                    status: BadgeStatus::Passed,
                }],
                hint: None,
            }
        }

        fn sample_model() -> BoardModel {
            BoardModel {
                id: WidgetId::new("board"),
                columns: vec![BoardColumn {
                    id: WidgetId::new("col:backlog"),
                    title: "Backlog".into(),
                    cards: vec![card("card:a", "First")],
                    scroll_offset: 0,
                }],
                selected_card_id: None,
                col_scroll_offset: 0,
            }
        }

        #[test]
        fn zero_size_rect_paints_nothing() {
            let model = sample_model();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
            );
            assert!(layout.columns.is_empty());
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        /// Regression for divergence 1: the column header text is clipped
        /// to its own bounds (push/pop bracketed) rather than painted
        /// with no clip at all.
        #[test]
        fn column_header_text_is_clipped() {
            let model = sample_model();
            let mut surface = RecordingSurface::default();
            paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            assert_eq!(surface.clip_pushes.len(), surface.clip_pops);
            // One header clip + one clip per painted card.
            assert!(surface.clip_pushes.len() >= 2);
            let header = &model.columns[0];
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == &header.title));
        }

        /// Regression for divergence 2/3: the card title/badges/hint all
        /// paint inside the card's own push/pop clip bracket, and the
        /// card border is a straight `surface_stroke_rect` call (never
        /// `surface_fill_rounded_rect`).
        #[test]
        fn card_paints_a_straight_border_inside_one_clip_bracket() {
            let model = sample_model();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            let cb = layout.columns[0].cards[0].bounds;
            assert!(
                surface.strokes.iter().any(|(r, _)| *r == cb),
                "card border must be a straight surface_stroke_rect over the card bounds",
            );
            // 1 header clip + 1 card clip + 1 badge-row clip (divergence
            // 4 below — `sample_model`'s card has one badge, no hint).
            assert_eq!(surface.clip_pushes.len(), 3);
            assert_eq!(surface.clip_pops, 3);
        }

        /// Regression for divergence 4: an ambient font taller than the
        /// 9pt `BADGE_Y_OFF`/`HINT_Y_OFF`/`HINT_H` geometry was tuned for
        /// must not paint badge/hint text past its own band — each gets
        /// its own clip bracket nested inside the card's. Observed RED
        /// before those brackets were added: `paint` pushed only the
        /// outer card clip, so a tall measured badge/hint run had
        /// nothing bounding it to its own strip.
        #[test]
        fn badge_row_and_hint_strip_are_each_clipped_to_their_own_band() {
            /// Reports a much taller glyph box than `RecordingSurface`'s
            /// default 14px — simulates a large user-configured ambient
            /// font (divergence 4's whole point: `paint` has no way to
            /// shrink badge/hint text back down to 9pt).
            struct TallTextSurface(RecordingSurface);

            impl PaintSurface for TallTextSurface {
                fn surface_begin_frame(&mut self, v: Viewport) {
                    self.0.surface_begin_frame(v)
                }
                fn surface_end_frame(&mut self) {
                    self.0.surface_end_frame()
                }
                fn surface_viewport(&self) -> Viewport {
                    self.0.surface_viewport()
                }
                fn surface_line_height(&self) -> f32 {
                    self.0.surface_line_height()
                }
                fn surface_char_width(&self) -> f32 {
                    self.0.surface_char_width()
                }
                fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                    let (w, _h) = self.0.surface_measure_text(text);
                    (w, 40.0)
                }
                fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                    self.0.surface_fill_rect(rect, color)
                }
                fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                    self.0.surface_fill_rounded_rect(rect, radius, color)
                }
                fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                    self.0.surface_stroke_rect(rect, color, stroke_width)
                }
                fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                    self.0.surface_draw_text_run(rect, text, color)
                }
                fn surface_draw_line(
                    &mut self,
                    from: crate::Point,
                    to: crate::Point,
                    color: Color,
                    stroke_width: f32,
                ) {
                    self.0.surface_draw_line(from, to, color, stroke_width)
                }
                fn surface_push_clip(&mut self, rect: Rect) {
                    self.0.surface_push_clip(rect)
                }
                fn surface_pop_clip(&mut self) {
                    self.0.surface_pop_clip()
                }
                fn surface_draw_image(&mut self, rect: Rect, image: &Image) -> ImagePaintResult {
                    self.0.surface_draw_image(rect, image)
                }
            }

            let mut model = sample_model();
            model.columns[0].cards[0].hint = Some("blocked".into());
            let mut surface = TallTextSurface(RecordingSurface::default());
            let layout = paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            let cb = layout.columns[0].cards[0].bounds;
            let badge_y = cb.y + BADGE_Y_OFF;
            let hint_y = cb.y + cb.height - HINT_Y_OFF;

            // Card clip + badge-row clip + hint-strip clip, nested
            // inside the (already-verified) header + card brackets.
            assert_eq!(surface.0.clip_pushes.len(), 4);
            assert_eq!(surface.0.clip_pops, 4);

            let badge_clip = surface
                .0
                .clip_pushes
                .iter()
                .find(|r| (r.y - (badge_y - 2.0)).abs() < 0.01)
                .expect("badge row must push its own clip band");
            assert!(
                badge_clip.height <= BADGE_ROW_H,
                "badge-row clip band must stay within BADGE_ROW_H regardless of measured text height"
            );

            let hint_clip = surface
                .0
                .clip_pushes
                .iter()
                .find(|r| (r.y - (hint_y - 2.0)).abs() < 0.01)
                .expect("hint strip must push its own clip over its background fill");
            assert!(
                (hint_clip.height - HINT_H).abs() < 0.01,
                "hint clip must match the fixed HINT_H strip, not the taller measured text"
            );
        }

        #[test]
        fn selected_card_uses_the_selection_background() {
            let mut model = sample_model();
            model.selected_card_id = Some(WidgetId::new("card:a"));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &model,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            let cb = layout.columns[0].cards[0].bounds;
            assert!(surface
                .fills
                .iter()
                .any(|(r, c)| *r == cb && *c == theme.board_selected_card_bg));
        }

        /// Badge glyph + label text is drawn via `surface_draw_text_run`,
        /// not a bespoke per-backend loop.
        #[test]
        fn badge_text_is_recorded() {
            let model = sample_model();
            let mut surface = RecordingSurface::default();
            paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            assert!(surface
                .text_runs
                .iter()
                .any(|(_, t, _)| t.starts_with('✓') && t.contains('P')));
        }

        /// A hint is only painted when there's room below the badge row —
        /// mirrors every pre-#1085 backend's `if hint_y > badge_y + 10.0`
        /// guard.
        #[test]
        fn hint_paints_only_when_present() {
            let mut model = sample_model();
            model.columns[0].cards[0].hint = Some("blocked".into());
            let mut surface = RecordingSurface::default();
            paint(
                &model,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 220.0, 300.0),
            );
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "blocked"));
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_card(id: &str, title: &str) -> BoardCard {
        BoardCard {
            id: WidgetId::new(id),
            title: title.to_string(),
            labels: vec![],
            badges: vec![
                CardBadge {
                    label: "P".into(),
                    status: BadgeStatus::Passed,
                },
                CardBadge {
                    label: "W".into(),
                    status: BadgeStatus::Running,
                },
                CardBadge {
                    label: "T".into(),
                    status: BadgeStatus::Pending,
                },
                CardBadge {
                    label: "R".into(),
                    status: BadgeStatus::Pending,
                },
                CardBadge {
                    label: "M".into(),
                    status: BadgeStatus::Pending,
                },
            ],
            hint: None,
        }
    }

    fn make_model() -> BoardModel {
        BoardModel {
            id: WidgetId::new("board"),
            columns: vec![
                BoardColumn {
                    id: WidgetId::new("col:backlog"),
                    title: "Backlog".to_string(),
                    cards: vec![
                        make_card("card:362", "#362 Board"),
                        make_card("card:300", "#300 Fixes"),
                    ],
                    scroll_offset: 0,
                },
                BoardColumn {
                    id: WidgetId::new("col:ready"),
                    title: "Ready".to_string(),
                    cards: vec![make_card("card:521", "#521 Issues")],
                    scroll_offset: 0,
                },
                BoardColumn {
                    id: WidgetId::new("col:done"),
                    title: "Done".to_string(),
                    cards: vec![],
                    scroll_offset: 0,
                },
            ],
            selected_card_id: Some(WidgetId::new("card:362")),
            col_scroll_offset: 0,
        }
    }

    fn measure() -> BoardMeasure {
        BoardMeasure::new(20.0, 2.0, 2.0, 5.0, 1.0)
    }

    // ── Construction ─────────────────────────────────────────────────

    #[test]
    fn construction_and_field_access() {
        let m = make_model();
        assert_eq!(m.columns.len(), 3);
        assert_eq!(m.columns[0].title, "Backlog");
        assert_eq!(m.columns[0].cards.len(), 2);
        assert_eq!(m.columns[0].cards[0].id.as_str(), "card:362");
        // 3 cards × 5 badges each = 15
        assert_eq!(m.badges_count(), 15);
    }

    /// Badge vocabulary is entirely host-defined — the primitive has no
    /// fixed notion of a "stage" or pipeline order (#476: the framework
    /// previously hardcoded one consumer's `Plan/Work/Test/Review/Merge`
    /// workflow via a `Stage` enum). Any label string and any order is
    /// valid; this test uses a vocabulary unrelated to that workflow to
    /// prove there's no hidden coupling.
    #[test]
    fn badge_vocabulary_is_host_defined() {
        let card = BoardCard {
            id: WidgetId::new("card:1"),
            title: "Bake bread".to_string(),
            labels: vec![],
            badges: vec![
                CardBadge {
                    label: "Mix".to_string(),
                    status: BadgeStatus::Passed,
                },
                CardBadge {
                    label: "Proof".to_string(),
                    status: BadgeStatus::Running,
                },
                CardBadge {
                    label: "Bake".to_string(),
                    status: BadgeStatus::Blocked,
                },
            ],
            hint: Some("oven is out of order".to_string()),
        };
        assert_eq!(card.badges.len(), 3);
        assert_eq!(card.badges[0].label, "Mix");
        assert_eq!(card.badges[2].status, BadgeStatus::Blocked);
        assert_eq!(card.hint.as_deref(), Some("oven is out of order"));

        // Round-trips through serde like any other card.
        let json = serde_json::to_string(&card).unwrap();
        let back: BoardCard = serde_json::from_str(&json).unwrap();
        assert_eq!(card, back);
    }

    // ── Layout ───────────────────────────────────────────────────────

    #[test]
    fn layout_three_columns_fit_in_wide_viewport() {
        let m = make_model();
        // 3 cols × min 20 + 2 gaps = 64; viewport = 100 → all fit
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        assert_eq!(
            layout.columns.len(),
            3,
            "all 3 columns should fit in 100-unit viewport"
        );
    }

    #[test]
    fn layout_equal_column_widths() {
        let m = make_model();
        // 3 cols, gap 2 → natural = (100 - 4) / 3 ≈ 32
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        let w0 = layout.columns[0].bounds.width;
        let w1 = layout.columns[1].bounds.width;
        let w2 = layout.columns[2].bounds.width;
        assert!(
            (w0 - w1).abs() < 0.5 && (w1 - w2).abs() < 0.5,
            "columns should have equal width: {w0}, {w1}, {w2}"
        );
    }

    #[test]
    fn layout_horizontal_scroll_hides_first_column() {
        let mut m = make_model();
        m.col_scroll_offset = 1; // skip the first column
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        // Column 0 ("Backlog") must be absent; column 1 ("Ready") first.
        assert_eq!(layout.columns[0].col_id.as_str(), "col:ready");
    }

    #[test]
    fn layout_min_width_clamp_limits_visible_columns() {
        let m = make_model();
        // min_width = 50, viewport = 60 → only 1 column fits (50 + 2 > 60 for 2)
        let tight = BoardMeasure::new(50.0, 2.0, 2.0, 5.0, 1.0);
        let layout = board_layout(&m, 0.0, 0.0, 60.0, 20.0, tight);
        assert_eq!(
            layout.columns.len(),
            1,
            "only 1 column should fit with min_width 50 in viewport 60"
        );
    }

    #[test]
    fn layout_always_shows_at_least_one_column() {
        let m = make_model();
        // Extremely narrow viewport
        let layout = board_layout(&m, 0.0, 0.0, 5.0, 20.0, measure());
        assert!(
            layout.columns.len() >= 1,
            "at least 1 column must be visible regardless of viewport"
        );
    }

    #[test]
    fn layout_origin_offset() {
        let m = make_model();
        let layout = board_layout(&m, 10.0, 5.0, 100.0, 20.0, measure());
        assert_eq!(layout.bounds.x, 10.0);
        assert_eq!(layout.bounds.y, 5.0);
        assert_eq!(layout.columns[0].bounds.x, 10.0);
        assert_eq!(layout.columns[0].bounds.y, 5.0);
    }

    #[test]
    fn layout_visible_cards_per_column() {
        let m = make_model();
        // card_height=5, card_gap=1 → step=6; body_h = 20-2 = 18 → floor(18/6)=3
        // col[0] has 2 cards, so only 2 card layouts (not 3)
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        // visible_cards is the capacity (how many fit), not how many are present
        assert_eq!(layout.columns[0].visible_cards, 3);
        // But only 2 cards in the column data → 2 card layouts
        assert_eq!(layout.columns[0].cards.len(), 2);
        // col[2] is empty → 0 card layouts, but visible_cards = 3
        assert_eq!(layout.columns[2].visible_cards, 3);
        assert_eq!(layout.columns[2].cards.len(), 0);
    }

    #[test]
    fn layout_scroll_offset_slices_cards() {
        let mut m = make_model();
        m.columns[0].scroll_offset = 1; // skip card:362
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        let col0 = &layout.columns[0];
        // Only 1 card visible (card:300)
        assert_eq!(col0.cards.len(), 1);
        assert_eq!(col0.cards[0].id.as_str(), "card:300");
        assert_eq!(col0.cards[0].card_index, 1); // absolute index
    }

    #[test]
    fn layout_empty_model_is_safe() {
        let m = BoardModel {
            id: WidgetId::new("board"),
            columns: vec![],
            selected_card_id: None,
            col_scroll_offset: 0,
        };
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 30.0, measure());
        assert_eq!(layout.columns.len(), 0);
    }

    // ── Hit-testing ──────────────────────────────────────────────────

    #[test]
    fn hit_test_card_body() {
        let m = make_model();
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        let card0 = &layout.columns[0].cards[0];
        let cx = card0.bounds.x + card0.bounds.width / 2.0;
        let cy = card0.bounds.y + card0.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            BoardHit::Card(id) => assert_eq!(id.as_str(), "card:362"),
            other => panic!("expected Card hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_column_header() {
        let m = make_model();
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        let hdr = layout.columns[1].header_bounds;
        match layout.hit_test(hdr.x + 1.0, hdr.y + 0.5) {
            BoardHit::ColumnHeader(id) => assert_eq!(id.as_str(), "col:ready"),
            other => panic!("expected ColumnHeader hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_empty_area_returns_empty() {
        let m = make_model();
        let layout = board_layout(&m, 0.0, 0.0, 100.0, 20.0, measure());
        assert_eq!(layout.hit_test(500.0, 500.0), BoardHit::Empty);
    }

    // ── Selection ────────────────────────────────────────────────────

    #[test]
    fn selected_position_returns_col_and_card_index() {
        let m = make_model();
        // "card:362" is columns[0].cards[0]
        assert_eq!(m.selected_position(), Some((0, 0)));
    }

    #[test]
    fn selected_position_second_card_in_column() {
        let mut m = make_model();
        m.selected_card_id = Some(WidgetId::new("card:300"));
        assert_eq!(m.selected_position(), Some((0, 1)));
    }

    #[test]
    fn selected_position_other_column() {
        let mut m = make_model();
        m.selected_card_id = Some(WidgetId::new("card:521"));
        assert_eq!(m.selected_position(), Some((1, 0)));
    }

    #[test]
    fn selected_position_none_when_nothing_selected() {
        let mut m = make_model();
        m.selected_card_id = None;
        assert_eq!(m.selected_position(), None);
    }

    #[test]
    fn selected_position_none_when_id_not_found() {
        let mut m = make_model();
        m.selected_card_id = Some(WidgetId::new("card:missing"));
        assert_eq!(m.selected_position(), None);
    }

    // ── Keyboard handling ────────────────────────────────────────────

    #[test]
    fn handle_key_j_returns_move_down() {
        let m = make_model();
        assert_eq!(
            m.handle_key("j", Modifiers::default()),
            Some(BoardAction::MoveSelection(MoveDir::Down))
        );
        assert_eq!(
            m.handle_key("Down", Modifiers::default()),
            Some(BoardAction::MoveSelection(MoveDir::Down))
        );
    }

    #[test]
    fn handle_key_h_l_move_horizontal() {
        let m = make_model();
        assert_eq!(
            m.handle_key("h", Modifiers::default()),
            Some(BoardAction::MoveSelection(MoveDir::Left))
        );
        assert_eq!(
            m.handle_key("l", Modifiers::default()),
            Some(BoardAction::MoveSelection(MoveDir::Right))
        );
    }

    #[test]
    fn handle_key_enter_opens_selected() {
        let m = make_model();
        match m.handle_key("Enter", Modifiers::default()) {
            Some(BoardAction::OpenIssue(id)) => assert_eq!(id.as_str(), "card:362"),
            other => panic!("expected OpenIssue, got {other:?}"),
        }
    }

    #[test]
    fn handle_key_enter_noop_when_nothing_selected() {
        let mut m = make_model();
        m.selected_card_id = None;
        assert_eq!(m.handle_key("Enter", Modifiers::default()), None);
    }

    #[test]
    fn handle_key_unknown_returns_none() {
        let m = make_model();
        assert_eq!(m.handle_key("Escape", Modifiers::default()), None);
    }

    /// #476: the v1 hardcoded keymap used to bind `r`/`d`/`P`/`m`/`b` to
    /// coord-tui's workflow verbs (refine/dispatch/merge/drop-to-backlog).
    /// Those verbs are gone from `BoardAction`, so `handle_key` must leave
    /// these keys unconsumed — hosts that want them are free to bind their
    /// own meaning without fighting the primitive's default keymap.
    #[test]
    fn handle_key_no_longer_consumes_former_workflow_verb_keys() {
        let m = make_model();
        for key in ["r", "d", "P", "m", "b"] {
            assert_eq!(
                m.handle_key(key, Modifiers::default()),
                None,
                "key {key:?} should not be consumed by the generic board keymap"
            );
        }
    }

    // ── Serde ────────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let m = make_model();
        let json = serde_json::to_string(&m).unwrap();
        let back: BoardModel = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn action_serde_roundtrip() {
        let actions = vec![
            BoardAction::SelectCard(WidgetId::new("card:1")),
            BoardAction::MoveSelection(MoveDir::Down),
            BoardAction::OpenReview(WidgetId::new("card:2")),
            BoardAction::ContextMenu(WidgetId::new("card:3"), Point { x: 10.0, y: 20.0 }),
        ];
        for a in &actions {
            let json = serde_json::to_string(a).unwrap();
            let back: BoardAction = serde_json::from_str(&json).unwrap();
            assert_eq!(a, &back);
        }
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

impl BoardModel {
    /// Total badge count across all cards (useful for tests / assertions).
    #[cfg(test)]
    fn badges_count(&self) -> usize {
        self.columns
            .iter()
            .flat_map(|c| c.cards.iter())
            .map(|card| card.badges.len())
            .sum()
    }
}
