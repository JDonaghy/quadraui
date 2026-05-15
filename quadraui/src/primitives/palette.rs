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
use crate::primitives::scrollbar::fit_thumb;
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
    /// When `false`, the query input row and separator are hidden —
    /// producing a popup-style list (tab switcher, branch picker).
    /// Default `true`.
    #[serde(default = "default_true")]
    pub show_query: bool,
    /// When set, a pinned "create" action row is rendered below the
    /// scrollable item list (e.g. `"Create branch '{query}'"` in a
    /// branch picker). The string is the display label — apps
    /// typically interpolate the query themselves each frame.
    #[serde(default)]
    pub create_label: Option<String>,
    /// When present, the palette renders a split layout: item list on
    /// the left, preview content on the right.
    #[serde(default)]
    pub preview: Option<PalettePreview>,
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
    /// Indentation level for tree-structured items. `0` = top level.
    /// Backends render `depth * indent_width` leading space.
    #[serde(default)]
    pub depth: usize,
    /// Whether this item shows an expand/collapse arrow.
    #[serde(default)]
    pub expandable: bool,
    /// Arrow direction when `expandable` is true (`▾` vs `▸`).
    #[serde(default)]
    pub expanded: bool,
}

fn default_true() -> bool {
    true
}

/// Preview pane content shown alongside the item list when the palette
/// operates in split-layout mode (file pickers, symbol pickers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PalettePreview {
    /// Syntax-highlighted content lines.
    pub lines: Vec<StyledText>,
    /// Optional title shown above the preview content (e.g. file path).
    #[serde(default)]
    pub title: Option<String>,
    /// Scroll offset into `lines`.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Line to visually highlight (e.g. the matched line in a search).
    #[serde(default)]
    pub highlight_line: Option<usize>,
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
    /// Click landed on the query input row.
    Query,
    /// Click landed on an item row.
    Item(usize),
    /// Click landed on the expand/collapse arrow of a tree item.
    ExpandToggle(usize),
    /// Click landed on the pinned "create" action row.
    CreateAction,
    /// Click landed on the preview pane area.
    Preview,
    /// Click landed on the scrollbar thumb (drag handle).
    ScrollbarThumb,
    /// Click landed on the scrollbar track (page-jump area).
    ScrollbarTrack,
    /// Click landed outside any region.
    Empty,
}

