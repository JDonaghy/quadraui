//! `IconGrid` primitive: a wrapping grid of large icons with a label
//! under each — Finder's "Icons" view / Explorer's "Large icons".
//!
//! Distinct from [`crate::ListView`] (one item per row, no wrapping) and
//! [`crate::primitives::board::BoardModel`] (kanban columns, not a
//! uniform tile grid). `IconGrid` is the right primitive for "folder of
//! files/apps shown as big icons": a file manager's icon view, an app
//! launcher grid, a sticker/emoji picker.
//!
//! # Backend contract
//!
//! **Purely declarative**, same as every other row-family primitive in
//! this crate: `IconGrid::layout` resolves `items[scroll_offset..]`
//! (row-quantised — a "row" here is one horizontal band of tiles, not
//! one item) wrapped into as many columns as fit the viewport, clipping
//! the last partially-visible row the same way
//! [`crate::ListView::layout`] clips its last row. The app owns
//! `selected_idx` / `scroll_offset` / `has_focus` for the next frame;
//! this module supplies the pure arithmetic ([`icon_grid_move_selection`])
//! an app's keyboard handler calls to compute the next `selected_idx`
//! from an [`IconGridNav`] key, and [`IconGridLayout::hit_test`] for
//! mouse clicks.
//!
//! # Selection model
//!
//! Single selection only (`selected_idx: usize`), matching
//! [`crate::ListView`] today. [`IconGridEvent::SelectionSet`] carries a
//! `Vec<usize>` (rather than a bare `idx`) for the same reason
//! [`crate::primitives::list::ListViewEvent::SelectionSet`] does: a
//! future multi-select extension widens the set this event already
//! reports without changing its shape — not implemented here.
//!
//! # TUI story
//!
//! Tiles are fixed-size cells: the icon glyph (Nerd Font or ASCII
//! fallback, centred) on the tile's first row, the label — truncated
//! with `…` — on its second. A tile only one cell tall (`tile_height <=
//! 1.0`, e.g. a viewport too short for even one 2-row tile) falls back
//! to a single-row `ls -l`-style `<glyph> <name>` layout instead of
//! dropping the label; see [`Primitive::paint_cells`]'s impl below.
//! Thumbnails ([`IconGridItem::image`]) are GUI-only and dropped on TUI
//! entirely — there is no ASCII-art fallback for a raster image here,
//! unlike [`crate::primitives::image::Image::fallback_text`]'s own
//! dedicated field for that case, because an `IconGrid` tile's icon
//! glyph already serves that role.

use crate::event::Rect;
use crate::primitives::image::Image;
use crate::primitives::row_stack;
use crate::types::{Decoration, Icon, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an `IconGrid` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{Icon, IconGrid, IconGridHit, IconGridItem, StyledText, WidgetId};
///
/// let grid = IconGrid {
///     id: WidgetId::new("icon_grid:files"),
///     items: vec![IconGridItem {
///         id: WidgetId::new("file:readme"),
///         icon: Icon::new("\u{f15b}", "F"),
///         image: None,
///         label: StyledText::plain("README.md"),
///         detail: None,
///         decoration: Default::default(),
///     }],
///     selected_idx: 0,
///     has_focus: true,
///     scroll_offset: 0,
///     tile_size: None,
/// };
///
/// let layout = grid.layout(100.0, 100.0, 40.0, 30.0);
/// assert_eq!(layout.hit_test(5.0, 5.0), IconGridHit::Item(0));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IconGrid {
    pub id: WidgetId,
    pub items: Vec<IconGridItem>,
    #[serde(default)]
    pub selected_idx: usize,
    /// Whether this grid currently holds keyboard focus. Gates the
    /// selected tile's highlight, same convention as
    /// [`crate::ListView::has_focus`] — see that field's doc for why
    /// this is deliberately not tied to a focus ring.
    #[serde(default)]
    pub has_focus: bool,
    /// Scroll offset in whole tile-rows (not items) — a "row" here is
    /// one horizontal band of tiles. The caller increments/decrements
    /// this in response to [`IconGridNav::PageUp`]/[`IconGridNav::PageDown`]
    /// or the layout's own selection-visible correction.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Optional caller-supplied tile size hint, in the layout caller's
    /// native units (cells on TUI, pixels on GUI — same convention as
    /// [`crate::primitives::list::ListItemMeasure::height`]). `None`
    /// means "use the default this backend derives from its own chrome
    /// metrics" — see [`Primitive::layout`](crate::primitive::Primitive::layout)'s
    /// impl below for the exact default.
    #[serde(default)]
    pub tile_size: Option<(f32, f32)>,
}

/// One tile in an `IconGrid`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IconGridItem {
    pub id: WidgetId,
    pub icon: Icon,
    /// Optional thumbnail, painted in place of [`Self::icon`] on a GUI
    /// backend (TUI always paints `icon`, never this — see the module
    /// doc's "TUI story"). `None` paints `icon` on every backend.
    #[serde(default)]
    pub image: Option<Image>,
    /// Label shown under the icon/thumbnail, truncated with `…` if it
    /// overflows the tile.
    pub label: StyledText,
    #[serde(default)]
    pub detail: Option<StyledText>,
    #[serde(default)]
    pub decoration: Decoration,
}

