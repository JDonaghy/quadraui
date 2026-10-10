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

/// Which interaction mode the palette is operating in.
///
/// - `List` (default) — the existing search-and-select behaviour: the
///   query row filters the item list and Enter activates the selected row.
/// - `Input` — the query row is a standalone free-text field (e.g. "new
///   branch name"). The item list is hidden; Enter emits
///   [`PaletteEvent::InputConfirmed`] with the current query value.
///
/// Backends that have not yet implemented mode-aware rendering can safely
/// ignore this field — they will render the list as usual, which is a
/// graceful degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PaletteMode {
    /// Search + list mode — the default. The query filters the item list.
    #[default]
    List,
    /// Free-text input mode. The item list is suppressed; Enter confirms
    /// the raw query text.
    Input,
}

/// Declarative description of a `Palette` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{Palette, PaletteHit, PaletteItem, PaletteItemMeasure, StyledText, WidgetId};
///
/// let palette = Palette {
///     id: WidgetId::new("palette:commands"),
///     title: "Commands".to_string(),
///     query: "sav".to_string(),
///     query_cursor: 3,
///     items: vec![PaletteItem {
///         text: StyledText::plain("Save File"),
///         detail: None,
///         icon: None,
///         match_positions: vec![0, 1, 2],
///         depth: 0,
///         expandable: false,
///         expanded: false,
///     }],
///     selected_idx: 0,
///     scroll_offset: 0,
///     total_count: 1,
///     has_focus: true,
///     show_query: true,
///     create_label: None,
///     preview: None,
///     mode: Default::default(),
/// };
///
/// let layout = palette.layout(60.0, 20.0, 1.0, 1.0, 0.0, 1.0, |_| {
///     PaletteItemMeasure::new(1.0)
/// });
/// assert_eq!(layout.hit_test(0.0, 2.0), PaletteHit::Item(0));
/// ```
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
    /// Interaction mode — `List` (default) or `Input`.
    ///
    /// In `Input` mode the item list is suppressed and Enter emits
    /// [`PaletteEvent::InputConfirmed`] rather than activating a row.
    #[serde(default)]
    pub mode: PaletteMode,
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

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) — one result row is one text row
    /// (quadraui#817).
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height)
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
    /// Resolve `scroll_offset` against a viewport that can show
    /// `visible_rows` items at once, keeping `selected_idx` inside the
    /// visible window: scrolls forward when the selection runs past the
    /// bottom of the window, backward when it runs past the top, and
    /// never leaves empty rows at the bottom when enough items exist
    /// above to fill the viewport.
    ///
    /// This is selection-visibility *policy*, not rendering — pure
    /// arithmetic over `scroll_offset` / `selected_idx` / `items.len()`
    /// with no native dependency. It used to be copy-pasted verbatim
    /// into `gtk`, `tui`, and `win`'s palette rasterisers (each computing
    /// its own `visible_rows` from backend-native metrics and calling
    /// this same four-branch clamp); `macos` had no copy at all, so
    /// moving the selection past the bottom of the window scrolled it
    /// out of view instead of following it. See #711. [`Palette::layout`]
    /// now calls this internally so every backend gets it for free;
    /// backends that don't (yet) route through `layout` — like `tui`,
    /// see its `draw_palette` doc — can call it directly with their own
    /// `visible_rows`.
    pub fn resolved_scroll_offset(&self, visible_rows: usize) -> usize {
        let total = self.items.len();
        let max_offset = total.saturating_sub(visible_rows);
        let effective = if visible_rows == 0 {
            0
        } else if self.selected_idx < self.scroll_offset {
            self.selected_idx
        } else if self.selected_idx >= self.scroll_offset + visible_rows {
            self.selected_idx + 1 - visible_rows
        } else {
            self.scroll_offset
        };
        effective.min(max_offset)
    }

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

        // Estimate how many item rows fit in the available space so the
        // selection-visibility guard below has a row count to work
        // with. Every current caller measures a uniform row height, so
        // item 0 is representative; a future variable-height caller
        // would need a real fit-as-many-as-possible pass here instead.
        let item_height = if self.items.is_empty() {
            0.0
        } else {
            measure_item(0).height
        };
        let estimated_visible_rows = if item_height > 0.0 {
            ((items_bottom - items_top).max(0.0) / item_height) as usize
        } else {
            0
        };
        let resolved_scroll_offset = self.resolved_scroll_offset(estimated_visible_rows);

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