/// Scrollbar geometry within a palette's item list area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaletteScrollbar {
    /// Full scrollbar track (background rail).
    pub track: Rect,
    /// Draggable thumb within the track.
    pub thumb: Rect,
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
    /// Width of the item list column. Equals `viewport_width` when no
    /// preview is present; narrower (~40%) when a preview pane is shown.
    pub item_list_width: f32,
    /// Pinned create-action row bounds, present when `Palette.create_label` is `Some`.
    pub create_bounds: Option<Rect>,
    /// Preview pane bounds, present when `Palette.preview` is `Some`.
    pub preview_bounds: Option<Rect>,
    /// Scrollbar track + thumb geometry, present when the item list
    /// overflows and `scrollbar_width > 0` was passed to `layout()`.
    pub scrollbar: Option<PaletteScrollbar>,
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
    /// - `scrollbar_width` — width reserved for the scrollbar when the
    ///   item list overflows. Pass 0.0 to skip scrollbar computation.
    /// - `min_thumb_len` — minimum scrollbar thumb length (1.0 cell for
    ///   TUI, 8.0 px for GTK). Ignored when `scrollbar_width` is 0.0.
    /// - `measure_item(i)` — height for item `i`.
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        title_height: f32,
        query_height: f32,
        scrollbar_width: f32,
        min_thumb_len: f32,
        measure_item: F,
    ) -> PaletteLayout
    where
        F: Fn(usize) -> PaletteItemMeasure,
    {
        let has_preview = self.preview.is_some();
        let item_list_width = if has_preview {
            (viewport_width * 0.4).round()
        } else {
            viewport_width
        };

        let mut visible_items: Vec<VisiblePaletteItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, PaletteHit)> = Vec::new();

        let mut y = 0.0_f32;

        let title_bounds = if title_height > 0.0 && y < viewport_height {
            let h = title_height.min(viewport_height - y);
            let bounds = Rect::new(0.0, y, viewport_width, h);
            hit_regions.push((bounds, PaletteHit::Title));
            y += h;
            Some(bounds)
        } else {
            None
        };

        let query_bounds = if query_height > 0.0 && y < viewport_height {
            let h = query_height.min(viewport_height - y);
            let bounds = Rect::new(0.0, y, viewport_width, h);
            hit_regions.push((bounds, PaletteHit::Query));
            y += h;
            Some(bounds)
        } else {
            None
        };

        let items_top = y;

        let create_row_h = if self.create_label.is_some() {
            measure_item(0).height
        } else {
            0.0
        };
        let items_bottom = viewport_height - create_row_h;

        let resolved_scroll_offset = if self.items.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.items.len() - 1)
        };

        for i in resolved_scroll_offset..self.items.len() {
            if y >= items_bottom {
                break;
            }
            let m = measure_item(i);
            let remaining = items_bottom - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, item_list_width, height);
            visible_items.push(VisiblePaletteItem {
                item_idx: i,
                bounds,
            });
            hit_regions.push((bounds, PaletteHit::Item(i)));
            y += m.height;
        }

        let total = self.items.len();
        let visible_count = visible_items.len();
        let has_scrollbar = scrollbar_width > 0.0 && total > visible_count;

        let scrollbar = if has_scrollbar {
            let track_h = items_bottom - items_top;
            let track = Rect::new(
                item_list_width - scrollbar_width,
                items_top,
                scrollbar_width,
                track_h,
            );
            let (thumb_start, thumb_len) = fit_thumb(
                resolved_scroll_offset as f32,
                total as f32,
                visible_count as f32,
                track_h,
                min_thumb_len,
            );
            let thumb = Rect::new(track.x, items_top + thumb_start, scrollbar_width, thumb_len);

            let content_width = item_list_width - scrollbar_width;
            for vi in &mut visible_items {
                vi.bounds.width = content_width;
            }
            for (rect, hit) in &mut hit_regions {
                if matches!(hit, PaletteHit::Item(_) | PaletteHit::ExpandToggle(_)) {
                    rect.width = content_width;
                }
            }

            hit_regions.push((thumb, PaletteHit::ScrollbarThumb));
            hit_regions.push((track, PaletteHit::ScrollbarTrack));

            Some(PaletteScrollbar { track, thumb })
        } else {
            None
        };

        let create_bounds = if self.create_label.is_some() && items_bottom < viewport_height {
            let bounds = Rect::new(0.0, items_bottom, item_list_width, create_row_h);
            hit_regions.push((bounds, PaletteHit::CreateAction));
            Some(bounds)
        } else {
            None
        };

        let preview_bounds = if has_preview {
            let preview_x = item_list_width;
            let preview_w = (viewport_width - item_list_width).max(0.0);
            let preview_h = (viewport_height - items_top).max(0.0);
            let bounds = Rect::new(preview_x, items_top, preview_w, preview_h);
            hit_regions.push((bounds, PaletteHit::Preview));
            Some(bounds)
        } else {
            None
        };

        PaletteLayout {
            viewport_width,
            viewport_height,
            title_bounds,
            query_bounds,
            visible_items,
            hit_regions,
            create_bounds,
            resolved_scroll_offset,
            item_list_width,
            preview_bounds,
            scrollbar,
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
    /// User toggled a tree item's expand/collapse state.
    ExpandToggled { idx: usize, expanded: bool },
    /// Palette was dismissed (Escape, click outside, etc.).
    Closed,
    /// A key was pressed while the palette had focus and the primitive
    /// did not consume it. App may interpret it (e.g. `Ctrl+P` cycles
    /// a history ring).
    KeyPressed { key: String, modifiers: Modifiers },
}