/// Resolved position of one visible tile after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleIconGridTile {
    /// Index into the original `IconGrid.items` Vec.
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconGridHit {
    /// Click landed on a tile. Carries the item's index into
    /// `IconGrid.items`.
    Item(usize),
    /// Click landed on the viewport's empty tail (an incomplete last
    /// row, or below the last row entirely).
    Empty,
}

/// Fully-resolved icon-grid layout.
///
/// `#[non_exhaustive]`: a backend-returned value, never constructed by a
/// consumer via struct literal, same convention as
/// [`crate::ListViewLayout`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IconGridLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// `floor(viewport_width / tile_width)`, clamped to at least `1` so
    /// a viewport narrower than one tile still lays out a single
    /// column rather than dividing by zero rows.
    pub columns: usize,
    pub tile_width: f32,
    pub tile_height: f32,
    /// Tiles that are at least partially visible, top to bottom, left
    /// to right within each row.
    pub visible_tiles: Vec<VisibleIconGridTile>,
    /// Ordered hit-region list, tiles only — no title/empty-tail entry
    /// (unlike [`crate::ListViewLayout::hit_regions`]'s title row,
    /// `IconGrid` has no header of its own).
    pub hit_regions: Vec<(Rect, IconGridHit)>,
    /// Scroll offset actually used, in whole tile-rows. Clamped to
    /// `[0, total_rows)` (or `0` when `items` is empty) and, when the
    /// caller's `scroll_offset` would leave `IconGrid::selected_idx`'s
    /// row outside the viewport, corrected to keep it visible — the
    /// same "layout bakes in every geometry decision" rule
    /// [`crate::primitive`]'s module doc describes, applied to scroll
    /// position instead of left to a separate controller.
    pub resolved_scroll_offset: usize,
    /// Sub-row pixel remainder when a non-`None` `scroll_px` (see
    /// [`IconGrid::layout_with_scroll_px`]) is in effect — see
    /// [`crate::primitives::scrollbar::resolve_scroll_px`]. `0.0` from
    /// plain [`IconGrid::layout`].
    pub scroll_sub_px: f32,
}

impl IconGridLayout {
    /// Test which tile (if any) contains `(x, y)`, relative to the
    /// rect this layout was computed for.
    pub fn hit_test(&self, x: f32, y: f32) -> IconGridHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return *hit;
            }
        }
        IconGridHit::Empty
    }
}

impl IconGrid {
    /// Compute the full rendering + hit-test layout for this grid.
    ///
    /// - `viewport_width`, `viewport_height` — available area in the
    ///   caller's unit.
    /// - `tile_width`, `tile_height` — uniform tile size in the same
    ///   unit. Columns are `floor(viewport_width / tile_width)`
    ///   (minimum `1`); rows stack vertically via the same
    ///   [`row_stack::layout_row_stack`] arithmetic every other
    ///   row-stacking primitive in this crate uses, with the last
    ///   visible row clipped to the viewport.
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        tile_width: f32,
        tile_height: f32,
    ) -> IconGridLayout {
        self.layout_with_scroll_px(
            viewport_width,
            viewport_height,
            tile_width,
            tile_height,
            None,
        )
    }

    /// Like [`Self::layout`], but with an explicit pixel-precise scroll
    /// position (`scroll_px`) a GUI painter has accumulated, instead of
    /// only [`IconGrid::scroll_offset`]'s whole-row value. `scroll_px`
    /// wins over `scroll_offset` when it is `Some` and usable — see
    /// [`crate::primitives::scrollbar::resolve_scroll_px`].
    pub fn layout_with_scroll_px(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        tile_width: f32,
        tile_height: f32,
        scroll_px: Option<f32>,
    ) -> IconGridLayout {
        let columns = if tile_width > 0.0 {
            ((viewport_width / tile_width).floor() as usize).max(1)
        } else {
            1
        };

        if self.items.is_empty() {
            return IconGridLayout {
                viewport_width,
                viewport_height,
                columns,
                tile_width,
                tile_height,
                visible_tiles: Vec::new(),
                hit_regions: Vec::new(),
                resolved_scroll_offset: 0,
                scroll_sub_px: 0.0,
            };
        }

        let total_rows = self.items.len().div_ceil(columns);
        let viewport_rows = if tile_height > 0.0 {
            (viewport_height / tile_height).floor() as usize
        } else {
            0
        };

        // Keep `selected_idx`'s row inside the viewport before resolving
        // `scroll_px` — the same correction
        // `crate::compose::tree_nav::TreeNavCore::scroll_to_visible`
        // performs for `TreeView`, baked into this layout call instead
        // of left to a separate controller (see `IconGridLayout::
        // resolved_scroll_offset`'s own doc).
        let mut scroll_offset_rows = self.scroll_offset.min(total_rows - 1);
        let selected = self.selected_idx.min(self.items.len() - 1);
        let selected_row = selected / columns;
        if viewport_rows > 0 {
            if selected_row < scroll_offset_rows {
                scroll_offset_rows = selected_row;
            } else if selected_row >= scroll_offset_rows + viewport_rows {
                scroll_offset_rows = selected_row + 1 - viewport_rows;
            }
        }

        let stack = row_stack::layout_row_stack(
            scroll_offset_rows,
            scroll_px,
            total_rows,
            0.0,
            0.0,
            viewport_height,
            viewport_width,
            |_row| (tile_height, ()),
        );

        let mut visible_tiles = Vec::new();
        let mut hit_regions = Vec::new();
        for row in &stack.visible {
            let row_start = row.idx * columns;
            for col in 0..columns {
                let item_idx = row_start + col;
                if item_idx >= self.items.len() {
                    break;
                }
                let x = col as f32 * tile_width;
                let w = tile_width.min((viewport_width - x).max(0.0));
                let bounds = Rect::new(x, row.bounds.y, w, row.bounds.height);
                visible_tiles.push(VisibleIconGridTile { item_idx, bounds });
                hit_regions.push((bounds, IconGridHit::Item(item_idx)));
            }
        }

        IconGridLayout {
            viewport_width,
            viewport_height,
            columns,
            tile_width,
            tile_height,
            visible_tiles,
            hit_regions,
            resolved_scroll_offset: stack.resolved_scroll_offset,
            scroll_sub_px: stack.scroll_sub_px,
        }
    }
}