// ── PaintSurface paint (#1076, PaintSurface Phase 4 slice 3/8) ───────────
//
// Before this, `gtk::draw_palette` (Cairo), `macos::palette::draw_palette`
// (Core Graphics) and `win::palette::draw_palette` (Direct2D) each
// independently painted the same title/query/item/scrollbar/preview
// content with their own drawing API, and — worse — each independently
// re-derived the *geometry* those verbs used, which had drifted three
// ways (issue #1076):
//
// - **`query_height`**: GTK reserved `line_height + 1.0` (baking the
//   query/list separator stroke into the layout call, per D-007's
//   "Palette: deferred, not missed" §2 fix); macOS/Windows both passed
//   plain `line_height`, so their `PaletteLayout::query_bounds` was 1px
//   short of where the separator (and hence the first item row) should
//   start — the exact class of bug D-007 fixed for GTK alone.
// - **Row flooring**: GTK computed `visible_rows` by flooring available
//   height / `line_height` and fed `Palette::layout` a *reduced*
//   `viewport_height` so the item list never shows a partial last row;
//   macOS/Windows fed `Palette::layout` the full popup height, so
//   `Palette::layout`'s own per-row clamp (`height.min(remaining)`)
//   could hand back a clipped, partial-height last row.
// - **Scrollbar width**: GTK/Windows agreed on `(6.0, 8.0)`
//   (`scrollbar_width`, `min_thumb_len`); macOS alone used `(8.0, 8.0)`.
// - **Title row reservation**: GTK/macOS both always reserved
//   `line_height` for the title row regardless of whether `Palette.title`
//   was empty; `win::palette::win_palette_layout` special-cased
//   `title_h = 0.0` when `palette.title.is_empty()`, dropping the title
//   row (and shifting everything below it up by one row) for empty-title
//   popups. `layout` below always reserves `line_height` for the title —
//   GTK/macOS's 2-vs-1 majority — so Windows's empty-title popups now
//   carry an always-blank title row rather than omitting it. Low
//   real-world impact (`Palette.title` is essentially always populated
//   in practice), but named here for the same "don't silently pick one"
//   reason as the three geometry drifts above.
//
// `layout` below is the one shared geometry function (GTK's own
// pre-#1076 formula, since it already matched two of the three fixes
// above); `gtk_palette_layout`/`mac_palette_layout`/`win_palette_layout`
// become thin unit-converting wrappers over it, so no per-backend
// geometry copy is left to drift again.
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::tree::native_surface_paint` for the same
// pattern applied one primitive earlier. Feature gaps found while
// unifying (adopted the richer/majority behaviour rather than silently
// picking one, per that migration's convention):
//
// - **Match-position highlighting** (`PaletteItem::match_positions`):
//   GTK had it (per-character Pango `AttrColor` spans — one shaped text
//   run for the whole label, with per-glyph colour attributes layered on
//   top); Windows had it (its own `matched_runs`/`draw_matched_text`
//   run-splitter, ported below, which instead paints each
//   highlighted/non-highlighted run as a *separate*
//   `surface_draw_text_run` call); macOS had none at all (its module
//   doc's "Scope omissions" — items rendered in plain fg). `paint`
//   carries Windows's run-splitter for every backend now, which means
//   GTK's item labels move from one Pango-shaped run to N adjacent runs
//   at match/non-match boundaries — a subtle text-shaping change (glyph
//   advance/kerning at those boundaries can differ fractionally from
//   whole-string shaping) rather than a pure colour change. Likely
//   visually negligible for the short, mostly-ASCII labels palettes
//   render, but not covered by a shaping-sensitive test in either
//   direction — flagging it here rather than silently.

// - **Icon rendering** (`PaletteItem::icon`): GTK painted it (with the
//   Nerd-Font-fallback swap #416 documents); macOS/Windows never did.
//   `paint` paints it for every backend via
//   [`crate::paint_surface::PaintSurface::surface_draw_icon_glyph`],
//   which already carries the fallback swap per-backend where needed.
// - **Query cursor**: GTK/macOS both painted a filled cursor block
//   (inverting the character underneath, terminal-style); Windows drew
//   no cursor at all. `paint` carries the block-cursor treatment for
//   every backend.
// - **`PaletteMode::Input` suppression**: GTK and Windows both hide the
//   item list in `Input` mode; macOS did not (its module doc did not
//   mention `PaletteMode` at all — items rendered regardless of mode).
//   `paint` suppresses the item list for every backend.
// - **Preview line colour**: GTK painted each preview line span in its
//   own `span.fg` (falling back to `fg`); macOS/Windows always painted
//   preview lines in a single flat colour, ignoring `StyledText::spans`'
//   per-span colour entirely. `paint` carries GTK's per-span treatment.
// - **Scrollbar colour**: GTK and Windows both painted the track as
//   `theme.surface_bg * 0.7` and the thumb in flat `theme.border_fg`
//   (both fully opaque); macOS alone used
//   `theme.scrollbar_track.with_alpha(0.4)` /
//   `theme.scrollbar_thumb.with_alpha(0.8)`. `paint` adopts macOS's
//   scheme — the minority, not majority, pick — because it is the only
//   one of the three that uses the theme's dedicated
//   `scrollbar_track`/`scrollbar_thumb` fields at all; GTK/Windows's
//   `surface_bg`/`border_fg` reuse looks like an accident of not having
//   those fields available at the time they were written, not a
//   deliberate design choice worth preserving. Net visible effect on
//   GTK/Windows: the thumb goes from `border_fg` (blue-ish,
//   `rgb(120,160,200)` in the default theme) to `scrollbar_thumb`
//   (neutral grey, `rgb(110,115,130)`) at 0.8 alpha. Not pinned by a
//   test either direction (no consumer currently themes the scrollbar
//   distinctly from its default), but called out here per this
//   migration's own "pick one, document why" rule.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Palette, PaletteItemMeasure, PaletteLayout, PaletteMode};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::text_util::safe_prefix;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;
    use std::collections::HashSet;

    /// Scrollbar track width, shared by [`layout`] and [`paint`] so the
    /// two can't disagree on it — the `(6.0, 8.0)` pair GTK and Windows
    /// already agreed on pre-#1076 (see this module's doc for why
    /// macOS's pre-#1076 `(8.0, 8.0)` lost the tie).
    pub(crate) const SB_W: f32 = 6.0;
    /// Minimum scrollbar thumb length, shared by [`layout`] and [`paint`].
    pub(crate) const MIN_THUMB_LEN: f32 = 8.0;
    /// Breathing room reserved below the item list before the popup's
    /// own bottom edge (there is no bottom border row the way TUI has
    /// one — this is purely visual padding).
    const BOTTOM_INSET: f32 = 4.0;

    /// Compute the shared [`Palette`] layout — the one geometry `paint`
    /// paints from and every backend's `Backend::palette_layout` (#818)
    /// exposes for hit-testing, so paint/hit-test/backend can't drift
    /// three ways again (see this module's doc for the drift #1076
    /// found and fixed here).
    ///
    /// Returns the layout alongside `rows_h` (the item area's full row
    /// capacity in `surface`-native units, already floored to a whole
    /// number of rows) — `paint` needs it for the scrollbar-track /
    /// preview-pane / create-row positions that sit below the last
    /// item, which aren't otherwise exposed as a single [`PaletteLayout`]
    /// field.
    ///
    /// Coordinate frame: **LOCAL** — `(0, 0)` is the popup's own
    /// top-left corner, matching [`Palette::layout`]'s native contract.
    pub(crate) fn layout(
        w: f32,
        h: f32,
        palette: &Palette,
        line_height: f32,
    ) -> (PaletteLayout, f32) {
        let title_h = line_height;
        let query_h = if palette.show_query {
            line_height + 1.0
        } else {
            0.0
        };
        let has_create = palette.create_label.is_some();
        let create_reserved = if has_create { line_height } else { 0.0 };
        let items_top = title_h + query_h;
        let raw_items_h = (h - items_top - BOTTOM_INSET - create_reserved).max(0.0);
        let visible_rows = (raw_items_h / line_height) as usize;
        let rows_h = visible_rows as f32 * line_height;
        let viewport_h = items_top + rows_h + create_reserved;

        let resolved = palette.layout(w, viewport_h, title_h, query_h, SB_W, MIN_THUMB_LEN, |_| {
            PaletteItemMeasure::new(line_height)
        });
        (resolved, rows_h)
    }

    /// Split `text` into contiguous `(run, highlighted)` chunks based on
    /// `match_positions` (byte offsets, one per highlighted character).
    /// Ported from `win::palette`'s pre-#1076 `matched_runs` — the
    /// DirectWrite-shaped answer to GTK's per-character Pango
    /// `AttrColor` spans, and the only one of the three pre-#1076
    /// implementations that had match highlighting at all outside GTK.
    fn matched_runs(text: &str, match_positions: &[usize]) -> Vec<(String, bool)> {
        if match_positions.is_empty() {
            return vec![(text.to_string(), false)];
        }
        let matches: HashSet<usize> = match_positions.iter().copied().collect();
        let mut runs: Vec<(String, bool)> = Vec::new();
        let mut cur = String::new();
        let mut cur_hi = false;
        let mut first = true;
        for (byte_idx, ch) in text.char_indices() {
            let hi = matches.contains(&byte_idx);
            if first {
                cur_hi = hi;
                first = false;
            } else if hi != cur_hi {
                runs.push((std::mem::take(&mut cur), cur_hi));
                cur_hi = hi;
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            runs.push((cur, cur_hi));
        }
        runs
    }

    /// Paint `text` starting at `row.x, row.y` (single line, `row.height`
    /// tall), colouring highlighted runs (per [`matched_runs`]) in
    /// `match_fg` and the rest in `fg`. Returns the total painted width.
    fn draw_matched_text(
        surface: &mut dyn PaintSurface,
        text: &str,
        match_positions: &[usize],
        row: Rect,
        fg: crate::Color,
        match_fg: crate::Color,
    ) -> f32 {
        let mut cursor_x = row.x;
        for (run, hi) in matched_runs(text, match_positions) {
            if run.is_empty() {
                continue;
            }
            let (w, h) = surface.surface_measure_text(&run);
            let color = if hi { match_fg } else { fg };
            surface.surface_draw_text_run(
                Rect::new(cursor_x, row.y + (row.height - h) / 2.0, w.max(1.0), h),
                &run,
                color,
            );
            cursor_x += w;
        }
        cursor_x - row.x
    }

    /// Paint a [`Palette`] modal — background, border, title, query
    /// input (with cursor), item list (selection/icon/match-highlight/
    /// detail), scrollbar, pinned create row, and preview pane — onto
    /// `surface` at `area`.
    ///
    /// `palette_layout`/`rows_h` must be [`layout`]'s own return value
    /// for this exact `(area.width, area.height, palette, line_height)`
    /// so paint and hit-test can never disagree. `nerd_fonts_enabled`
    /// selects `Icon::glyph` vs. `Icon::fallback`. `style` supplies the
    /// corner radius/border width/shadow elevation the
    /// popup box paints with — callers pass `&self.style()`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        palette: &Palette,
        area: Rect,
        palette_layout: &PaletteLayout,
        rows_h: f32,
        line_height: f32,
        nerd_fonts_enabled: bool,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        if area.width < 20.0 || area.height < line_height * 4.0 {
            return;
        }

        // The shadow paints outside `area`, so it must land
        // before `surface_push_clip` below — a clip is still in effect
        // once this returns, and would otherwise cut the shadow off at
        // the popup's own edge.
        surface.surface_draw_shadow(
            area,
            style.corner_radius,
            style.shadow_elevation,
            Color::rgba(0, 0, 0, 100),
        );
        surface.surface_push_clip(area);
        surface.surface_fill_rounded_rect(area, style.corner_radius, theme.surface_bg);
        surface.surface_stroke_rounded_rect(
            area,
            style.corner_radius,
            theme.border_fg,
            style.border_width,
        );

        // ── Title row ───────────────────────────────────────────────
        if let Some(tb) = palette_layout.title_bounds {
            let title_text = if palette.total_count > 0 {
                format!(
                    " {}  {}/{} ",
                    palette.title,
                    palette.items.len(),
                    palette.total_count
                )
            } else {
                format!(" {} ", palette.title)
            };
            let (_, th) = surface.surface_measure_text(&title_text);
            surface.surface_draw_text_run(
                Rect::new(
                    area.x + tb.x + 8.0,
                    area.y + tb.y + (tb.height - th) / 2.0,
                    tb.width - 8.0,
                    th,
                ),
                &title_text,
                theme.title_fg,
            );
        }

        // ── Query row + cursor ──────────────────────────────────────
        if let Some(qb) = palette_layout.query_bounds {
            let qx = area.x + qb.x;
            let qy = area.y + qb.y;
            let prompt = "> ";
            let (pw, ph) = surface.surface_measure_text(prompt);
            surface.surface_draw_text_run(
                Rect::new(qx + 8.0, qy + (qb.height - ph) / 2.0, pw, ph),
                prompt,
                theme.query_fg,
            );

            let query_text_x = qx + 8.0 + pw;
            let (_, qh) = surface.surface_measure_text(&palette.query);
            surface.surface_draw_text_run(
                Rect::new(query_text_x, qy + (qb.height - qh) / 2.0, qb.width, qh),
                &palette.query,
                theme.query_fg,
            );

            let cursor_prefix = safe_prefix(&palette.query, palette.query_cursor);
            let (cursor_prefix_w, _) = surface.surface_measure_text(cursor_prefix);
            let cursor_x = query_text_x + cursor_prefix_w;
            let cursor_char: String = palette
                .query
                .get(palette.query_cursor..)
                .and_then(|s| s.chars().next())
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".to_string());
            let (cursor_w, _) = surface.surface_measure_text(&cursor_char);
            let cursor_w = cursor_w.max(line_height * 0.45);
            surface.surface_fill_rect(Rect::new(cursor_x, qy, cursor_w, qb.height), theme.query_fg);
            if !cursor_char.trim().is_empty() {
                let (_, ch) = surface.surface_measure_text(&cursor_char);
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, qy + (qb.height - ch) / 2.0, cursor_w, ch),
                    &cursor_char,
                    theme.surface_bg,
                );
            }
        }

        // ── Separator row ────────────────────────────────────────────
        // No separator in Input mode — there is no item list below it.
        //
        // The separator lives in the **last pixel of `query_bounds`**, not
        // at `query_bounds.y + query_bounds.height`. That extra pixel is
        // exactly what [`layout`]'s `query_h = line_height + 1.0` reserves
        // it (GTK's pre-#1076 formula, D-007 §2): the query *text* occupies
        // `line_height`, the stroke occupies the `+ 1.0`, and
        // `query_bounds.y + query_bounds.height` is therefore already the
        // first **item** row. Painting at `+ height` instead stole that
        // row's top pixel and — because item rows are painted after the
        // separator — was silently overwritten by the first row's
        // selection fill whenever row 0 was the selected one, so the
        // separator vanished entirely (quadraui#1076 CI: Windows
        // `separator_paints_at_corrected_row_not_drifted_one`).
        if palette.show_query && palette.mode != PaletteMode::Input {
            if let Some(qb) = palette_layout.query_bounds {
                let sep_y = area.y + qb.y + (qb.height - 1.0).max(0.0);
                surface
                    .surface_fill_rect(Rect::new(area.x, sep_y, area.width, 1.0), theme.border_fg);
            }
        }

        // ── Result rows ──────────────────────────────────────────────
        // Input mode suppresses the item list entirely — the query
        // field is the only interaction target.
        if palette.mode == PaletteMode::Input {
            surface.surface_pop_clip();
            return;
        }

        let items_top = palette_layout
            .query_bounds
            .map(|b| b.y + b.height)
            .or_else(|| palette_layout.title_bounds.map(|b| b.y + b.height))
            .unwrap_or(0.0);
        let rows_y = area.y + items_top;
        let content_w = palette_layout.item_list_width
            - if palette_layout.scrollbar.is_some() {
                SB_W
            } else {
                0.0
            };

        surface.surface_push_clip(Rect::new(area.x, rows_y, content_w, rows_h));
        for vis in &palette_layout.visible_items {
            let item = &palette.items[vis.item_idx];
            let row_x = area.x + vis.bounds.x;
            let row_y = area.y + vis.bounds.y;
            let row_w = vis.bounds.width;
            let row_h = vis.bounds.height;
            let is_selected = vis.item_idx == palette.selected_idx && palette.has_focus;

            if is_selected {
                surface.surface_fill_rect(Rect::new(row_x, row_y, row_w, row_h), theme.selected_bg);
            }

            let mut cursor_x = row_x + 8.0;

            let prefix = if is_selected { "\u{25b6} " } else { "  " };
            let (pw, ph) = surface.surface_measure_text(prefix);
            surface.surface_draw_text_run(
                Rect::new(cursor_x, row_y + (row_h - ph) / 2.0, pw, ph),
                prefix,
                theme.surface_fg,
            );
            cursor_x += pw;

            if let Some(ref icon) = item.icon {
                let glyph = if nerd_fonts_enabled {
                    icon.glyph.as_str()
                } else {
                    icon.fallback.as_str()
                };
                let (iw, ih) = surface.surface_measure_text(glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(cursor_x, row_y + (row_h - ih) / 2.0, iw, ih),
                    glyph,
                    theme.surface_fg,
                );
                cursor_x += iw + 6.0;
            }

            let detail_info = item.detail.as_ref().map(|detail| {
                let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
                let (dw, _) = surface.surface_measure_text(&detail_text);
                (detail_text, dw)
            });
            let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
            let text_right_limit = row_x + row_w - detail_reserve - 4.0;

            let full_text: String = item.text.spans.iter().map(|s| s.text.as_str()).collect();
            draw_matched_text(
                surface,
                &full_text,
                &item.match_positions,
                Rect::new(
                    cursor_x,
                    row_y,
                    (text_right_limit - cursor_x).max(0.0),
                    row_h,
                ),
                theme.surface_fg,
                theme.match_fg,
            );

            if let Some((detail_text, dw)) = detail_info {
                let dx = row_x + row_w - dw - 8.0;
                if dx > cursor_x {
                    let (_, dh) = surface.surface_measure_text(&detail_text);
                    surface.surface_draw_text_run(
                        Rect::new(dx, row_y + (row_h - dh) / 2.0, dw, dh),
                        &detail_text,
                        theme.muted_fg,
                    );
                }
            }
        }
        surface.surface_pop_clip();

        // ── Scrollbar ─────────────────────────────────────────────────
        if let Some(sb) = &palette_layout.scrollbar {
            let track = Rect::new(
                area.x + sb.track.x,
                area.y + sb.track.y,
                sb.track.width,
                sb.track.height,
            );
            surface.surface_fill_rect(track, theme.scrollbar_track.with_alpha(0.4));
            let thumb = Rect::new(
                area.x + sb.thumb.x + 1.0,
                area.y + sb.thumb.y,
                (sb.thumb.width - 2.0).max(0.0),
                sb.thumb.height,
            );
            surface.surface_fill_rect(thumb, theme.scrollbar_thumb.with_alpha(0.8));
        }

        // ── Create action row (pinned below items) ─────────────────────
        if let Some(ref label) = palette.create_label {
            let create_y = rows_y + rows_h;
            surface.surface_fill_rect(
                Rect::new(
                    area.x,
                    create_y,
                    palette_layout.item_list_width,
                    line_height,
                ),
                theme.hover_bg,
            );
            let prefix = "+ ";
            let (pw, ph) = surface.surface_measure_text(prefix);
            surface.surface_draw_text_run(
                Rect::new(area.x + 8.0, create_y + (line_height - ph) / 2.0, pw, ph),
                prefix,
                theme.accent_fg,
            );
            let (lw, lh) = surface.surface_measure_text(label);
            surface.surface_draw_text_run(
                Rect::new(
                    area.x + 8.0 + pw,
                    create_y + (line_height - lh) / 2.0,
                    lw,
                    lh,
                ),
                label,
                theme.accent_fg,
            );
        }

        // ── Preview pane ────────────────────────────────────────────────
        if let (Some(pb), Some(preview)) = (palette_layout.preview_bounds, palette.preview.as_ref())
        {
            let preview_x = area.x + pb.x;
            let preview_y = area.y + pb.y;
            let preview_w = pb.width;
            let preview_h = pb.height;

            surface.surface_draw_line(
                crate::Point::new(preview_x, preview_y),
                crate::Point::new(preview_x, preview_y + preview_h),
                theme.border_fg,
                1.0,
            );

            surface.surface_push_clip(Rect::new(preview_x, preview_y, preview_w, preview_h));
            let content_x = preview_x + 8.0;
            let content_right = preview_x + preview_w - 8.0;
            let mut cursor_y = preview_y;

            if let Some(ref title) = preview.title {
                let (_, th) = surface.surface_measure_text(title);
                surface.surface_draw_text_run(
                    Rect::new(
                        content_x,
                        cursor_y + (line_height - th) / 2.0,
                        content_right - content_x,
                        th,
                    ),
                    title,
                    theme.muted_fg,
                );
                cursor_y += line_height;
            }

            let preview_visible =
                ((preview_y + preview_h - cursor_y) / line_height).max(0.0) as usize;
            for (vi, line_idx) in (preview.scroll_offset..).take(preview_visible).enumerate() {
                let row_y = cursor_y + vi as f32 * line_height;
                if row_y + line_height > preview_y + preview_h + 0.5 {
                    break;
                }
                if line_idx >= preview.lines.len() {
                    break;
                }

                if preview.highlight_line == Some(line_idx) {
                    surface.surface_fill_rect(
                        Rect::new(preview_x, row_y, preview_w, line_height),
                        theme.selected_bg,
                    );
                }

                let line = &preview.lines[line_idx];
                let mut cx = content_x;
                for span in &line.spans {
                    if cx > content_right {
                        break;
                    }
                    let span_fg = span.fg.unwrap_or(theme.foreground);
                    let (sw, sh) = surface.surface_measure_text(&span.text);
                    surface.surface_draw_text_run(
                        Rect::new(cx, row_y + (line_height - sh) / 2.0, sw, sh),
                        &span.text,
                        span_fg,
                    );
                    cx += sw;
                }
            }
            surface.surface_pop_clip();
        }

        surface.surface_pop_clip();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::primitives::palette::PaletteItem;
        use crate::types::{Color, StyledText, WidgetId};
        use crate::Image;

        /// Records every fill + text-run call — mirrors
        /// `primitives::tree::native_surface_paint`'s `RecordingSurface`
        /// test double, scoped to the verbs this primitive uses, so these
        /// tests run on any host without Cairo/Core Graphics/Direct2D.
        ///
        /// Fills are recorded **in paint order**, which is what lets
        /// [`separator_sits_in_the_reserved_query_pixel_not_the_item_row`]
        /// below assert the separator isn't merely *emitted* at the right
        /// place but also isn't overpainted by a later row fill.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            texts: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 120.0, 1.0)
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
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            /// Deliberately does **not** forward to
            /// `PaintSurface`'s default (which would recurse into
            /// `surface_fill_rounded_rect` three times and pollute
            /// `fills` with shadow layers ahead of the real background
            /// fill every index-based `fills[..]` assertion here relies
            /// on) — a no-op recorder, same posture as `surface_draw_line`
            /// below.
            fn surface_draw_shadow(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _elevation: u8,
                _color: Color,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        const LINE_HEIGHT: f32 = 16.0;
        const AREA: Rect = Rect::new(0.0, 0.0, 200.0, 120.0);

        fn item(label: &str) -> PaletteItem {
            PaletteItem {
                text: StyledText::plain(label.to_string()),
                detail: None,
                icon: None,
                match_positions: vec![],
                depth: 0,
                expandable: false,
                expanded: false,
            }
        }

        fn sample() -> Palette {
            Palette {
                id: WidgetId::new("pal"),
                title: "Commands".into(),
                query: "op".into(),
                query_cursor: 2,
                items: vec![item("open file"), item("close"), item("quit")],
                selected_idx: 0,
                scroll_offset: 0,
                total_count: 0,
                has_focus: true,
                show_query: true,
                create_label: None,
                preview: None,
                mode: PaletteMode::List,
            }
        }

        fn paint_sample(palette: &Palette) -> (RecordingSurface, PaletteLayout) {
            let (palette_layout, rows_h) = layout(AREA.width, AREA.height, palette, LINE_HEIGHT);
            let mut surface = RecordingSurface::default();
            paint(
                palette,
                AREA,
                &palette_layout,
                rows_h,
                LINE_HEIGHT,
                false,
                &mut surface,
                &Theme::default(),
                &Style::default(),
            );
            (surface, palette_layout)
        }

        /// The query band reserves `line_height + 1.0` (GTK's pre-#1076
        /// formula, adopted for every backend by #1076) — `line_height`
        /// for the text, `+ 1.0` for the query/list separator stroke — so
        /// `query_bounds.y + query_bounds.height` is already the first
        /// *item* row, not the separator's row.
        #[test]
        fn query_band_reserves_one_extra_pixel_for_the_separator() {
            let p = sample();
            let (palette_layout, _) = layout(AREA.width, AREA.height, &p, LINE_HEIGHT);
            let qb = palette_layout.query_bounds.expect("query bounds present");
            assert_eq!(qb.height, LINE_HEIGHT + 1.0);
            assert_eq!(qb.y + qb.height, LINE_HEIGHT * 2.0 + 1.0);
        }

        /// Regression for the quadraui#1076 CI failure on `windows-latest`
        /// (`win::palette::tests::separator_paints_at_corrected_row_not_drifted_one`):
        /// `paint` used to stroke the separator at
        /// `query_bounds.y + query_bounds.height`, i.e. on the **first item
        /// row's top pixel** rather than in the pixel the query band
        /// reserves for it. Because item rows are painted *after* the
        /// separator, the first row's selection fill then overpainted it
        /// and the separator disappeared entirely whenever row 0 was
        /// selected — invisible to GTK/macOS's own tests only because
        /// their fixtures don't select row 0.
        #[test]
        fn separator_sits_in_the_reserved_query_pixel_not_the_item_row() {
            let p = sample();
            let (surface, palette_layout) = paint_sample(&p);
            let theme = Theme::default();
            let qb = palette_layout.query_bounds.expect("query bounds present");
            let first_row = palette_layout.visible_items[0].bounds;

            let sep_idx = surface
                .fills
                .iter()
                .position(|(r, c)| {
                    *c == theme.border_fg && r.height == 1.0 && r.width == AREA.width
                })
                .expect("separator fill emitted");
            let sep = surface.fills[sep_idx].0;

            assert_eq!(
                sep.y,
                qb.y + qb.height - 1.0,
                "separator must occupy the last pixel of the query band"
            );
            assert_eq!(
                first_row.y,
                qb.y + qb.height,
                "first item row starts immediately below the query band"
            );
            assert!(
                sep.y + sep.height <= first_row.y,
                "separator ({sep:?}) must not overlap the first item row ({first_row:?})"
            );

            // Selection fill for row 0 is emitted after the separator — if
            // the two overlapped, the separator would be invisible.
            let sel = surface
                .fills
                .iter()
                .skip(sep_idx)
                .find(|(_, c)| *c == theme.selected_bg)
                .map(|(r, _)| *r)
                .expect("selected row fill emitted after the separator");
            assert_eq!(sel.y, first_row.y);
            assert!(
                sel.y >= sep.y + sep.height,
                "row 0's selection fill ({sel:?}) overpaints the separator ({sep:?})"
            );
        }

        /// `PaletteMode::Input` hides the item list, so there is nothing
        /// for a separator to separate — none is painted.
        #[test]
        fn input_mode_paints_no_separator() {
            let mut p = sample();
            p.mode = PaletteMode::Input;
            let (surface, _) = paint_sample(&p);
            let theme = Theme::default();
            assert!(
                !surface
                    .fills
                    .iter()
                    .any(|(r, c)| *c == theme.border_fg && r.height == 1.0),
                "Input mode must not paint a query/list separator"
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_roundtrip_serde() {
        let palette = Palette {
            id: WidgetId::new("cmd-palette"),
            title: "Commands".to_string(),
            query: "open".to_string(),
            query_cursor: 4,
            items: vec![
                PaletteItem {
                    text: StyledText::plain("Open File"),
                    detail: Some(StyledText::plain("Ctrl+O")),
                    icon: None,
                    match_positions: vec![0, 1, 2, 3],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
                PaletteItem {
                    text: StyledText::plain("Open Recent"),
                    detail: None,
                    icon: None,
                    match_positions: vec![0, 1, 2, 3],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 42,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        };
        let json = serde_json::to_string(&palette).unwrap();
        let back: Palette = serde_json::from_str(&json).unwrap();
        assert_eq!(palette, back);
    }

    #[test]
    fn palette_event_roundtrip_serde() {
        let events = vec![
            PaletteEvent::QueryChanged {
                value: "foo".to_string(),
            },
            PaletteEvent::SelectionChanged { idx: 3 },
            PaletteEvent::ItemConfirmed { idx: 0 },
            PaletteEvent::Closed,
            PaletteEvent::KeyPressed {
                key: "Ctrl+P".to_string(),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: PaletteEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── D6 Palette layout API tests ───────────────────────────────────

    fn make_palette_item(text: &str) -> PaletteItem {
        PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions: vec![],
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn make_palette(
        title: &str,
        query: &str,
        items: Vec<PaletteItem>,
        selected: usize,
        scroll: usize,
    ) -> Palette {
        Palette {
            id: WidgetId::new("p"),
            title: title.to_string(),
            query: query.to_string(),
            query_cursor: 0,
            items,
            selected_idx: selected,
            scroll_offset: scroll,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    #[test]
    fn palette_layout_empty() {
        let p = make_palette("Commands", "", vec![], 0, 0);
        let layout = p.layout(40.0, 20.0, 0.0, 0.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert!(layout.title_bounds.is_none());
        assert!(layout.query_bounds.is_none());
        assert_eq!(layout.visible_items.len(), 0);
        assert_eq!(layout.hit_test(10.0, 5.0), PaletteHit::Empty);
    }

    #[test]
    fn palette_layout_stacks_title_query_items() {
        let p = make_palette(
            "Commands",
            "open",
            (0..3)
                .map(|i| make_palette_item(&format!("cmd{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(40.0, 10.0, 1.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        // Title at y=0 (h=1), query at y=1 (h=1), items at y=2,3,4.
        assert_eq!(layout.title_bounds.unwrap().y, 0.0);
        assert_eq!(layout.query_bounds.unwrap().y, 1.0);
        assert_eq!(layout.visible_items[0].bounds.y, 2.0);
        assert_eq!(layout.visible_items[2].bounds.y, 4.0);
        // Hit-tests.
        assert_eq!(layout.hit_test(10.0, 0.5), PaletteHit::Title);
        assert_eq!(layout.hit_test(10.0, 1.5), PaletteHit::Query);
        assert_eq!(layout.hit_test(10.0, 2.5), PaletteHit::Item(0));
    }

    #[test]
    fn palette_layout_no_title_query_only() {
        let p = make_palette("", "", vec![make_palette_item("a")], 0, 0);
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.query_bounds.unwrap().y, 0.0);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    #[test]
    fn palette_layout_scroll_offset_skips_items() {
        // 20 items, a 4-row visible window (viewport_height 5, query_h
        // 1 → items area is 4 rows), selected_idx sitting inside the
        // requested scroll window so the guard passes `scroll_offset`
        // through unchanged.
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            2,
            2,
        );
        let layout = p.layout(40.0, 5.0, 0.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        // Query at y=0, items from offset 2.
        assert_eq!(layout.visible_items[0].item_idx, 2);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    // ── #711: selection-visibility guard lives in the primitive ─────────

    #[test]
    fn resolved_scroll_offset_scrolls_forward_past_bottom() {
        // selected_idx (15) is past the bottom of a 9-row window that
        // starts at the stale scroll_offset (0) — must scroll forward
        // just enough to bring it into the last visible row.
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            15,
            0,
        );
        assert_eq!(p.resolved_scroll_offset(9), 7);
    }

    #[test]
    fn resolved_scroll_offset_scrolls_backward_past_top() {
        // selected_idx (1) sits above the stale scroll_offset (10) —
        // must scroll back to make it the top visible row.
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            1,
            10,
        );
        assert_eq!(p.resolved_scroll_offset(9), 1);
    }

    #[test]
    fn palette_layout_keeps_selection_visible_past_bottom() {
        // Regression for #711: `mac_palette_layout` fed `scroll_offset`
        // straight into `Palette::layout` with no visibility guard, so
        // moving the selection past the bottom of the window scrolled
        // it out of view instead of following it. The guard now lives
        // in `Palette::layout` itself, so every backend that calls it
        // (gtk, win, macos) gets correct behaviour with no backend-side
        // code — this test exercises the primitive directly, once, for
        // all of them.
        let items: Vec<_> = (0..20)
            .map(|i| make_palette_item(&format!("i{i}")))
            .collect();
        // scroll_offset stale at 0, but selected_idx (15) is past the
        // bottom of the 9-row visible window (viewport_height 10,
        // query_h 1 → items area is 9 rows of height 1.0 each).
        let p = make_palette("", "", items, 15, 0);
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert_eq!(layout.resolved_scroll_offset, 7);
        assert!(
            layout.visible_items.iter().any(|vi| vi.item_idx == 15),
            "selected item 15 should be visible in the resolved window"
        );
    }

    #[test]
    fn palette_layout_pixel_units() {
        // GTK-style: 32 px title, 40 px query, 24 px item rows.
        let p = make_palette(
            "Commands",
            "",
            (0..3)
                .map(|i| make_palette_item(&format!("c{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(400.0, 300.0, 32.0, 40.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(24.0)
        });
        assert_eq!(layout.title_bounds.unwrap().height, 32.0);
        assert_eq!(layout.query_bounds.unwrap().y, 32.0);
        assert_eq!(layout.query_bounds.unwrap().height, 40.0);
        assert_eq!(layout.visible_items[0].bounds.y, 72.0);
        assert_eq!(layout.visible_items[0].bounds.height, 24.0);
    }

    #[test]
    fn palette_layout_with_preview_splits_width() {
        let mut p = make_palette("Files", "", vec![make_palette_item("main.rs")], 0, 0);
        p.preview = Some(PalettePreview {
            lines: vec![StyledText::plain("fn main() {}")],
            title: Some("main.rs".into()),
            scroll_offset: 0,
            highlight_line: None,
        });
        let layout = p.layout(100.0, 50.0, 1.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert_eq!(layout.item_list_width, 40.0);
        assert_eq!(layout.visible_items[0].bounds.width, 40.0);
        let pb = layout.preview_bounds.unwrap();
        assert_eq!(pb.x, 40.0);
        assert_eq!(pb.width, 60.0);
        assert_eq!(pb.y, 2.0);
        assert_eq!(pb.height, 48.0);
    }

    #[test]
    fn palette_layout_without_preview_full_width() {
        let p = make_palette("Cmd", "", vec![make_palette_item("foo")], 0, 0);
        let layout = p.layout(100.0, 50.0, 1.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert_eq!(layout.item_list_width, 100.0);
        assert!(layout.preview_bounds.is_none());
        assert_eq!(layout.visible_items[0].bounds.width, 100.0);
    }

    #[test]
    fn palette_preview_hit_test() {
        let mut p = make_palette("Files", "", vec![make_palette_item("a.rs")], 0, 0);
        p.preview = Some(PalettePreview {
            lines: vec![],
            title: None,
            scroll_offset: 0,
            highlight_line: None,
        });
        let layout = p.layout(100.0, 50.0, 1.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert_eq!(layout.hit_test(50.0, 10.0), PaletteHit::Preview);
        assert_eq!(layout.hit_test(10.0, 2.5), PaletteHit::Item(0));
    }

    #[test]
    fn palette_scrollbar_present_when_overflow() {
        // 20 items, viewport fits 5 rows (query_h=1, items area = 10 - 1 = 9 rows,
        // item_h=1 → 9 visible, 20 total → overflow).
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 2.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        let sb = layout
            .scrollbar
            .as_ref()
            .expect("scrollbar should be present");
        assert_eq!(sb.track.x, 38.0); // item_list_width(40) - scrollbar_width(2)
        assert_eq!(sb.track.y, 1.0); // items_top = after query
        assert_eq!(sb.track.width, 2.0);
        assert_eq!(sb.track.height, 9.0); // items_bottom(10) - items_top(1)
        assert!(sb.thumb.height > 0.0);
        assert!(sb.thumb.height <= sb.track.height);
        // Items should be narrowed.
        assert_eq!(layout.visible_items[0].bounds.width, 38.0);
    }

    #[test]
    fn palette_scrollbar_none_when_no_overflow() {
        let p = make_palette("", "", vec![make_palette_item("a")], 0, 0);
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 2.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert!(layout.scrollbar.is_none());
        // Items keep full width when no scrollbar.
        assert_eq!(layout.visible_items[0].bounds.width, 40.0);
    }

    #[test]
    fn palette_scrollbar_none_when_width_zero() {
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 0.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        assert!(layout.scrollbar.is_none());
    }

    #[test]
    fn palette_scrollbar_hit_test() {
        let p = make_palette(
            "",
            "",
            (0..20)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, 2.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        let sb = layout.scrollbar.as_ref().unwrap();
        // Hit the thumb.
        let thumb_center_y = sb.thumb.y + sb.thumb.height / 2.0;
        assert_eq!(
            layout.hit_test(39.0, thumb_center_y),
            PaletteHit::ScrollbarThumb,
        );
        // Hit the track below the thumb.
        let below_thumb = sb.thumb.y + sb.thumb.height + 0.5;
        if below_thumb < sb.track.y + sb.track.height {
            assert_eq!(
                layout.hit_test(39.0, below_thumb),
                PaletteHit::ScrollbarTrack,
            );
        }
        // Item area should not cover the scrollbar column.
        assert_ne!(layout.hit_test(37.0, 1.5), PaletteHit::ScrollbarThumb);
    }

    #[test]
    fn palette_scrollbar_thumb_tracks_scroll_offset() {
        let items: Vec<_> = (0..20)
            .map(|i| make_palette_item(&format!("i{i}")))
            .collect();
        // Scroll at top.
        let p0 = make_palette("", "", items.clone(), 0, 0);
        let l0 = p0.layout(40.0, 10.0, 0.0, 1.0, 2.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        let sb0 = l0.scrollbar.as_ref().unwrap();
        // Scroll near bottom.
        let p1 = make_palette("", "", items, 15, 11);
        let l1 = p1.layout(40.0, 10.0, 0.0, 1.0, 2.0, 1.0, |_| {
            PaletteItemMeasure::new(1.0)
        });
        let sb1 = l1.scrollbar.as_ref().unwrap();
        assert!(
            sb1.thumb.y > sb0.thumb.y,
            "thumb should move down with scroll"
        );
    }

    #[test]
    fn palette_scrollbar_pixel_units() {
        // GTK-like dimensions.
        let p = make_palette(
            "Files",
            "main",
            (0..50)
                .map(|i| make_palette_item(&format!("f{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(400.0, 600.0, 32.0, 40.0, 6.0, 8.0, |_| {
            PaletteItemMeasure::new(24.0)
        });
        let sb = layout.scrollbar.as_ref().expect("scrollbar present");
        assert_eq!(sb.track.width, 6.0);
        assert!(sb.thumb.height >= 8.0, "thumb respects min_thumb_len");
        assert_eq!(sb.track.y, 72.0); // 32 + 40 = items_top
                                      // Items narrowed by scrollbar.
        let content_w = 400.0 - 6.0;
        assert_eq!(layout.visible_items[0].bounds.width, content_w);
    }

    #[test]
    fn palette_tree_item_serde_round_trip() {
        let item = PaletteItem {
            text: StyledText::plain("src/"),
            detail: None,
            icon: None,
            match_positions: vec![],
            depth: 2,
            expandable: true,
            expanded: true,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: PaletteItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.depth, 2);
        assert!(back.expandable);
        assert!(back.expanded);
    }
}
