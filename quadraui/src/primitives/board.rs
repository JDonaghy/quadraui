//! `Board` primitive: a kanban/pipeline widget that renders columns of cards
//! with stage badges, supports keyboard + mouse navigation, and is **pure
//! render + input** — no data fetching, no business logic.
//!
//! Data-in (`BoardModel`), actions-out (`BoardAction`).
//!
//! ## Layout model
//!
//! ```text
//! ┌─ Board ────────────────────────────────────────────────────────┐
//! │  Backlog          Refining      Ready          Pipeline    Done │
//! │ ┌──────────────┐ ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌─┐│
//! │ │#362 Board    │ │#550 board  │ │#521 Issues │ │#207 GTK  │ │…││
//! │ │✓P ●W ·T ·R ·M│ │✓P ·W ·T ·R │ │✓P ✓W ✓T ●R │ │✓P✓W✓T✓R…│ │ ││
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
//! Part of the `Backend` trait: every backend implements `draw_board`.
//! Apps write `backend.draw_board(rect, &model)` once and all backends
//! rasterise correctly.

use crate::event::{Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

// ── Identifiers ───────────────────────────────────────────────────────────────

/// Identifier for a board card (alias of [`WidgetId`]).
pub type CardId = WidgetId;

// ── Badge model ───────────────────────────────────────────────────────────────

/// A pipeline stage tracked per card.
///
/// The canonical order (left-to-right in the badge row) is
/// `Plan → Work → Test → Review → Merge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stage {
    /// Planning / refinement.
    Plan,
    /// Active implementation.
    Work,
    /// Automated test run.
    Test,
    /// Human code review.
    Review,
    /// Merge to target branch.
    Merge,
}

impl Stage {
    /// Short display label used in the badge row (1 char).
    pub fn short_label(self) -> char {
        match self {
            Stage::Plan => 'P',
            Stage::Work => 'W',
            Stage::Test => 'T',
            Stage::Review => 'R',
            Stage::Merge => 'M',
        }
    }
}

/// Status of a single pipeline badge on a card.
///
/// Distinct from `PipelineView::StageStatus` — the board variant adds
/// `RequestChanges` and `Blocked` which are not meaningful in a CI
/// pipeline context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BadgeStatus {
    /// Not yet started — rendered dim (·).
    Pending,
    /// Currently in progress — rendered with accent colour (●).
    Running,
    /// Completed successfully — rendered green (✓).
    Passed,
    /// Reviewer has requested changes — rendered with warning tint (↩).
    RequestChanges,
    /// Blocked on an upstream condition — rendered error colour (✗).
    Blocked,
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
    /// Inline badge row — one entry per pipeline stage. Rendered as a
    /// compact icon row: `✓P ●W ·T ·R ·M`. Informational-only in v1
    /// (no per-stage hit target).
    #[serde(default)]
    pub stage_badges: Vec<(Stage, BadgeStatus)>,
    /// Optional assignee handle shown at the trailing edge of the card.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Machine / runner name displayed below the badge row.
    #[serde(default)]
    pub machine: Option<String>,
    /// One-line decision callout rendered at the bottom of the card
    /// (e.g. `"decision: use approach B"`). `None` = omitted.
    #[serde(default)]
    pub decision_hint: Option<String>,
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

/// A verdict on a test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestVerdict {
    Pass,
    Skip,
    Fail,
}

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
    /// Refinement action on a card (`r` key).
    Refine(CardId),
    /// Dispatch / pipeline action on a card (`P` key).
    Dispatch(CardId),
    /// Record a test verdict on a card (`F` key for fail, `S` for skip,
    /// `p`/`P` for pass depending on consumer keymap).
    RecordTest(CardId, TestVerdict),
    /// Start review on a card.
    StartReview(CardId),
    /// Open the review for a card.
    OpenReview(CardId),
    /// Merge action on a card.
    Merge(CardId),
    /// Drop a card back to the backlog.
    DropToBacklog(CardId),
}

// ── Keyboard handling ─────────────────────────────────────────────────────────

impl BoardModel {
    /// Handle a keyboard event on the focused board.
    ///
    /// Returns a [`BoardAction`] if the key is consumed, `None` if the
    /// caller should handle it.
    ///
    /// ### Keybindings (v1 hardcoded, coord-tui–compatible)
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
    /// | `r` | Refine selected card |
    /// | `d` / `P` | Dispatch selected card |
    /// | `m` | Merge selected card |
    /// | `b` | Drop selected card to backlog |
    /// Move the selection in the given direction, updating `selected_card_id`
    /// and `col_scroll_offset` as needed.
    ///
    /// Callers that want the `BoardAction` for external routing should use
    /// [`handle_key`] instead. This method is for apps (like `BoardApp`) that
    /// just want the state to update immediately.
    pub fn move_selection(&mut self, dir: MoveDir) {
        let (cur_col, cur_card) = self.selected_position();
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

    /// Return `(col_index, card_index)` of the currently-selected card.
    fn selected_position(&self) -> (Option<usize>, Option<usize>) {
        let Some(id) = self.selected_card_id.as_ref() else {
            return (None, None);
        };
        for (ci, col) in self.columns.iter().enumerate() {
            if let Some(ri) = col.cards.iter().position(|c| &c.id == id) {
                return (Some(ci), Some(ri));
            }
        }
        (None, None)
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
            "r" => selected.map(|id| BoardAction::Refine(id.clone())),
            "d" | "P" => selected.map(|id| BoardAction::Dispatch(id.clone())),
            "m" => selected.map(|id| BoardAction::Merge(id.clone())),
            "b" => selected.map(|id| BoardAction::DropToBacklog(id.clone())),
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
        for col in &self.columns {
            if rect_contains(col.header_bounds, x, y) {
                return BoardHit::ColumnHeader(col.col_id.clone());
            }
            for card in &col.cards {
                if rect_contains(card.bounds, x, y) {
                    return BoardHit::Card(card.id.clone());
                }
            }
        }
        BoardHit::Empty
    }
}

fn rect_contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
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
    let start = model.col_scroll_offset.min(n_total.saturating_sub(1));
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

        let scroll_off = col.scroll_offset.min(col.cards.len());
        let card_end = (scroll_off + visible_cards_per_col).min(col.cards.len());

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_card(id: &str, title: &str) -> BoardCard {
        BoardCard {
            id: WidgetId::new(id),
            title: title.to_string(),
            labels: vec![],
            stage_badges: vec![
                (Stage::Plan, BadgeStatus::Passed),
                (Stage::Work, BadgeStatus::Running),
                (Stage::Test, BadgeStatus::Pending),
                (Stage::Review, BadgeStatus::Pending),
                (Stage::Merge, BadgeStatus::Pending),
            ],
            assignee: None,
            machine: None,
            decision_hint: None,
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
        assert_eq!(m.stage_badges_count(), 15);
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
            BoardAction::RecordTest(WidgetId::new("card:2"), TestVerdict::Pass),
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
    fn stage_badges_count(&self) -> usize {
        self.columns
            .iter()
            .flat_map(|c| c.cards.iter())
            .map(|card| card.stage_badges.len())
            .sum()
    }
}