/// A 2-D navigation key for [`icon_grid_move_selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconGridNav {
    /// Previous item in flat order — wraps from a row's first column to
    /// the previous row's last column (a plain `idx - 1`, clamped at
    /// `0`).
    Left,
    /// Next item in flat order — wraps from a row's last column to the
    /// next row's first column (a plain `idx + 1`, clamped at `len - 1`).
    Right,
    /// Same column, one row up. Clamped at the top row rather than
    /// wrapping to the bottom.
    Up,
    /// Same column, one row down. Clamped at the last item rather than
    /// wrapping to the top.
    Down,
    /// First item.
    Home,
    /// Last item.
    End,
    /// Same column, `page_rows` rows up.
    PageUp,
    /// Same column, `page_rows` rows down.
    PageDown,
}

/// Resolve `nav` against a grid of `total` items laid out in `columns`
/// columns, returning the new selected index. `page_rows` is how many
/// tile-rows [`IconGridNav::PageUp`]/[`IconGridNav::PageDown`] move by —
/// typically [`IconGridLayout::viewport_height`] divided by
/// [`IconGridLayout::tile_height`], floored to at least `1`.
///
/// Pure arithmetic over `(current, total, columns)`, independent of any
/// particular [`IconGrid`] value, so a host's keyboard handler (and this
/// module's own tests) can call it without laying out a real grid
/// first — the same shape as [`crate::types::apply_selection_gesture`].
///
/// `total == 0` or `columns == 0` returns `current` unchanged — there is
/// nothing to navigate to. `current` is clamped into `[0, total)`
/// before `nav` is applied, so a stale out-of-range index never panics.
pub fn icon_grid_move_selection(
    current: usize,
    total: usize,
    columns: usize,
    page_rows: usize,
    nav: IconGridNav,
) -> usize {
    if total == 0 || columns == 0 {
        return current;
    }
    let idx = current.min(total - 1);
    let page = columns * page_rows.max(1);
    match nav {
        IconGridNav::Left => idx.saturating_sub(1),
        IconGridNav::Right => (idx + 1).min(total - 1),
        // Clamped, not wrapped: an `Up` at the top row keeps the same
        // column (`idx % columns`) rather than jumping to the bottom —
        // mirrors a plain grid widget's convention, not `Left`/`Right`'s
        // documented wrap.
        IconGridNav::Up => idx.checked_sub(columns).unwrap_or(idx % columns),
        IconGridNav::Down => (idx + columns).min(total - 1),
        IconGridNav::Home => 0,
        IconGridNav::End => total - 1,
        IconGridNav::PageUp => idx.checked_sub(page).unwrap_or(idx % columns),
        IconGridNav::PageDown => (idx + page).min(total - 1),
    }
}

/// Events an `IconGrid` emits back to the app. Not produced by
/// [`IconGrid::layout`] itself — a host derives these from
/// [`crate::UiEvent`] plus [`IconGridLayout::hit_test`] /
/// [`icon_grid_move_selection`], the same convention every other
/// row-family primitive's `*Event` enum follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconGridEvent {
    /// Keyboard/mouse moved selection to a different tile.
    SelectionChanged { idx: usize },
    /// Enter (keyboard) or double-click (mouse) activated a tile.
    ItemActivated { idx: usize },
    /// The full selection, as a de-duplicated, sorted set of indices.
    /// Always exactly one entry today (single-selection only — see the
    /// module doc's "Selection model") — carries a `Vec` rather than a
    /// bare `idx` so a future multi-select extension widens what this
    /// already reports without changing its shape.
    SelectionSet { indices: Vec<usize> },
}

