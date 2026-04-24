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

use crate::event::Rect;
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

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Sixth primitive on the new shape. Palette has three vertical regions:
// title (optional chrome), query input, then the items list. The title
// and query heights are caller-supplied; item positions come out of the
// measurer closure.

/// Per-item measurement for a palette result row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaletteItemMeasure {
    pub height: f32,
}

impl PaletteItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible palette item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisiblePaletteItem {
    /// Index into `Palette.items`.
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteHit {
    /// Click landed on the title chrome row (typically no-op).
    Title,
    /// Click landed on the query input row. Apps typically focus the
    /// query and may use the local x-offset (click_x - query_bounds.x)
    /// to place the caret; the primitive doesn't resolve that in v1.
    Query,
    /// Click landed on an item row.
    Item(usize),
    /// Click landed outside any region.
    Empty,
}

/// Fully-resolved palette layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Present iff title_height > 0.
    pub title_bounds: Option<Rect>,
    /// Present iff query_height > 0 (query input is optional but
    /// typically present).
    pub query_bounds: Option<Rect>,
    pub visible_items: Vec<VisiblePaletteItem>,
    pub hit_regions: Vec<(Rect, PaletteHit)>,
    /// Scroll offset actually used, clamped to `[0, items.len())`.
    pub resolved_scroll_offset: usize,
}

impl PaletteLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> PaletteHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        PaletteHit::Empty
    }
}

impl Palette {
    /// Compute the full rendering + hit-test layout.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — modal overlay area.
    /// - `title_height` — rows reserved for the title header. Pass 0.0
    ///   to omit.
    /// - `query_height` — rows reserved for the query input. Pass 0.0
    ///   to omit (unusual — palettes normally show the input).
    /// - `measure_item(i)` — height for item `i`.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        title_height: f32,
        query_height: f32,
        measure_item: F,
    ) -> PaletteLayout
    where
        F: Fn(usize) -> PaletteItemMeasure,
    {
        let mut visible_items: Vec<VisiblePaletteItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, PaletteHit)> = Vec::new();

        let mut y = 0.0_f32;

        // Title row (optional).
        let title_bounds = if title_height > 0.0 && y < viewport_height {
            let h = title_height.min(viewport_height - y);
            let bounds = Rect::new(0.0, y, viewport_width, h);
            hit_regions.push((bounds, PaletteHit::Title));
            y += h;
            Some(bounds)
        } else {
            None
        };

        // Query input row (optional but usually present).
        let query_bounds = if query_height > 0.0 && y < viewport_height {
            let h = query_height.min(viewport_height - y);
            let bounds = Rect::new(0.0, y, viewport_width, h);
            hit_regions.push((bounds, PaletteHit::Query));
            y += h;
            Some(bounds)
        } else {
            None
        };

        // Clamp scroll_offset.
        let resolved_scroll_offset = if self.items.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.items.len() - 1)
        };

        // Items list.
        for i in resolved_scroll_offset..self.items.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_item(i);
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            visible_items.push(VisiblePaletteItem {
                item_idx: i,
                bounds,
            });
            hit_regions.push((bounds, PaletteHit::Item(i)));
            y += m.height;
        }

        PaletteLayout {
            viewport_width,
            viewport_height,
            title_bounds,
            query_bounds,
            visible_items,
            hit_regions,
            resolved_scroll_offset,
        }
    }
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
    /// Scrollbar was clicked or dragged. `new_offset` is the row index
    /// the app should apply to `Palette::scroll_offset`. Derived by
    /// `quadraui::dispatch::dispatch_mouse_drag` from the drag state's
    /// track geometry; apps simply store the value.
    ScrollOffsetChanged { new_offset: usize },
    /// A key was pressed while the palette had focus and the primitive
    /// did not consume it. App may interpret it (e.g. `Ctrl+P` cycles
    /// a history ring).
    KeyPressed { key: String, modifiers: Modifiers },
}
