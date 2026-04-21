//! `Palette` primitive: a modal overlay with a search input and a
//! filterable, selectable list of results. Used for command palettes,
//! quick-open file pickers, buffer switchers, and fuzzy finders in
//! general.
//!
//! A `Palette` is app-driven: the app filters its own source against
//! the current `query` each frame and produces the visible `items`
//! list. The primitive renders what it's given and emits events.
//!
//! Scope for the first primitive cut: flat lists. Preview panes
//! (right-side file preview) and tree structures (symbol picker with
//! expandable rows) are later primitive extensions; apps with those
//! needs fall back to their legacy rendering until the extensions land.
//!
//! # Backend contract
//!
//! **Declarative + modal.** Render as an overlay on top of the rest of
//! the UI (highest z-order); intercept ALL mouse and keyboard events
//! when open. Render the `query` text input at the top, then
//! `items[scroll_offset..]` below. Click on item → emit
//! `PaletteEvent::ItemActivated { idx }`. Printable keys append to
//! query → emit `QueryChanged`. `j`/`k`/arrows move `selected_idx`,
//! Enter activates, Escape emits `Cancelled`.
//!
//! **Click intercept is mandatory.** If the backend lets clicks fall
//! through to the editor / underlying UI when the palette is open,
//! users will accidentally interact with hidden widgets — a class of
//! bug we hit in vimcode's Win-GUI port (see
//! `docs/NATIVE_GUI_LESSONS.md` §10). For each click handler in your
//! backend, the very first check should be "is a palette / dialog
//! open? If yes, route here instead."

use crate::types::{Icon, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `Palette` widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub id: WidgetId,
    /// Header text shown above the query input, e.g. `"Commands"` or
    /// `"Open File"`. Optional "N/M" count is rendered by the backend
    /// when `total_count > items.len()`.
    pub title: String,
    /// Current search query text.
    pub query: String,
    /// Cursor byte offset in `query`. Backends paint a cursor block
    /// at the corresponding visible column.
    #[serde(default)]
    pub query_cursor: usize,
    /// Filtered, pre-scored visible items in display order.
    pub items: Vec<PaletteItem>,
    /// Index into `items` of the currently highlighted row.
    pub selected_idx: usize,
    /// How many rows have been scrolled past. App-owned for v1.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Total number of items in the underlying source (before filtering).
    /// Displayed as `N/M` in the header. `0` means "don't show count".
    #[serde(default)]
    pub total_count: usize,
    #[serde(default)]
    pub has_focus: bool,
}

/// One row in a `Palette`'s filtered result list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteItem {
    /// Primary row text (file name, command name, buffer label).
    pub text: StyledText,
    /// Optional right-aligned secondary text (line number, shortcut,
    /// file path suffix).
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// Optional left-aligned icon.
    #[serde(default)]
    pub icon: Option<Icon>,
    /// Character positions inside `text` that match the current query.
    /// Backends render these with a highlight (typically bold + accent
    /// colour). Indices are byte offsets into the concatenated span
    /// text. Empty means "no fuzzy-match highlighting".
    #[serde(default)]
    pub match_positions: Vec<usize>,
}

/// Events a `Palette` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaletteEvent {
    /// The query text changed (user typed, pasted, or deleted).
    QueryChanged { value: String },
    /// Keyboard / mouse moved selection to a different row.
    SelectionChanged { idx: usize },
    /// User confirmed the highlighted row (Enter or double-click).
    ItemConfirmed { idx: usize },
    /// Palette was dismissed (Escape, click outside, etc.).
    Closed,
    /// A key was pressed while the palette had focus and the primitive
    /// did not consume it. App may interpret it (e.g. `Ctrl+P` cycles
    /// a history ring).
    KeyPressed { key: String, modifiers: Modifiers },
}