// ── Accessibility ────────────────────────────────────────────────────
//
// `IconGrid`'s `A11yPrimitive` impl lives here rather than in
// `crate::a11y::tree` (where `ListView`/`TreeView`'s impls live) — the
// same "new primitive owns its own a11y impl" precedent
// `TreeTable::a11y_info`'s doc describes. `A11yRole` has no dedicated
// grid/grid-cell variant yet, so this reports the closest existing
// role pair (`List`/`ListItem`), same as `ListView`.

impl crate::a11y::A11yPrimitive for IconGrid {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::List)
            .with_set_size(self.items.len() as u32)
    }

    fn a11y_children(&self) -> Vec<crate::a11y::A11yChild> {
        let set_size = self.items.len() as u32;
        self.items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let mut info = crate::a11y::A11yInfo::new()
                    .with_role(crate::a11y::A11yRole::ListItem)
                    .with_selected(idx == self.selected_idx)
                    .with_position_in_set(idx as u32 + 1)
                    .with_set_size(set_size);
                let label: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
                if !label.trim().is_empty() {
                    info = info.with_label(label);
                }
                if let Some(detail) = item.detail.as_ref() {
                    let text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
                    if !text.trim().is_empty() {
                        info = info.with_value(text);
                    }
                }
                crate::a11y::A11yChild::new(format!("item.{idx}"), info)
            })
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        if self.selected_idx < self.items.len() {
            Some(format!("item.{}", self.selected_idx))
        } else {
            None
        }
    }
}

// ── GUI paint: centred icon + label tile ────────────────────────────

/// Default tile size derived from a [`crate::primitive::LayoutCtx`] when
/// [`IconGrid::tile_size`] is `None`. TUI (`ctx.line_height <= 1.0`)
/// gets a cell-quantised size; every pixel backend gets a size scaled
/// off its own chrome metrics.
///
/// Gated on the same feature set as the
/// [`crate::primitive::Primitive`] impl below, its only caller: a
/// zero-backend build compiles the descriptor and its layout/nav math
/// but has no renderer to resolve a default tile size for.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn default_tile_size(ctx: &crate::primitive::LayoutCtx) -> (f32, f32) {
    if ctx.line_height <= 1.0 {
        // TUI: 12 cells wide (enough for a short filename plus margin),
        // 2 cells tall (icon row + label row) — `Primitive::paint_cells`
        // falls back to a 1-row tile when the rect itself is too short
        // for even one 2-row tile, but the *default* tile height stays
        // `2.0` here regardless, since that decision depends on the
        // viewport, not the tile hint.
        (12.0, 2.0)
    } else {
        (ctx.char_width * 12.0, ctx.line_height * 3.0)
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{IconGrid, IconGridLayout};
    use crate::paint_surface::PaintSurface;
    use crate::primitives::row_stack::row_content_paint;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::Rect;

    /// Paint an [`IconGrid`]'s visible tiles — background, each tile's
    /// centred icon/label (selection highlight behind the selected
    /// tile) — onto `surface` at `area`.
    ///
    /// `grid_layout` must be the same [`IconGridLayout`] the caller uses
    /// for hit-testing, so paint and hit-test can never disagree —
    /// mirrors [`crate::primitives::list::native_surface_paint::paint`]'s
    /// own `list_layout` contract.
    pub(crate) fn paint(
        grid: &IconGrid,
        area: Rect,
        grid_layout: &IconGridLayout,
        nerd_fonts_enabled: bool,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        _style: &Style,
    ) {
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }
        surface.surface_push_clip(area);
        surface.surface_fill_rect(area, theme.background);

        for tile in &grid_layout.visible_tiles {
            let item = &grid.items[tile.item_idx];
            let tile_area = Rect::new(
                area.x + tile.bounds.x,
                area.y + tile.bounds.y,
                tile.bounds.width,
                tile.bounds.height,
            );
            let selected = tile.item_idx == grid.selected_idx && grid.has_focus;
            let label: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
            row_content_paint::paint_tile_icon_label(
                surface,
                tile_area,
                Some(&item.icon),
                nerd_fonts_enabled,
                &label,
                theme.surface_fg,
                selected,
                theme.selected_bg,
            );
        }

        surface.surface_pop_clip();
    }
}

// ── `crate::primitive::Primitive` impl ──────────────────────────────

/// [`crate::primitive::Primitive::Layout`] for [`IconGrid`] — the
/// grid-local [`IconGridLayout`] paired with the absolute area it was
/// laid out into, mirroring [`crate::primitives::list::ListViewPrimitiveLayout`]'s
/// shape.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`IconGridLayout`]
/// itself.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IconGridPrimitiveLayout {
    /// Absolute area [`IconGrid::layout`] was called with —
    /// [`Self::hit_test`] subtracts this rect's origin before
    /// delegating to [`IconGridLayout::hit_test`], whose `hit_regions`
    /// are grid-local.
    pub area: Rect,
    /// Nerd-Font toggle resolved from the
    /// [`crate::primitive::LayoutCtx`]
    /// [`Primitive::layout`](crate::primitive::Primitive::layout) was
    /// called with.
    pub nerd_fonts_enabled: bool,
    /// The grid-local layout itself.
    pub layout: IconGridLayout,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl IconGridPrimitiveLayout {
    /// Hit-test at an absolute `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> IconGridHit {
        self.layout.hit_test(x - self.area.x, y - self.area.y)
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::primitive::Primitive for IconGrid {
    type Layout = IconGridPrimitiveLayout;
    type Event = IconGridEvent;

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        let (default_w, default_h) = default_tile_size(ctx);
        let (mut tile_w, mut tile_h) = self.tile_size.unwrap_or((default_w, default_h));
        if tile_w <= 0.0 {
            tile_w = default_w;
        }
        if tile_h <= 0.0 {
            tile_h = default_h;
        }

        #[cfg(feature = "tui")]
        if ctx.line_height <= 1.0 {
            // Same "fall back to a 1-row tile" rule
            // `Self::paint_cells` applies when painting — kept here too
            // so layout and paint never disagree on tile height. See
            // the module doc's "TUI story".
            if rect.height < 2.0 && tile_h > 1.0 {
                tile_h = 1.0;
            }
            let layout = self.layout(rect.width, rect.height, tile_w, tile_h);
            return IconGridPrimitiveLayout {
                area: rect,
                nerd_fonts_enabled: ctx.nerd_fonts_enabled,
                layout,
            };
        }

        let layout = self.layout(rect.width, rect.height, tile_w, tile_h);
        IconGridPrimitiveLayout {
            area: rect,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
        }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(
                self,
                layout.area,
                &layout.layout,
                layout.nerd_fonts_enabled,
                surface,
                theme,
                style,
            );
        }
        // A `tui`-only build has no `PaintSurface` to paint into at all
        // — `self` reaching the screen on TUI goes through
        // `Self::paint_cells` instead, which `DrawsPrimitives::
        // draw_primitive`'s default body calls in place of this method
        // whenever `Backend::cell_buffer` answers `Some`.
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Paints directly onto the TUI cell buffer — there is no separate
    /// `crate::tui::draw_icon_grid` module to delegate to, unlike
    /// [`crate::ListView`]/[`crate::TreeView`]'s own TUI rasterisers;
    /// this is `IconGrid`'s one and only TUI rasteriser.
    ///
    /// Each tile paints its icon glyph (Nerd Font vs. ASCII fallback per
    /// `nerd_fonts_enabled`) centred on the tile's first row and its
    /// label — truncated with `…` via
    /// [`crate::text_util::fit_chars_to_cells`] — centred on the second,
    /// unless [`IconGridLayout::tile_height`] is `<= 1.0` (the rect was
    /// too short for even one 2-row tile), in which case it falls back
    /// to a single `ls`-style `<glyph> <name>` row. The selected tile
    /// (when [`IconGrid::has_focus`]) paints with
    /// [`crate::theme::Theme::selected_bg`] behind it.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        use crate::text_util::{char_cell_width, display_width, fit_chars_to_cells};
        use crate::tui::{ratatui_color, set_cell, set_cell_wide};

        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        if area.width == 0 || area.height == 0 {
            return;
        }

        let fg = ratatui_color(theme.surface_fg);
        let bg = ratatui_color(theme.background);
        let sel_bg = ratatui_color(theme.selected_bg);
        let one_row = layout.layout.tile_height <= 1.0;

        for tile in &layout.layout.visible_tiles {
            let item = &self.items[tile.item_idx];
            let tx = area.x + tile.bounds.x.round() as u16;
            let ty = area.y + tile.bounds.y.round() as u16;
            let tw = tile.bounds.width.round() as u16;
            if tw == 0 {
                continue;
            }
            let selected = tile.item_idx == self.selected_idx && self.has_focus;
            let row_bg = if selected { sel_bg } else { bg };

            let glyph = if layout.nerd_fonts_enabled {
                item.icon.glyph.as_str()
            } else {
                item.icon.fallback.as_str()
            };
            let label: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();

            if one_row {
                // Fall back to a single-row `<glyph> <name>` tile — the
                // module doc's "TUI story".
                for col in 0..tw {
                    set_cell(buf, tx + col, ty, ' ', fg, row_bg);
                }
                let mut col: u16 = 0;
                for ch in glyph.chars() {
                    let w = char_cell_width(ch);
                    if col + w > tw {
                        break;
                    }
                    if w == 2 {
                        set_cell_wide(buf, tx + col, ty, ch, fg, row_bg);
                    } else {
                        set_cell(buf, tx + col, ty, ch, fg, row_bg);
                    }
                    col += w;
                }
                if col < tw {
                    col += 1; // one cell of gap between glyph and name
                }
                let budget = tw.saturating_sub(col);
                let (fit_chars, ellipsis, _fit_w) = fit_chars_to_cells(&label, budget);
                let mut vcol = col;
                for (i, ch) in label.chars().enumerate() {
                    if i as u16 >= fit_chars {
                        break;
                    }
                    let w = char_cell_width(ch);
                    if w == 2 {
                        set_cell_wide(buf, tx + vcol, ty, ch, fg, row_bg);
                    } else {
                        set_cell(buf, tx + vcol, ty, ch, fg, row_bg);
                    }
                    vcol += w;
                }
                if ellipsis && vcol < tw {
                    set_cell(buf, tx + vcol, ty, '…', fg, row_bg);
                }
                continue;
            }

            let th = tile.bounds.height.round() as u16;
            for row in 0..th {
                for col in 0..tw {
                    set_cell(buf, tx + col, ty + row, ' ', fg, row_bg);
                }
            }

            // Icon glyph, centred on the tile's first row.
            let glyph_w = display_width(glyph) as u16;
            if glyph_w <= tw {
                let gx = tx + (tw - glyph_w) / 2;
                let mut col = gx;
                for ch in glyph.chars() {
                    let w = char_cell_width(ch);
                    if w == 2 {
                        set_cell_wide(buf, col, ty, ch, fg, row_bg);
                    } else {
                        set_cell(buf, col, ty, ch, fg, row_bg);
                    }
                    col += w;
                }
            }

            // Label, truncated with an ellipsis, centred on the second
            // row (every row after the first is blank filler on a
            // taller-than-2 tile).
            if th >= 2 {
                let (fit_chars, ellipsis, fit_w) = fit_chars_to_cells(&label, tw);
                let lx = tx + (tw.saturating_sub(fit_w)) / 2;
                let mut col = lx;
                for (i, ch) in label.chars().enumerate() {
                    if i as u16 >= fit_chars {
                        break;
                    }
                    let w = char_cell_width(ch);
                    if w == 2 {
                        set_cell_wide(buf, col, ty + 1, ch, fg, row_bg);
                    } else {
                        set_cell(buf, col, ty + 1, ch, fg, row_bg);
                    }
                    col += w;
                }
                if ellipsis {
                    set_cell(buf, col, ty + 1, '…', fg, row_bg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon() -> Icon {
        Icon::new("\u{f15b}", "F")
    }

    fn item(name: &str) -> IconGridItem {
        IconGridItem {
            id: WidgetId::new(format!("file:{name}")),
            icon: icon(),
            image: None,
            label: StyledText::plain(name),
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn grid(n: usize) -> IconGrid {
        IconGrid {
            id: WidgetId::new("icon_grid:test"),
            items: (0..n).map(|i| item(&format!("item-{i}"))).collect(),
            selected_idx: 0,
            has_focus: true,
            scroll_offset: 0,
            tile_size: None,
        }
    }

    // ── layout ───────────────────────────────────────────────────────

    #[test]
    fn layout_computes_expected_column_count() {
        // 20 items, 40-col rect, 10-wide tiles: floor(40 / 10) = 4 columns.
        let g = grid(20);
        let layout = g.layout(40.0, 100.0, 10.0, 10.0);
        assert_eq!(layout.columns, 4);
    }

    #[test]
    fn layout_wraps_items_into_rows_left_to_right() {
        let g = grid(10);
        let layout = g.layout(40.0, 100.0, 10.0, 10.0);
        // 4 columns: row 0 = items 0..4, row 1 = items 4..8.
        let row0: Vec<usize> = layout
            .visible_tiles
            .iter()
            .filter(|t| t.bounds.y == 0.0)
            .map(|t| t.item_idx)
            .collect();
        assert_eq!(row0, vec![0, 1, 2, 3]);
        let row1: Vec<usize> = layout
            .visible_tiles
            .iter()
            .filter(|t| t.bounds.y == 10.0)
            .map(|t| t.item_idx)
            .collect();
        assert_eq!(row1, vec![4, 5, 6, 7]);
    }

    #[test]
    fn layout_reflows_on_resize() {
        let g = grid(20);
        let wide = g.layout(40.0, 100.0, 10.0, 10.0);
        assert_eq!(wide.columns, 4);
        let narrow = g.layout(20.0, 100.0, 10.0, 10.0);
        assert_eq!(narrow.columns, 2);
        // Item 3 is on row 0 at 4 columns, but row 1 at 2 columns.
        let wide_row = wide
            .visible_tiles
            .iter()
            .find(|t| t.item_idx == 3)
            .unwrap()
            .bounds
            .y;
        let narrow_row = narrow
            .visible_tiles
            .iter()
            .find(|t| t.item_idx == 3)
            .unwrap()
            .bounds
            .y;
        assert_eq!(wide_row, 0.0);
        assert_eq!(narrow_row, 10.0);
    }

    #[test]
    fn layout_clips_the_last_row_to_the_viewport() {
        let g = grid(8); // 4 cols -> 2 rows at 10-tall tiles.
        let layout = g.layout(40.0, 15.0, 10.0, 10.0);
        let last = layout
            .visible_tiles
            .iter()
            .find(|t| t.item_idx == 4)
            .unwrap();
        assert_eq!(last.bounds.height, 5.0);
    }

    #[test]
    fn layout_on_empty_items_has_no_tiles() {
        let g = grid(0);
        let layout = g.layout(40.0, 100.0, 10.0, 10.0);
        assert!(layout.visible_tiles.is_empty());
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn layout_corrects_scroll_offset_to_keep_selection_visible() {
        let mut g = grid(20); // 4 cols -> 5 rows at 10-tall tiles.
        g.selected_idx = 16; // row 4
        g.scroll_offset = 0;
        let layout = g.layout(40.0, 20.0, 10.0, 10.0); // 2 visible rows.
                                                       // Row 4 must be the last visible row: scroll offset == 3.
        assert_eq!(layout.resolved_scroll_offset, 3);
    }

    // ── hit_test ─────────────────────────────────────────────────────

    #[test]
    fn hit_test_resolves_to_the_right_item() {
        let g = grid(20);
        let layout = g.layout(40.0, 100.0, 10.0, 10.0);
        // Column 2, row 1 -> item 4*1 + 2 = 6.
        assert_eq!(layout.hit_test(25.0, 15.0), IconGridHit::Item(6));
    }

    #[test]
    fn hit_test_outside_any_tile_is_empty() {
        let g = grid(3);
        let layout = g.layout(40.0, 100.0, 10.0, 10.0);
        assert_eq!(layout.hit_test(5.0, 50.0), IconGridHit::Empty);
    }

    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn primitive_layout_hit_test_is_area_relative() {
        let g = grid(20);
        let ctx = crate::primitive::LayoutCtx {
            line_height: 16.0,
            char_width: 8.0,
            nerd_fonts_enabled: false,
        };
        let area = Rect::new(100.0, 50.0, 400.0, 400.0);
        let layout = crate::primitive::Primitive::layout(&g, &ctx, area);
        let tile0 = layout
            .layout
            .visible_tiles
            .iter()
            .find(|t| t.item_idx == 0)
            .unwrap()
            .bounds;
        assert_eq!(
            layout.hit_test(area.x + tile0.x + 1.0, area.y + tile0.y + 1.0),
            IconGridHit::Item(0)
        );
    }

    // ── icon_grid_move_selection ─────────────────────────────────────

    #[test]
    fn left_right_wrap_across_rows() {
        // 4 columns: item 4 is row 1 col 0; Left from it lands on item 3
        // (row 0 col 3) — a plain decrement, which is the "wrap" the
        // issue describes.
        assert_eq!(icon_grid_move_selection(4, 20, 4, 1, IconGridNav::Left), 3);
        assert_eq!(icon_grid_move_selection(3, 20, 4, 1, IconGridNav::Right), 4);
    }

    #[test]
    fn left_at_zero_is_clamped() {
        assert_eq!(icon_grid_move_selection(0, 20, 4, 1, IconGridNav::Left), 0);
    }

    #[test]
    fn right_at_last_item_is_clamped() {
        assert_eq!(
            icon_grid_move_selection(19, 20, 4, 1, IconGridNav::Right),
            19
        );
    }

    #[test]
    fn up_down_move_by_one_column_count() {
        assert_eq!(icon_grid_move_selection(6, 20, 4, 1, IconGridNav::Up), 2);
        assert_eq!(icon_grid_move_selection(6, 20, 4, 1, IconGridNav::Down), 10);
    }

    #[test]
    fn up_at_top_row_keeps_the_column() {
        assert_eq!(icon_grid_move_selection(2, 20, 4, 1, IconGridNav::Up), 2);
    }

    #[test]
    fn down_past_the_end_clamps_to_the_last_item() {
        // 20 items, 4 cols -> last row is items 16..20. Down from item
        // 18 (row 4 col 2) would land on item 22, past the end.
        assert_eq!(
            icon_grid_move_selection(18, 20, 4, 1, IconGridNav::Down),
            19
        );
    }

    #[test]
    fn home_and_end() {
        assert_eq!(icon_grid_move_selection(10, 20, 4, 1, IconGridNav::Home), 0);
        assert_eq!(icon_grid_move_selection(10, 20, 4, 1, IconGridNav::End), 19);
    }

    #[test]
    fn page_up_and_page_down_move_by_page_rows_rows() {
        assert_eq!(
            icon_grid_move_selection(18, 30, 4, 2, IconGridNav::PageDown),
            26
        );
        assert_eq!(
            icon_grid_move_selection(18, 30, 4, 2, IconGridNav::PageUp),
            10
        );
    }

    #[test]
    fn move_selection_on_empty_grid_is_unchanged() {
        assert_eq!(icon_grid_move_selection(0, 0, 4, 1, IconGridNav::Right), 0);
    }

    // ── serde ────────────────────────────────────────────────────────

    #[test]
    fn icon_grid_roundtrip_serde() {
        let g = grid(3);
        let json = serde_json::to_string(&g).unwrap();
        let back: IconGrid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn icon_grid_item_without_optional_fields_deserializes() {
        let json = serde_json::json!({
            "id": "file:a",
            "icon": {"glyph": "f", "fallback": "F"},
            "label": {"spans": [{"text": "a.txt", "fg": null, "bg": null, "bold": false, "italic": false, "underline": false}]}
        });
        let item: IconGridItem = serde_json::from_value(json).unwrap();
        assert!(item.image.is_none());
        assert!(item.detail.is_none());
        assert_eq!(item.decoration, Decoration::Normal);
    }

    // ── a11y ─────────────────────────────────────────────────────────

    #[test]
    fn a11y_children_mark_the_selected_tile() {
        use crate::a11y::A11yPrimitive;
        let mut g = grid(3);
        g.selected_idx = 1;
        let children = g.a11y_children();
        assert_eq!(children.len(), 3);
        assert_eq!(children[1].info.selected, Some(true));
        assert_eq!(children[0].info.selected, Some(false));
    }

    #[test]
    fn a11y_focus_key_tracks_selected_idx() {
        use crate::a11y::A11yPrimitive;
        let mut g = grid(3);
        g.selected_idx = 2;
        assert_eq!(g.a11y_focus_key(), Some("item.2".to_string()));
    }

    // ── `Primitive` TUI paint_cells parity ───────────────────────────

    #[cfg(feature = "tui")]
    mod tui_paint {
        use super::*;
        use crate::primitive::{LayoutCtx, Primitive};
        use crate::theme::Theme;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        fn render(g: &IconGrid, area: Rect) -> ratatui::buffer::Buffer {
            let ctx = LayoutCtx {
                line_height: 1.0,
                char_width: 1.0,
                nerd_fonts_enabled: false,
            };
            let layout = Primitive::layout(g, &ctx, area);
            let mut terminal =
                Terminal::new(TestBackend::new(area.width as u16, area.height as u16))
                    .expect("test terminal");
            terminal
                .draw(|frame| {
                    Primitive::paint_cells(g, &layout, frame.buffer_mut(), &Theme::default());
                })
                .expect("draw");
            terminal.backend().buffer().clone()
        }

        #[test]
        fn two_row_tile_paints_glyph_then_label() {
            let g = grid(1);
            let area = Rect::new(0.0, 0.0, 12.0, 4.0);
            let buf = render(&g, area);
            let row1: String = (0..12)
                .map(|x| buf.cell((x, 1)).unwrap().symbol().chars().next().unwrap())
                .collect();
            assert!(
                row1.contains("item-0"),
                "label row must contain the item's name, got {row1:?}"
            );
        }

        #[test]
        fn short_rect_falls_back_to_one_row_tile() {
            let g = grid(1);
            let area = Rect::new(0.0, 0.0, 20.0, 1.0);
            let buf = render(&g, area);
            let row0: String = (0..20)
                .map(|x| buf.cell((x, 0)).unwrap().symbol().chars().next().unwrap())
                .collect();
            assert!(
                row0.contains("item-0"),
                "one-row fallback must still show the name on the only row, got {row0:?}"
            );
        }

        #[test]
        fn short_rect_one_row_fallback_truncates_in_the_right_tile() {
            // Two 10-wide tiles side by side (tx = 0 and tx = 10). The
            // second tile's label is long enough to need truncation —
            // the `…` must land inside that tile's own 10-cell span,
            // not bleed into the first tile's.
            let mut g = grid(2);
            g.tile_size = Some((10.0, 2.0));
            g.items[1].label = StyledText::plain("a-very-long-file-name");
            let area = Rect::new(0.0, 0.0, 20.0, 1.0);
            let buf = render(&g, area);
            let row0: Vec<char> = (0..20)
                .map(|x| buf.cell((x, 0)).unwrap().symbol().chars().next().unwrap())
                .collect();
            let first_tile = &row0[0..10];
            let second_tile = &row0[10..20];
            assert!(
                !first_tile.contains(&'…'),
                "ellipsis must not bleed into the first tile, got {row0:?}"
            );
            assert!(
                second_tile.contains(&'…'),
                "ellipsis must land inside the truncating (second) tile, got {row0:?}"
            );
        }

        #[test]
        fn selected_tile_paints_with_the_selection_background() {
            let mut g = grid(1);
            g.has_focus = true;
            g.selected_idx = 0;
            let area = Rect::new(0.0, 0.0, 12.0, 4.0);
            let buf = render(&g, area);
            let theme = Theme::default();
            let sel_bg = crate::tui::ratatui_color(theme.selected_bg);
            assert_eq!(buf.cell((0, 0)).unwrap().bg, sel_bg);
        }
    }
}
