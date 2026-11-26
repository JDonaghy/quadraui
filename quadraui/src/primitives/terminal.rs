//! `Terminal` primitive: a 2D grid of styled cells, used as the rendering
//! surface for VT100-compatible terminal emulators.
//!
//! The primitive is a *snapshot* — the source-of-truth (vimcode's
//! `vt100::Parser` + `TerminalPane` history ring buffer) lives in the
//! engine. Each frame the engine builds a fresh `Terminal` describing
//! the cells visible at the current scroll offset, including any
//! selection / cursor / find-match overlays.
//!
//! Per-cell foreground/background are explicit `Color` values rather than
//! palette indices — vimcode resolves vt100 palette colors through the
//! active theme before populating the cell grid, so the primitive is
//! palette-agnostic.
//!
//! # Widget contract
//!
//! `Terminal` implements [`crate::primitive::Primitive`] and
//! [`crate::interactive::Interactive`], so it reaches a frame as
//! [`crate::frame::Surface::Widget`] and paints through
//! [`crate::DrawsPrimitives::draw_dyn`] /
//! [`crate::DrawsPrimitives::draw_primitive`] on every backend:
//!
//! - `layout` resolves the viewport → grid conversion, reserving the
//!   scrollbar gutter first, so [`TerminalLayout::hit_test`] /
//!   [`TerminalLayout::cell_bounds`] describe exactly the cells paint
//!   drew. The gutter is [`TerminalScrollbar::width`] when set, else one
//!   cell on a cell grid and [`crate::Style::scrollbar_width`] on a pixel
//!   surface. Pixel cell metrics are the editor font's
//!   ([`crate::primitive::LayoutCtx::editor_line_height`] and
//!   [`crate::primitive::LayoutCtx::char_width`]).
//! - `paint` runs the shared pixel painter in the editor font;
//!   `paint_cells` runs [`cell_paint`] against [`crate::CellSurface`].
//! - `Interactive` owns the pointer: a left press on a cell takes focus
//!   and starts a selection ([`TerminalEvent::SelectStart`]), a drag
//!   extends it, the release ends it, and a wheel notch becomes
//!   [`TerminalEvent::Scroll`]. Keys are not consumed — a focused
//!   terminal lets every key and committed text bubble to the host's
//!   `AppLogic::handle`, which forwards them to the PTY.
//!
//! [`TerminalDivider`] is the split-pane divider drawn between two
//! side-by-side terminals ([`TerminalSplitLayout::divider_x`]).
//!
//! For high-FPS terminals (60+ fps), a host may compare consecutive
//! `Terminal` snapshots with [`Terminal::dirty_rows`].

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a terminal cell grid.
///
/// `cells[row][col]` — outer Vec is rows top-to-bottom, inner Vec is
/// columns left-to-right. Rows may be ragged (different lengths) but
/// backends should treat missing trailing cells as blank.
///
/// Stays a plain, non-`#[non_exhaustive]` struct: a published consumer
/// builds it with an exhaustive struct literal today, so marking it
/// `#[non_exhaustive]` would break that consumer's build. `Default` is
/// still provided for consumers that want the
/// `Default::default()`-plus-field-assignment pattern.
///
/// # Examples
///
/// ```
/// use quadraui::{Color, Terminal, TerminalCell, TerminalHit, WidgetId};
///
/// let cell = TerminalCell {
///     text: "$".to_string(),
///     fg: Color::rgb(200, 200, 200),
///     bg: Color::rgb(0, 0, 0),
///     bold: false,
///     italic: false,
///     underline: false,
///     dim: false,
///     selected: false,
///     is_cursor: true,
///     cursor_shape: Default::default(),
///     cursor_blinking: false,
///     is_find_match: false,
///     is_find_active: false,
/// };
///
/// let mut terminal = Terminal::default();
/// terminal.id = WidgetId::new("terminal:shell");
/// terminal.cells = vec![vec![cell]];
///
/// let layout = terminal.layout(80.0, 24.0, 1.0, 1.0);
/// assert_eq!(layout.hit_test(0.0, 0.0), TerminalHit::Cell { row: 0, col: 0 });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: WidgetId,
    pub cells: Vec<Vec<TerminalCell>>,
    /// When present, the terminal paints a themed vertical
    /// [`crate::Scrollbar`] in a gutter at its right edge.
    #[serde(default)]
    pub scrollbar: Option<TerminalScrollbar>,
}

impl Default for Terminal {
    /// An empty-id terminal with no cells and no scrollbar.
    fn default() -> Self {
        Self {
            id: WidgetId::new(""),
            cells: Vec::new(),
            scrollbar: None,
        }
    }
}

/// Scrollbar state for a `Terminal`. Paint builds a
/// [`crate::Scrollbar::vertical`] from these values on the gutter
/// [`TerminalLayout::scrollbar`] reserved, and paints it with the
/// scrollbar's own painter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScrollbar {
    pub total_lines: usize,
    pub visible_lines: usize,
    pub scroll_offset: usize,
    /// When `true`, `scroll_offset = 0` means "at the bottom" (live
    /// view) and increasing offset scrolls up into history. The
    /// rasteriser flips the offset before constructing the visual
    /// scrollbar so the thumb sits at the bottom when offset is 0.
    #[serde(default)]
    pub inverted: bool,
    /// Scrollbar width in surface-native units (pixels on a pixel
    /// backend, cells on TUI). When `None`, the gutter is one cell on a
    /// cell grid and [`crate::Style::scrollbar_width`] on a pixel surface.
    #[serde(default)]
    pub width: Option<u16>,
}

impl TerminalScrollbar {
    /// The themed vertical bar this state paints as on `track`, one row
    /// per `row_height`.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    pub(crate) fn bar(&self, id: WidgetId, track: Rect, row_height: f32) -> crate::Scrollbar {
        crate::Scrollbar::vertical(
            id,
            track,
            self.effective_scroll_offset() as f32,
            self.total_lines as f32,
            self.visible_lines as f32,
            row_height,
        )
    }

    /// Scroll offset suitable for `Scrollbar::vertical`. When
    /// `inverted`, flips so offset 0 maps to the track bottom.
    pub fn effective_scroll_offset(&self) -> usize {
        if self.inverted {
            self.total_lines
                .saturating_sub(self.visible_lines)
                .saturating_sub(self.scroll_offset)
        } else {
            self.scroll_offset
        }
    }
}

/// One styled cell in a `Terminal`. Carries the rendered grapheme
/// cluster, RGB foreground/background, attributes, and overlay flags
/// (cursor / selection / find match) that the backend interprets
/// visually.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    /// The cell's full grapheme cluster — a base character plus any
    /// combining marks (accents, etc.) vt100 appended to it, e.g. `"a"`,
    /// `"日"`, or `"e\u{0301}"`. **Not** guaranteed to be a single
    /// `char`: use [`TerminalCell::cell_width`] rather than
    /// `text.chars().count()` to measure it. Blank / never-written cells
    /// store a single space (`" "`), matching vt100's own convention for
    /// an empty cell.
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    /// SGR 2 (faint/dim) — `vt100::Cell::dim()`. Rasterisers don't
    /// apply a separate font attribute for it (unlike bold/italic/underline):
    /// faint text has no font-weight equivalent, so
    /// [`crate::terminal_style::resolve_cell_style`] folds it into the
    /// resolved foreground colour instead, blending toward the
    /// resolved background so it reads as "faded" on both dark and
    /// light themes.
    #[serde(default)]
    pub dim: bool,
    /// Cell is part of the user's mouse selection.
    #[serde(default)]
    pub selected: bool,
    /// Cell holds the VT100 cursor position. Backends typically render
    /// this with inverted colours — except when [`cursor_shape`](Self::cursor_shape)
    /// is [`TerminalCursorShape::Underline`] or [`TerminalCursorShape::Bar`],
    /// where inverting the whole cell would look like a solid block again;
    /// see [`crate::terminal_style::resolve_cell_style`]'s doc for the
    /// shape-dependent color rule and [`crate::terminal_style::cursor_accent_rect`]
    /// for the separate accent geometry pixel-based rasterisers paint on top.
    #[serde(default)]
    pub is_cursor: bool,
    /// Shape of the VT100 cursor at this cell, from the child program's
    /// most recent DECSCUSR request (`ESC [ Ps SP q`) — see
    /// [`crate::terminal_engine::TerminalSession::cursor_shape`].
    /// Meaningful only when [`is_cursor`](Self::is_cursor)
    /// is `true`; every other cell carries the default (`Block`).
    #[serde(default)]
    pub cursor_shape: TerminalCursorShape,
    /// `true` when the child requested a *blinking* DECSCUSR variant
    /// (odd `Ps` values `1`/`3`/`5`) rather than steady (`2`/`4`/`6`).
    /// Meaningful only when [`is_cursor`](Self::is_cursor) is `true`.
    /// Pixel-based rasterisers (GTK/macOS/win, via
    /// [`crate::terminal_style::cursor_blink_visible`]) toggle the
    /// cursor's visibility on a wall-clock timer when this is set; the
    /// TUI rasteriser ignores it.
    #[serde(default)]
    pub cursor_blinking: bool,
    /// Cell is part of a non-active find match (dim highlight).
    #[serde(default)]
    pub is_find_match: bool,
    /// Cell is part of the currently-selected find match (bright highlight).
    #[serde(default)]
    pub is_find_active: bool,
}

/// Shape of the VT100 cursor, as steered by the child program's DECSCUSR
/// (`ESC [ Ps SP q`) request — see
/// [`crate::terminal_engine::TerminalSession::cursor_shape`].
///
/// Distinct from [`crate::primitives::editor::CursorShape`], even though
/// the three variants happen to share the same DECSCUSR vocabulary: a
/// shell's insert-mode bar cursor and an editor's insert-mode bar cursor
/// are unrelated primitives' concepts, so this crate keeps them as separate
/// types rather than reusing one across `Terminal` and `Editor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerminalCursorShape {
    /// Filled block — the default when the child never sends DECSCUSR.
    #[default]
    Block,
    /// Underline.
    Underline,
    /// Thin vertical bar, common in shells' insert-mode line editors
    /// (e.g. zsh/vim insert mode).
    Bar,
}

impl TerminalCell {
    /// Display width of this cell's grapheme cluster in terminal columns
    /// — `1` for ordinary text, `2` for CJK / emoji / other fullwidth
    /// glyphs. Combining marks trailing the base character add no width.
    ///
    /// Deliberately *derived* from `text` rather than cached in a
    /// separate field: `text` is the single source of truth, so there is
    /// no way for a cell's stored width to drift from its own content.
    /// Delegates to [`crate::text_util::display_width`] — the same
    /// `unicode-width`-backed table vt100 itself consults when it
    /// decides whether to reserve a spacer column for a wide character,
    /// so this always agrees with vt100's own column
    /// accounting for cells built by [`crate::terminal_engine`].
    pub fn cell_width(&self) -> u16 {
        crate::text_util::display_width(&self.text) as u16
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Ninth and last primitive on the new shape. Terminal is a uniform cell
// grid — layout here just resolves viewport dimensions to grid sizes
// and provides click-to-cell mapping. The cell contents are rendered
// directly from `cells[row][col]`; the layout method doesn't walk
// them.

/// Cell dimensions supplied by the backend. TUI passes `(1.0, 1.0)`
/// (char-cell units) for this embedded [`Terminal`] primitive's own layout;
/// native backends pass the font's advance width and line height.
///
/// Reused for a second, distinct purpose: outside this
/// primitive, [`crate::tui::backend::TuiBackend::cell_pixel_size`] carries
/// the *real* pixel size of a terminal cell — genuinely non-identity on TUI
/// — so SGR-Pixels mouse reports (`?1016h`) can be divided back into
/// fractional cell coordinates. Don't assume `(1.0, 1.0)` from this doc
/// alone when reading code that touches that field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalCellSize {
    pub width: f32,
    pub height: f32,
}

impl TerminalCellSize {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Classification of a hit-test result. For terminals every click maps
/// to a grid cell (row, col) — except clicks outside the viewport or
/// below the last rendered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalHit {
    Cell { row: u16, col: u16 },
    Empty,
}

/// Fully-resolved terminal layout. Because the cell grid is uniform,
/// there's no `visible_cells` list — painters iterate
/// `terminal.cells` directly at grid positions. The layout provides
/// the viewport → grid conversion and click hit-testing.
///
/// `viewport_width`/`viewport_height`, [`Self::hit_test`] and
/// [`Self::cell_bounds`] are LOCAL to [`Self::area`]'s origin; `area`
/// and `scrollbar` are in the frame the layout was resolved in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalLayout {
    /// The cell grid's own width — the pane minus the scrollbar gutter.
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub cell_size: TerminalCellSize,
    /// Number of rows that fit in the viewport (may differ from
    /// `terminal.cells.len()`; use `grid_rows.min(cells.len())` when
    /// iterating).
    pub grid_rows: u16,
    /// Number of columns that fit in the viewport.
    pub grid_cols: u16,
    /// The whole pane the terminal paints into, scrollbar gutter included.
    pub area: Rect,
    /// The scrollbar track, at the pane's right edge — `Some` exactly
    /// when the terminal carries a [`TerminalScrollbar`].
    pub scrollbar: Option<Rect>,
}

impl TerminalLayout {
    /// Resolve `term`'s grid for the pane `area`: the scrollbar gutter
    /// ([`TerminalScrollbar::width`], else `default_gutter`) comes off the
    /// right edge first, then the remaining width and the pane height
    /// divide into `cell_width` × `cell_height` cells.
    pub fn resolve(
        term: &Terminal,
        area: Rect,
        cell_width: f32,
        cell_height: f32,
        default_gutter: f32,
    ) -> Self {
        let gutter = match &term.scrollbar {
            Some(sb) => sb
                .width
                .map(f32::from)
                .unwrap_or(default_gutter)
                .clamp(0.0, area.width.max(0.0)),
            None => 0.0,
        };
        let cell_area_w = (area.width - gutter).max(0.0);
        let mut layout = term.layout(cell_area_w, area.height, cell_width, cell_height);
        layout.area = area;
        layout.scrollbar = term
            .scrollbar
            .map(|_| Rect::new(area.x + cell_area_w, area.y, gutter, area.height));
        layout
    }

    /// The cell under a LOCAL point, clamped onto the grid — what a
    /// selection drag that leaves the pane extends to. `None` when the
    /// grid has no cells at all.
    pub fn clamped_cell(&self, x: f32, y: f32) -> Option<(u16, u16)> {
        if self.grid_rows == 0
            || self.grid_cols == 0
            || self.cell_size.width <= 0.0
            || self.cell_size.height <= 0.0
        {
            return None;
        }
        let col = (x / self.cell_size.width).floor().max(0.0) as u32;
        let row = (y / self.cell_size.height).floor().max(0.0) as u32;
        Some((
            row.min(u32::from(self.grid_rows) - 1) as u16,
            col.min(u32::from(self.grid_cols) - 1) as u16,
        ))
    }

    /// Convert a point in the viewport to a grid cell, or `Empty` if
    /// the point is outside the rendered grid.
    pub fn hit_test(&self, x: f32, y: f32) -> TerminalHit {
        if x < 0.0
            || y < 0.0
            || x >= self.viewport_width
            || y >= self.viewport_height
            || self.cell_size.width <= 0.0
            || self.cell_size.height <= 0.0
        {
            return TerminalHit::Empty;
        }
        let col = (x / self.cell_size.width).floor() as u32;
        let row = (y / self.cell_size.height).floor() as u32;
        if row < self.grid_rows as u32 && col < self.grid_cols as u32 {
            TerminalHit::Cell {
                row: row as u16,
                col: col as u16,
            }
        } else {
            TerminalHit::Empty
        }
    }

    /// Rectangle occupied by cell `(row, col)`, or `None` if the cell
    /// is outside the grid.
    pub fn cell_bounds(&self, row: u16, col: u16) -> Option<Rect> {
        if row >= self.grid_rows || col >= self.grid_cols {
            return None;
        }
        Some(Rect::new(
            col as f32 * self.cell_size.width,
            row as f32 * self.cell_size.height,
            self.cell_size.width,
            self.cell_size.height,
        ))
    }
}

impl Terminal {
    /// Compare `self` against the previously-painted `prev` snapshot and
    /// return the indices of rows whose cell content differs.
    ///
    /// Returns `None` when a full repaint is required rather than a
    /// row-by-row diff: the two grids have different row counts, so
    /// `prev`'s row `i` doesn't necessarily correspond to `self`'s row
    /// `i` (a resize/reflow shifted everything). Returns `Some(rows)`
    /// otherwise — sorted ascending, deduplicated, and possibly empty
    /// (e.g. a cursor-blink tick that produced an identical grid).
    ///
    /// `TerminalCell` compares by value, so any change to a
    /// cell's glyph, colour, or overlay flag (`selected` / `is_cursor` /
    /// `is_find_match` / `is_find_active`) — i.e. scroll, selection,
    /// cursor move, and find-highlight updates — registers the owning
    /// row as dirty; there is nothing overlay-specific for a caller to
    /// special-case.
    pub fn dirty_rows(&self, prev: &Terminal) -> Option<Vec<usize>> {
        if self.cells.len() != prev.cells.len() {
            return None;
        }
        Some(
            self.cells
                .iter()
                .zip(prev.cells.iter())
                .enumerate()
                .filter_map(|(i, (a, b))| (a != b).then_some(i))
                .collect(),
        )
    }

    /// Compute viewport → grid conversion. The layout is uniform-cell,
    /// so this is just division; there's no per-cell work.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — pane dimensions.
    /// - `cell_width`, `cell_height` — cell dimensions. TUI passes
    ///   `(1.0, 1.0)`; native backends pass the font's advance width
    ///   and line height.
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> TerminalLayout {
        let grid_cols = if cell_width > 0.0 {
            (viewport_width / cell_width).floor().max(0.0) as u16
        } else {
            0
        };
        let grid_rows = if cell_height > 0.0 {
            (viewport_height / cell_height).floor().max(0.0) as u16
        } else {
            0
        };
        TerminalLayout {
            viewport_width,
            viewport_height,
            cell_size: TerminalCellSize::new(cell_width, cell_height),
            grid_rows,
            grid_cols,
            area: Rect::new(0.0, 0.0, viewport_width, viewport_height),
            scrollbar: None,
        }
    }
}

// ── Split-pane layout ───────────────────────────────────────────────────────

/// Layout geometry for a side-by-side terminal split. The divider
/// occupies one `cell_width` column between the two panes. An optional
/// scrollbar strip is reserved at the right edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalSplitLayout {
    /// Left pane region.
    pub left: Rect,
    /// Right pane region.
    pub right: Rect,
    /// X-coordinate of the divider column's left edge.
    pub divider_x: f32,
    /// Width of the divider column (one `cell_width`).
    pub divider_width: f32,
    /// Cell dimensions used for coordinate-to-cell conversion.
    pub cell_size: TerminalCellSize,
    /// Scrollbar strip bounds (right edge of area). `None` when
    /// `scrollbar_width` was 0.
    pub scrollbar: Option<Rect>,
}

impl TerminalSplitLayout {
    /// Compute a two-pane split layout from a containing area, left
    /// pane column count, cell dimensions, and optional scrollbar width.
    ///
    /// TUI callers pass `cell_width = 1.0, cell_height = 1.0`;
    /// GTK callers pass the font's advance width and line height.
    /// Pass `scrollbar_width = 0.0` to omit the scrollbar zone.
    pub fn new(
        area: Rect,
        left_cols: usize,
        cell_width: f32,
        cell_height: f32,
        scrollbar_width: f32,
    ) -> Self {
        let sb_w = if scrollbar_width > 0.0 {
            scrollbar_width.min(area.width)
        } else {
            0.0
        };
        let usable_w = (area.width - sb_w).max(0.0);
        let left_w = (left_cols as f32 * cell_width).min(usable_w);
        let divider_x = area.x + left_w;
        let divider_w = cell_width.min((usable_w - left_w).max(0.0));
        let right_x = divider_x + divider_w;
        let right_w = (usable_w - left_w - divider_w).max(0.0);
        let scrollbar = if sb_w > 0.0 {
            Some(Rect::new(area.x + usable_w, area.y, sb_w, area.height))
        } else {
            None
        };
        Self {
            left: Rect::new(area.x, area.y, left_w, area.height),
            right: Rect::new(right_x, area.y, right_w, area.height),
            divider_x,
            divider_width: divider_w,
            cell_size: TerminalCellSize::new(cell_width, cell_height),
            scrollbar,
        }
    }

    /// Resolve an absolute point to a split zone with pane-relative cell
    /// coordinates. Converts pixel/cell positions to `(col, row)` within
    /// the hit pane using the stored `cell_size`.
    pub fn hit_test(&self, x: f32, y: f32) -> TerminalSplitHit {
        if y < self.left.y || y >= self.left.y + self.left.height {
            return TerminalSplitHit::Outside;
        }
        if let Some(sb) = self.scrollbar {
            if x >= sb.x && x < sb.x + sb.width {
                return TerminalSplitHit::Scrollbar;
            }
        }
        if x >= self.left.x && x < self.left.x + self.left.width {
            let col = ((x - self.left.x) / self.cell_size.width).floor() as u16;
            let row = ((y - self.left.y) / self.cell_size.height).floor() as u16;
            TerminalSplitHit::LeftPane { col, row }
        } else if x >= self.divider_x && x < self.divider_x + self.divider_width {
            TerminalSplitHit::Divider
        } else if x >= self.right.x && x < self.right.x + self.right.width {
            let col = ((x - self.right.x) / self.cell_size.width).floor() as u16;
            let row = ((y - self.right.y) / self.cell_size.height).floor() as u16;
            TerminalSplitHit::RightPane { col, row }
        } else {
            TerminalSplitHit::Outside
        }
    }
}

/// Result of [`TerminalSplitLayout::hit_test`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSplitHit {
    LeftPane { col: u16, row: u16 },
    RightPane { col: u16, row: u16 },
    Divider,
    Scrollbar,
    Outside,
}

/// Events a `Terminal` emits back to the app, as
/// [`crate::UiEvent::Terminal`]. Its [`crate::interactive::Interactive`]
/// impl produces the selection and scroll variants; the app applies them
/// to its own session (e.g. [`TerminalEvent::Scroll`] through the
/// engine's `handle_wheel`, which decides between forwarding the wheel to
/// the child and scrolling local history).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    /// A key was pressed with the terminal focused, for a host that
    /// reports keys per terminal rather than as plain
    /// [`crate::UiEvent::KeyPressed`]. The widget itself never emits it:
    /// keys bubble to the host unconsumed.
    KeyPressed { key: String, modifiers: Modifiers },
    /// User started selecting at `(row, col)` (0-based, content area).
    SelectStart { row: u16, col: u16 },
    /// User dragged the selection to a new endpoint.
    SelectExtend { row: u16, col: u16 },
    /// User released the mouse button — selection is finalised.
    SelectEnd,
    /// Mouse wheel scroll: positive = scroll content downward (toward
    /// live), negative = scroll backward into history.
    Scroll { delta: i32 },
}

impl crate::interactive::WidgetAction for TerminalEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Terminal(id, self)
    }
}

// ── Widget contract ─────────────────────────────────────────────────────────

/// A [`Terminal`]'s per-widget interaction state: the cell the live
/// selection drag last reported, so a pointer move inside the same cell
/// emits nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalState {
    /// `(row, col)` of the drag's current endpoint, or `None` when no
    /// selection drag is in progress.
    pub drag: Option<(u16, u16)>,
}

impl Terminal {
    /// `(cell_width, cell_height, default_gutter)` for `ctx`'s units: one
    /// cell each on a cell grid; the editor font's advance and line height
    /// plus the [`crate::Style::scrollbar_width`] token on a pixel surface.
    fn cell_metrics(ctx: &crate::primitive::LayoutCtx) -> (f32, f32, f32) {
        match ctx.units() {
            crate::primitive::Units::Cells => (1.0, 1.0, 1.0),
            _ => (
                ctx.char_width,
                ctx.editor_line_height(),
                ctx.style().scrollbar_width,
            ),
        }
    }
}

impl crate::primitive::Primitive for Terminal {
    type Layout = TerminalLayout;
    type Event = TerminalHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Terminal cells render the user's content, so they paint in the
    /// editor font.
    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Editor
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Terminal(self.clone())
    }

    /// A terminal fills whatever pane it is given — the host sizes the
    /// PTY from that pane, so sizing the pane from the snapshot's own grid
    /// would pin it at its first size.
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::NONE
    }

    /// `rect` is the whole pane, scrollbar gutter included — see
    /// [`TerminalLayout::resolve`] and [`Self::cell_metrics`].
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TerminalLayout {
        let (cell_w, cell_h, gutter) = Self::cell_metrics(ctx);
        TerminalLayout::resolve(self, rect, cell_w, cell_h, gutter)
    }

    /// Clears the pane to [`crate::Theme::background`], paints the cell
    /// grid in the surface's editor font, then the scrollbar in the gutter.
    fn paint(
        &self,
        layout: &TerminalLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_pane(self, layout, surface, theme);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// [`cell_paint::paint`] over `buf`, through the crate's
    /// [`crate::CellSurface`] bridge.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TerminalLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(self, layout, cells, theme);
    }
}

impl crate::interactive::Interactive for Terminal {
    type State = TerminalState;
    type Action = TerminalEvent;

    /// One focus stop: a focused terminal is where the host sends keys.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// - **Left press** takes focus. On a grid cell it also captures the
    ///   pointer and starts a selection there
    ///   ([`TerminalEvent::SelectStart`]); on the scrollbar gutter or past
    ///   the last row it only focuses.
    /// - **Move** while selecting extends the selection to the cell under
    ///   the pointer, clamped onto the grid, once per cell
    ///   ([`TerminalEvent::SelectExtend`]).
    /// - **Left release** ends the selection ([`TerminalEvent::SelectEnd`])
    ///   and releases the capture.
    /// - **Wheel** becomes [`TerminalEvent::Scroll`], one row per notch,
    ///   negative into history.
    /// - **Actions**: `Focus`/`Click` take focus; `ScrollUp`/`ScrollDown`
    ///   scroll one row, the same events a wheel notch produces.
    ///
    /// Keys and committed text are left unconsumed so they reach the host.
    fn handle(
        &self,
        state: &mut TerminalState,
        event: &crate::interactive::RoutedEvent,
        layout: &TerminalLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<TerminalEvent> {
        use crate::a11y::A11yAction;
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                cx.request_focus(None);
                match layout.hit_test(local.x, local.y) {
                    TerminalHit::Cell { row, col } => {
                        cx.capture_pointer();
                        state.drag = Some((row, col));
                        Handled::Event(TerminalEvent::SelectStart { row, col })
                    }
                    TerminalHit::Empty => Handled::Consumed,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => {
                let Some(prev) = state.drag else {
                    return Handled::Ignored;
                };
                match layout.clamped_cell(local.x, local.y) {
                    Some((row, col)) if (row, col) != prev => {
                        state.drag = Some((row, col));
                        Handled::Event(TerminalEvent::SelectExtend { row, col })
                    }
                    _ => Handled::Consumed,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::MouseButton::Left),
                ..
            } => {
                if state.drag.take().is_some() {
                    cx.release_pointer();
                    Handled::Event(TerminalEvent::SelectEnd)
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Wheel { dy, .. } => {
                match crate::primitives::scrollbar::wheel_row_delta(*dy, 1.0) {
                    0 => Handled::Ignored,
                    delta => Handled::Event(TerminalEvent::Scroll {
                        delta: delta as i32,
                    }),
                }
            }
            RoutedEvent::Action(A11yAction::Focus | A11yAction::Click) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::Action(A11yAction::ScrollUp) => {
                Handled::Event(TerminalEvent::Scroll { delta: -1 })
            }
            RoutedEvent::Action(A11yAction::ScrollDown) => {
                Handled::Event(TerminalEvent::Scroll { delta: 1 })
            }
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &TerminalState) -> Vec<crate::a11y::A11yAction> {
        use crate::a11y::A11yAction;
        vec![
            A11yAction::Focus,
            A11yAction::ScrollUp,
            A11yAction::ScrollDown,
        ]
    }
}

/// The vertical divider between two side-by-side terminal panes — a
/// one-unit line down the column [`TerminalSplitLayout::divider_x`]
/// names, in [`crate::Theme::separator`]. A themed `│` column on a cell
/// grid; a hairline from [`crate::terminal_style::divider_geometry`] on a
/// pixel surface.
///
/// Its own widget rather than a flag on [`Terminal`]: the divider belongs
/// to the split, not to either pane, and paints where neither pane's
/// rect reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDivider {
    pub id: WidgetId,
}

impl TerminalDivider {
    pub fn new(id: WidgetId) -> Self {
        Self { id }
    }
}

/// The column a [`TerminalDivider`] paints: `bounds.x` is the divider's
/// column, `bounds.y` its top and `bounds.height` its length. The line is
/// always one unit wide whatever `bounds.width` is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalDividerLayout {
    pub bounds: Rect,
}

impl crate::a11y::A11yPrimitive for TerminalDivider {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new().with_role(crate::a11y::A11yRole::Presentation)
    }
}

impl crate::primitive::Primitive for TerminalDivider {
    type Layout = TerminalDividerLayout;
    type Event = crate::interactive::Never;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// One unit wide, stretching vertically.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let width = match ctx.units() {
            crate::primitive::Units::Cells => 1.0,
            _ => crate::terminal_style::divider_geometry(0.0, 0.0, 0.0).width as f32,
        };
        crate::NaturalSize::new(Some(width), None)
    }

    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TerminalDividerLayout {
        TerminalDividerLayout { bounds: rect }
    }

    fn paint(
        &self,
        layout: &TerminalDividerLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_divider(
            surface,
            layout.bounds.x,
            layout.bounds.y,
            layout.bounds.height,
            theme,
        );
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TerminalDividerLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint_divider(layout, cells, theme);
    }
}

crate::non_interactive!(TerminalDivider);

/// The cell-grid painters for [`Terminal`] and [`TerminalDivider`],
/// written against [`crate::CellSurface`].
///
/// Each cell's colours come from
/// [`crate::terminal_style::resolve_cell_style`] — the overlay ladder the
/// pixel painter shares: cursor inverts fg/bg, selection uses
/// `theme.selection_bg`, find-match and find-active use the highlight
/// colours on [`crate::Theme`].
///
/// # Cursor shape (best-effort)
///
/// [`TerminalCursorShape::Block`] is painted entirely by
/// `resolve_cell_style`'s colour invert. `Underline`/`Bar` don't invert,
/// so the cell is underlined in its own foreground when
/// [`crate::terminal_style::cursor_accent_visible`] says the accent shows.
/// A terminal cell has no sub-cell geometry, so **`Bar` renders
/// identically to `Underline`** here; the pixel painter draws a true thin
/// bar.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Terminal, TerminalDividerLayout, TerminalLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::terminal_style::{cursor_accent_visible, resolve_cell_style};
    use crate::theme::Theme;

    /// Paint `term`'s cell grid into `layout.area`, then its scrollbar in
    /// the gutter [`TerminalLayout::scrollbar`] reserved.
    pub(crate) fn paint(
        term: &Terminal,
        layout: &TerminalLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let area = layout.area;
        let x0 = area.x.round() as u16;
        let y0 = area.y.round() as u16;
        let height = area.height.round() as u16;
        if area.width.round() as u16 == 0 || height == 0 {
            return;
        }
        let cell_area_w = layout.viewport_width.round() as u16;

        for (row_idx, row) in term.cells.iter().enumerate() {
            if row_idx as u16 >= height {
                break;
            }
            let y = y0 + row_idx as u16;
            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx as u16 >= cell_area_w {
                    break;
                }
                let (bg, fg) = resolve_cell_style(cell, theme);
                let style = CellStyle {
                    fg: Some(fg),
                    bg: Some(bg),
                    bold: cell.bold,
                    italic: cell.italic,
                    underline: (cell.underline || cursor_accent_visible(cell)).then_some(fg),
                    ..CellStyle::default()
                };
                surface.put_grapheme(x0 + col_idx as u16, y, &cell.text, style);
            }
        }

        if let (Some(track), Some(sb)) = (layout.scrollbar, term.scrollbar) {
            let bar = sb.bar(term.id, track, 1.0);
            let bar_layout = crate::primitives::scrollbar::ScrollbarLayout::resolve(&bar, track);
            crate::primitives::scrollbar::cell_paint::paint(
                &bar_layout,
                surface,
                theme,
                theme.background,
            );
        }
    }

    /// Paint a `│` column down `layout.bounds.x` in `theme.separator` on
    /// `theme.background`.
    pub(crate) fn paint_divider(
        layout: &TerminalDividerLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let style = crate::cell_surface::solid_style(theme.separator, theme.background);
        let x = layout.bounds.x.round() as u16;
        let y0 = layout.bounds.y.round() as u16;
        for dy in 0..layout.bounds.height.round() as u16 {
            surface.put_char(x, y0 + dy, '│', style);
        }
    }
}

// ── PaintSurface paint ────────────────────────────────────────────
//
// The one pixel implementation of terminal painting, written against
// [`crate::paint_surface::PaintSurface`]. Per-cell glyph styling
// (`TerminalCell::bold`/`italic`/`underline`) and the wide-glyph
// horizontal scale go through
// [`crate::paint_surface::PaintSurface::surface_draw_text_run_styled`];
// see that method's doc for which flags each backend applies.
//
// `paint`'s `dirty_rows` parameter paints only the listed rows, for a
// caller that keeps its own frame-to-frame repaint cache.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{Terminal, TerminalLayout};
    use crate::paint_surface::PaintSurface;
    use crate::terminal_style::{
        cursor_accent_rect, cursor_accent_visible, divider_geometry, resolve_cell_style,
        wide_cell_advance, wide_glyph_x_scale,
    };
    use crate::theme::Theme;
    use crate::Rect;

    /// The whole pane `layout` resolved: clear it to `theme.background`
    /// (so no pixel the grid leaves uncovered keeps a previous frame's
    /// glyphs), paint the cell grid in the surface's editor font, then
    /// the scrollbar in the gutter through the chrome surface.
    pub(crate) fn paint_pane(
        term: &Terminal,
        layout: &TerminalLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        surface.surface_fill_rect(layout.area, theme.background);
        let mut grid = |s: &mut dyn PaintSurface| {
            paint(
                term,
                s,
                theme,
                layout.area.x,
                layout.area.y,
                layout.viewport_width,
                layout.viewport_height,
                layout.cell_size.height,
                layout.cell_size.width,
                None,
            );
        };
        if !surface.surface_with_editor_font(&mut grid) {
            grid(surface);
        }
        if let (Some(track), Some(sb)) = (layout.scrollbar, term.scrollbar) {
            let bar = sb.bar(term.id, track, layout.cell_size.height);
            crate::primitives::scrollbar::native_surface_paint::paint(&bar, surface, theme);
        }
    }

    /// Paint `term`'s cell grid into the rectangular region starting at
    /// `(x, y)` on `surface`, without clearing around it.
    ///
    /// `cell_area_w`/`cell_area_h` clip painting to the pane's own
    /// bounds — cells past the right edge, or rows whose top has reached
    /// the bottom, stop being drawn rather than bleeding past the pane.
    /// `line_height`/`char_width` are the per-cell
    /// dimensions in surface-native units.
    ///
    /// `dirty_rows`: `None` paints every row; `Some(rows)` paints only
    /// the rows whose index appears in `rows` (sorted ascending) and
    /// leaves every other row's pixels untouched.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        term: &Terminal,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        x: f32,
        y: f32,
        cell_area_w: f32,
        cell_area_h: f32,
        line_height: f32,
        char_width: f32,
        dirty_rows: Option<&[usize]>,
    ) {
        for (row_idx, row) in term.cells.iter().enumerate() {
            let row_y = y + row_idx as f32 * line_height;
            // Stop once a row's top has reached the pane bottom — such a
            // row belongs to a taller (pre-resize) grid and would bleed
            // past the pane. A row that merely straddles the bottom edge
            // is still drawn (and clipped by whatever the host paints
            // over it).
            if row_y >= y + cell_area_h {
                break;
            }
            if let Some(rows) = dirty_rows {
                if rows.binary_search(&row_idx).is_err() {
                    continue;
                }
            }
            let mut cell_x = x;
            let mut col = 0usize;
            while col < row.len() {
                let cell = &row[col];
                // Double-width glyphs (CJK, emoji, ...) get a
                // two-column cell: the vt100 grid already reserves the
                // following column as an empty continuation
                // placeholder, so claim it here rather than letting it
                // paint its own (mismatched) background over the
                // glyph's right half.
                let (cell_w, cols_advanced) = wide_cell_advance(&cell.text, char_width as f64);
                let cell_w = cell_w as f32;
                let is_wide = cols_advanced == 2;

                if cell_x + cell_w > x + cell_area_w {
                    break;
                }
                let (cell_bg, cell_fg) = resolve_cell_style(cell, theme);
                surface.surface_fill_rect(Rect::new(cell_x, row_y, cell_w, line_height), cell_bg);

                if cell.text != " " && cell.text != "\0" && !cell.text.is_empty() {
                    let s = cell.text.as_str();
                    let cell_rect = Rect::new(cell_x, row_y, cell_w, line_height);
                    let scale_x = if is_wide {
                        // The font each backend falls back to for CJK/emoji rarely lays the
                        // glyph out at exactly two cells — scale it to fill `cell_w` instead of
                        // leaving a ragged gap or overlap.
                        let (natural_w, _) = surface.surface_measure_text(s);
                        wide_glyph_x_scale(natural_w as f64, cell_w as f64) as f32
                    } else {
                        1.0
                    };
                    surface.surface_draw_text_run_styled(
                        cell_rect,
                        s,
                        cell_fg,
                        cell.bold,
                        cell.italic,
                        cell.underline,
                        scale_x,
                    );
                }

                // Underline/Bar cursor accent — `Block`
                // is already fully painted above via `resolve_cell_style`'s
                // colour invert, so `cursor_accent_rect` returns `None`
                // for it and this is a no-op. `cell.fg` (the cell's own,
                // un-inverted foreground) is the accent colour, mirroring
                // how the `Block` invert uses it as the new background.
                if cursor_accent_visible(cell) {
                    if let Some(accent) =
                        cursor_accent_rect(cell.cursor_shape, cell_x, row_y, cell_w, line_height)
                    {
                        surface.surface_fill_rect(accent, cell.fg);
                    }
                }

                cell_x += cell_w;
                col += cols_advanced;
            }
        }
    }

    /// Draw a vertical divider line for a terminal split pane. Paints a
    /// 1-unit-wide line at `x` from `y` to `y + height` using
    /// `theme.separator`. Geometry comes from
    /// [`crate::terminal_style::divider_geometry`].
    pub(crate) fn paint_divider(
        surface: &mut dyn PaintSurface,
        x: f32,
        y: f32,
        height: f32,
        theme: &Theme,
    ) {
        let g = divider_geometry(x as f64, y as f64, height as f64);
        surface.surface_fill_rect(
            Rect::new(g.x as f32, g.y as f32, g.width as f32, g.height as f32),
            theme.separator,
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::terminal::{TerminalCell, TerminalCursorShape};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::chart`/`primitives::find_replace`/
        /// `primitives::text_display`'s own `RecordingSurface`
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            styled_runs: Vec<(Rect, String, Color, bool, bool, bool, f32)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                20.0
            }
            fn surface_char_width(&self) -> f32 {
                10.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 10.0, 20.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// Test-only recorder — `paint` never calls this verb
            /// (see this module's own doc for why no primitive here has been
            /// migrated onto it yet); this exists only so `RecordingSurface`
            /// satisfies the trait. Records into the same `fills` list as
            /// `surface_fill_rect` (radius dropped) — no test asserts on it
            /// today.
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
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_text_run_styled(
                &mut self,
                rect: Rect,
                text: &str,
                color: Color,
                bold: bool,
                italic: bool,
                underline: bool,
                scale_x: f32,
            ) {
                self.styled_runs.push((
                    rect,
                    text.to_string(),
                    color,
                    bold,
                    italic,
                    underline,
                    scale_x,
                ));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
            TerminalCell {
                text: ch.to_string(),
                fg,
                bg,
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
            }
        }

        #[test]
        fn cell_background_and_glyph_paint_at_the_grid_position() {
            let fg = Color::rgb(255, 255, 255);
            let bg = Color::rgb(10, 20, 30);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![cell('A', fg, bg)]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|&(r, c)| r == Rect::new(0.0, 0.0, 10.0, 20.0) && c == bg),
                "expected the cell's own background fill, fills were {:?}",
                surface.fills,
            );
            assert!(
                surface
                    .styled_runs
                    .iter()
                    .any(|(r, t, c, ..)| t == "A" && *c == fg && r.x == 0.0 && r.y == 0.0),
                "expected the glyph painted in the cell's fg colour, got {:?}",
                surface.styled_runs,
            );
        }

        #[test]
        fn blank_and_nul_cells_paint_background_only() {
            let bg = Color::rgb(10, 20, 30);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![
                    cell(' ', Color::rgb(0, 0, 0), bg),
                    cell('\0', Color::rgb(0, 0, 0), bg),
                ]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            assert_eq!(
                surface.fills.len(),
                2,
                "both cells should still fill their background"
            );
            assert!(
                surface.styled_runs.is_empty(),
                "space and NUL cells must not paint a glyph, got {:?}",
                surface.styled_runs,
            );
        }

        #[test]
        fn bold_italic_underline_flags_reach_the_styled_run() {
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![TerminalCell {
                    bold: true,
                    italic: true,
                    underline: true,
                    ..cell('X', Color::rgb(255, 255, 255), Color::rgb(0, 0, 0))
                }]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            let (_, _, _, bold, italic, underline, _) = surface.styled_runs[0];
            assert!(bold && italic && underline);
        }

        /// A `dim` cell reaches the styled run with its foreground already
        /// blended toward the background — `paint` sources `cell_fg` from
        /// [`crate::terminal_style::resolve_cell_style`], the single place
        /// SGR 2 (faint) is applied, so every pixel backend
        /// (GTK/macOS/win, all routed through this shared `paint`) gets the
        /// dimmed colour for free with no per-backend styling flag.
        #[test]
        fn dim_flag_dims_the_color_reaching_the_styled_run() {
            let fg = Color::rgb(200, 200, 200);
            let bg = Color::rgb(0, 0, 0);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![TerminalCell {
                    dim: true,
                    ..cell('X', fg, bg)
                }]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            let (_, _, color, _, _, _, _) = surface.styled_runs[0];
            assert_eq!(
                color,
                Color::rgb(100, 100, 100),
                "dim cell's glyph color must be blended 50% toward its background"
            );
        }

        /// A `Bar`/`Underline` cursor cell paints its
        /// normal (uninverted) background fill *plus* a second, smaller
        /// accent fill in the cell's own foreground colour — unlike
        /// `Block`, which paints only the (inverted) background fill
        /// with no accent. This is the shared logic every pixel backend
        /// (GTK/macOS/win) gets for free through this one `paint` fn.
        #[test]
        fn cursor_shape_accent_paints_a_second_smaller_fill() {
            let fg = Color::rgb(200, 30, 30);
            let bg = Color::rgb(10, 10, 10);

            for shape in [TerminalCursorShape::Underline, TerminalCursorShape::Bar] {
                let term = Terminal {
                    id: WidgetId::new("term"),
                    cells: vec![vec![TerminalCell {
                        is_cursor: true,
                        cursor_shape: shape,
                        ..cell(' ', fg, bg)
                    }]],
                    scrollbar: None,
                };
                let theme = Theme::default();
                let mut surface = RecordingSurface::default();

                paint(
                    &term,
                    &mut surface,
                    &theme,
                    0.0,
                    0.0,
                    200.0,
                    100.0,
                    20.0,
                    10.0,
                    None,
                );

                // Exactly two fills: the normal (uninverted) cell
                // background, plus the accent stripe in the cell's fg.
                assert_eq!(
                    surface.fills.len(),
                    2,
                    "{shape:?}: expected a background fill plus one accent fill, got {:?}",
                    surface.fills,
                );
                assert!(
                    surface
                        .fills
                        .iter()
                        .any(|&(r, c)| c == bg && r.width == 10.0 && r.height == 20.0),
                    "{shape:?}: the full 10x20 cell background should still be the cell's own \
                     (uninverted) bg, got {:?}",
                    surface.fills,
                );
                assert!(
                    surface
                        .fills
                        .iter()
                        .any(|&(r, c)| c == fg && (r.width < 10.0 || r.height < 20.0)),
                    "{shape:?}: expected a smaller accent fill in the cell's fg colour, got {:?}",
                    surface.fills,
                );
            }
        }

        /// A `Block` cursor cell paints only the (inverted) background
        /// fill — no second accent fill, unlike `Underline`/`Bar` above.
        #[test]
        fn block_cursor_shape_paints_no_accent_fill() {
            let fg = Color::rgb(200, 30, 30);
            let bg = Color::rgb(10, 10, 10);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![TerminalCell {
                    is_cursor: true,
                    ..cell(' ', fg, bg)
                }]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            assert_eq!(
                surface.fills.len(),
                1,
                "Block cursor should paint exactly one (inverted) background fill, got {:?}",
                surface.fills,
            );
        }

        #[test]
        fn dirty_rows_filter_skips_untouched_rows() {
            let painted = Color::rgb(200, 200, 200);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![
                    vec![cell('A', painted, painted)],
                    vec![cell('B', painted, painted)],
                ],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                Some(&[1]),
            );

            assert!(
                surface.fills.iter().all(|(r, _)| r.y == 20.0),
                "only row 1 (y=20) should have painted anything, fills were {:?}",
                surface.fills,
            );
        }

        #[test]
        fn paint_divider_fills_the_separator_geometry() {
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint_divider(&mut surface, 10.0, 5.0, 40.0, &theme);

            assert!(
                surface.fills.iter().any(|&(_, c)| c == theme.separator),
                "expected a fill in theme.separator, fills were {:?}",
                surface.fills,
            );
        }

        // ── Grapheme clusters ───────────────────────────────────────────

        /// A combining mark appended to a base character (e.g. `e` +
        /// U+0301 COMBINING ACUTE ACCENT) must reach the rasteriser as one
        /// glyph run, not just the base character — dropping the mark would be
        /// a defect.
        #[test]
        fn combining_mark_grapheme_paints_as_one_glyph_run() {
            let fg = Color::rgb(255, 255, 255);
            let bg = Color::rgb(0, 0, 0);
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![vec![TerminalCell {
                    text: "e\u{0301}".to_string(),
                    ..cell('e', fg, bg)
                }]],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            assert!(
                surface
                    .styled_runs
                    .iter()
                    .any(|(_, t, ..)| t == "e\u{0301}"),
                "expected the full grapheme cluster painted as one run, got {:?}",
                surface.styled_runs,
            );
        }

        /// A combining-mark grapheme is still a single (narrow) column —
        /// the trailing mark must not be double-counted as a second
        /// character's worth of width.
        #[test]
        fn combining_mark_grapheme_advances_a_single_column() {
            let fg = Color::rgb(255, 255, 255);
            let bg = Color::rgb(0, 0, 0);
            let row = vec![
                TerminalCell {
                    text: "e\u{0301}".to_string(),
                    ..cell('e', fg, bg)
                },
                cell('B', fg, Color::rgb(9, 9, 9)),
            ];
            let term = Terminal {
                id: WidgetId::new("term"),
                cells: vec![row],
                scrollbar: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &term,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                100.0,
                20.0,
                10.0,
                None,
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|&(r, c)| r.x == 10.0 && c == Color::rgb(9, 9, 9)),
                "second cell should start at x=10 (one column over), fills were {:?}",
                surface.fills,
            );
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(unused_imports)]
pub(crate) use native_surface_paint::{paint, paint_divider};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_layout_basic() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 0.0);
        assert_eq!(sl.left, Rect::new(0.0, 0.0, 40.0, 24.0));
        assert_eq!(sl.divider_x, 40.0);
        assert_eq!(sl.divider_width, 1.0);
        assert_eq!(sl.right, Rect::new(41.0, 0.0, 39.0, 24.0));
        assert!(sl.scrollbar.is_none());
    }

    #[test]
    fn split_layout_pixel_units() {
        let area = Rect::new(10.0, 5.0, 800.0, 600.0);
        let sl = TerminalSplitLayout::new(area, 40, 8.0, 16.0, 0.0);
        assert_eq!(sl.left, Rect::new(10.0, 5.0, 320.0, 600.0));
        assert_eq!(sl.divider_x, 330.0);
        assert_eq!(sl.divider_width, 8.0);
        assert_eq!(sl.right, Rect::new(338.0, 5.0, 472.0, 600.0));
    }

    #[test]
    fn split_layout_left_cols_exceed_area() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let sl = TerminalSplitLayout::new(area, 30, 1.0, 1.0, 0.0);
        assert_eq!(sl.left.width, 20.0);
        assert_eq!(sl.divider_width, 0.0);
        assert_eq!(sl.right.width, 0.0);
    }

    #[test]
    fn split_layout_zero_left_cols() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 0, 1.0, 1.0, 0.0);
        assert_eq!(sl.left.width, 0.0);
        assert_eq!(sl.divider_x, 0.0);
        assert_eq!(sl.divider_width, 1.0);
        assert_eq!(sl.right.width, 79.0);
    }

    #[test]
    fn split_hit_test_cell_coords() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 0.0);
        assert_eq!(
            sl.hit_test(10.0, 5.0),
            TerminalSplitHit::LeftPane { col: 10, row: 5 }
        );
        assert_eq!(sl.hit_test(40.0, 5.0), TerminalSplitHit::Divider);
        assert_eq!(
            sl.hit_test(50.0, 5.0),
            TerminalSplitHit::RightPane { col: 9, row: 5 }
        );
        assert_eq!(sl.hit_test(50.0, 30.0), TerminalSplitHit::Outside);
        assert_eq!(sl.hit_test(-1.0, 5.0), TerminalSplitHit::Outside);
    }

    #[test]
    fn split_hit_test_pixel_units() {
        let area = Rect::new(10.0, 5.0, 800.0, 600.0);
        let sl = TerminalSplitLayout::new(area, 40, 8.0, 16.0, 0.0);
        // Click at x=26 (offset 16 from left=10) → col 2
        assert_eq!(
            sl.hit_test(26.0, 21.0),
            TerminalSplitHit::LeftPane { col: 2, row: 1 }
        );
        // Right pane starts at 338.0; click at 354.0 → offset 16 → col 2
        assert_eq!(
            sl.hit_test(354.0, 37.0),
            TerminalSplitHit::RightPane { col: 2, row: 2 }
        );
    }

    #[test]
    fn split_layout_with_scrollbar() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 1.0);
        // Scrollbar takes 1 cell from the right edge.
        let sb = sl.scrollbar.unwrap();
        assert_eq!(sb.x, 79.0);
        assert_eq!(sb.width, 1.0);
        // Right pane is narrower by 1.
        assert_eq!(sl.right.width, 38.0);
    }

    #[test]
    fn split_hit_test_scrollbar() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 1.0);
        assert_eq!(sl.hit_test(79.0, 5.0), TerminalSplitHit::Scrollbar);
        // Just inside right pane (not scrollbar).
        assert_eq!(
            sl.hit_test(78.0, 5.0),
            TerminalSplitHit::RightPane { col: 37, row: 5 }
        );
    }

    #[test]
    fn effective_scroll_offset_non_inverted() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 100,
            inverted: false,
            width: None,
        };
        assert_eq!(sb.effective_scroll_offset(), 100);
    }

    #[test]
    fn effective_scroll_offset_inverted_at_bottom() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        // offset 0 = live view → thumb at bottom → effective = max
        assert_eq!(sb.effective_scroll_offset(), 476);
    }

    #[test]
    fn effective_scroll_offset_inverted_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 476,
            inverted: true,
            width: None,
        };
        // fully scrolled into history → thumb at top → effective = 0
        assert_eq!(sb.effective_scroll_offset(), 0);
    }

    #[test]
    fn effective_scroll_offset_inverted_midpoint() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 200,
            inverted: true,
            width: None,
        };
        assert_eq!(sb.effective_scroll_offset(), 276);
    }

    #[test]
    fn effective_scroll_offset_inverted_no_overflow() {
        let sb = TerminalScrollbar {
            total_lines: 24,
            visible_lines: 24,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        // total == visible → no scrollable range
        assert_eq!(sb.effective_scroll_offset(), 0);
    }

    // ── dirty_rows ───────────────────────────────────────────────────

    fn cell(ch: char) -> TerminalCell {
        TerminalCell {
            text: ch.to_string(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
        }
    }

    // ── TerminalCell::cell_width ────────────────────────────────────────

    #[test]
    fn cell_width_narrow_text_is_one() {
        assert_eq!(cell('a').cell_width(), 1);
        assert_eq!(cell(' ').cell_width(), 1);
    }

    #[test]
    fn cell_width_wide_cjk_is_two() {
        assert_eq!(cell('日').cell_width(), 2);
        assert_eq!(cell('中').cell_width(), 2);
    }

    #[test]
    fn cell_width_combining_mark_does_not_add_width() {
        let c = TerminalCell {
            text: "e\u{0301}".to_string(),
            ..cell('e')
        };
        assert_eq!(
            c.cell_width(),
            1,
            "base char + combining accent is still one column"
        );
    }

    fn grid(rows: &[&str]) -> Terminal {
        Terminal {
            id: WidgetId::new("term"),
            cells: rows
                .iter()
                .map(|row| row.chars().map(cell).collect())
                .collect(),
            scrollbar: None,
        }
    }

    #[test]
    fn dirty_rows_identical_grids_are_clean() {
        let prev = grid(&["abc", "def"]);
        let next = grid(&["abc", "def"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![]));
    }

    #[test]
    fn dirty_rows_flags_only_changed_rows() {
        let prev = grid(&["abc", "def", "ghi"]);
        let next = grid(&["abc", "XYZ", "ghi"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![1]));
    }

    #[test]
    fn dirty_rows_flags_multiple_changed_rows() {
        let prev = grid(&["abc", "def", "ghi"]);
        let next = grid(&["AAA", "def", "GGG"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![0, 2]));
    }

    #[test]
    fn dirty_rows_none_when_row_count_differs() {
        let prev = grid(&["abc", "def"]);
        let next = grid(&["abc", "def", "ghi"]);
        assert_eq!(next.dirty_rows(&prev), None);
    }

    #[test]
    fn dirty_rows_row_length_change_counts_as_dirty() {
        // Same row count, but one row grew (ragged rows) — the
        // `Vec<TerminalCell>` equality is length-sensitive, so this
        // must show up as dirty rather than silently comparing equal.
        let prev = grid(&["ab", "cd"]);
        let next = grid(&["abc", "cd"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![0]));
    }

    #[test]
    fn dirty_rows_overlay_flag_change_is_dirty() {
        // Cursor / selection / find-highlight overlays are plain
        // `TerminalCell` fields, so a flag-only change (same glyph and
        // colour) still registers the row as dirty.
        let prev = grid(&["abc"]);
        let mut next = grid(&["abc"]);
        next.cells[0][1].is_cursor = true;
        assert_eq!(next.dirty_rows(&prev), Some(vec![0]));
    }

    #[test]
    fn terminal_roundtrip_serde() {
        let term = Terminal {
            id: WidgetId::new("terminal-0"),
            cells: vec![
                vec![
                    TerminalCell {
                        text: "$".to_string(),
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: true,
                        italic: false,
                        underline: false,
                        dim: false,
                        selected: false,
                        is_cursor: false,
                        is_find_match: false,
                        is_find_active: false,
                        cursor_shape: TerminalCursorShape::Block,
                        cursor_blinking: false,
                    },
                    TerminalCell {
                        // Base char + combining acute accent — a
                        // multi-codepoint grapheme cluster, not just a
                        // single `char`.
                        text: "e\u{0301}".to_string(),
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: false,
                        italic: false,
                        underline: false,
                        dim: false,
                        selected: false,
                        is_cursor: true,
                        is_find_match: false,
                        is_find_active: false,
                        cursor_shape: TerminalCursorShape::Block,
                        cursor_blinking: false,
                    },
                ],
                vec![TerminalCell {
                    text: "m".to_string(),
                    fg: Color::rgb(255, 100, 50),
                    bg: Color::rgb(20, 20, 20),
                    bold: false,
                    italic: false,
                    underline: true,
                    dim: false,
                    selected: true,
                    is_cursor: false,
                    is_find_match: true,
                    is_find_active: false,
                    cursor_shape: TerminalCursorShape::Block,
                    cursor_blinking: false,
                }],
            ],
            scrollbar: None,
        };
        let json = serde_json::to_string(&term).unwrap();
        let back: Terminal = serde_json::from_str(&json).unwrap();
        assert_eq!(term, back);
    }

    #[test]
    fn terminal_scrollbar_serde_round_trip() {
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![TerminalCell {
                text: "x".to_string(),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(20, 20, 20),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
            }]],
            scrollbar: Some(TerminalScrollbar {
                total_lines: 500,
                visible_lines: 24,
                scroll_offset: 100,
                inverted: false,
                width: None,
            }),
        };
        let json = serde_json::to_string(&term).unwrap();
        let back: Terminal = serde_json::from_str(&json).unwrap();
        assert_eq!(term, back);
        let sb = back.scrollbar.unwrap();
        assert_eq!(sb.total_lines, 500);
        assert_eq!(sb.visible_lines, 24);
        assert_eq!(sb.scroll_offset, 100);
    }

    #[test]
    fn terminal_event_roundtrip_serde() {
        let events = vec![
            TerminalEvent::KeyPressed {
                key: "a".to_string(),
                modifiers: Modifiers::default(),
            },
            TerminalEvent::SelectStart { row: 5, col: 10 },
            TerminalEvent::SelectExtend { row: 6, col: 20 },
            TerminalEvent::SelectEnd,
            TerminalEvent::Scroll { delta: -3 },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TerminalEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── D6 Terminal layout API tests ──────────────────────────────────

    fn make_term(rows: usize, cols: usize) -> Terminal {
        let cell = TerminalCell {
            text: " ".to_string(),
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(20, 20, 20),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
        };
        Terminal {
            id: WidgetId::new("term"),
            cells: (0..rows).map(|_| vec![cell.clone(); cols]).collect(),
            scrollbar: None,
        }
    }

    #[test]
    fn terminal_layout_tui_cells() {
        let term = make_term(24, 80);
        let layout = term.layout(80.0, 24.0, 1.0, 1.0);
        assert_eq!(layout.grid_rows, 24);
        assert_eq!(layout.grid_cols, 80);
        // Click at (5, 3) → cell (row=3, col=5).
        match layout.hit_test(5.5, 3.5) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(row, 3);
                assert_eq!(col, 5);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_pixel_cells() {
        // Native: 800x600 viewport, 8 px × 16 px cells = 100 cols × 37 rows.
        let term = make_term(37, 100);
        let layout = term.layout(800.0, 600.0, 8.0, 16.0);
        assert_eq!(layout.grid_cols, 100);
        assert_eq!(layout.grid_rows, 37); // 600/16 = 37.5 → 37
                                          // Click at (160, 48) → col=20, row=3.
        match layout.hit_test(160.0, 48.0) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(col, 20);
                assert_eq!(row, 3);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_hit_test_outside() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        assert_eq!(layout.hit_test(-1.0, 5.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(5.0, -1.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(100.0, 5.0), TerminalHit::Empty);
    }

    #[test]
    fn terminal_layout_cell_bounds() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        let r = layout.cell_bounds(3, 5).unwrap();
        assert_eq!(r.x, 5.0);
        assert_eq!(r.y, 3.0);
        assert_eq!(r.width, 1.0);
        assert_eq!(r.height, 1.0);
        // Out of range → None.
        assert!(layout.cell_bounds(99, 0).is_none());
        assert!(layout.cell_bounds(0, 99).is_none());
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent,
        WidgetAction,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::{Handled, Key, MouseButton, Point};

    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: usize,
        captures: usize,
        releases: usize,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) {
            self.focus_requests += 1;
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captures += 1;
        }
        fn release_pointer(&mut self) {
            self.releases += 1;
        }
        fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn cell(text: &str) -> TerminalCell {
        TerminalCell {
            text: text.to_string(),
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(20, 20, 20),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
        }
    }

    fn term(rows: usize, cols: usize, scrollbar: Option<TerminalScrollbar>) -> Terminal {
        Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell("x"); cols]; rows],
            scrollbar,
        }
    }

    fn scrollbar(width: Option<u16>) -> TerminalScrollbar {
        TerminalScrollbar {
            total_lines: 100,
            visible_lines: 5,
            scroll_offset: 0,
            inverted: false,
            width,
        }
    }

    fn cells_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, y),
            mods: Modifiers::default(),
        }
    }

    fn wheel(dy: f32) -> RoutedEvent {
        RoutedEvent::Wheel {
            dx: 0.0,
            dy,
            local: Point::new(1.0, 1.0),
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn declares_a_single_focus_stop() {
        assert_eq!(term(1, 1, None).focus(), FocusSpec::Stop);
    }

    /// The scrollbar gutter comes off the grid before the viewport is
    /// divided into cells, so a click in the gutter is not a cell.
    #[test]
    fn cell_layout_reserves_a_one_column_scrollbar_gutter() {
        let t = term(5, 20, Some(scrollbar(None)));
        let rect = Rect::new(0.0, 0.0, 10.0, 5.0);
        let layout = Primitive::layout(&t, &cells_ctx(), rect);
        assert_eq!(layout.grid_cols, 9);
        assert_eq!(layout.grid_rows, 5);
        assert_eq!(layout.hit_test(9.0, 0.0), TerminalHit::Empty);
        assert_eq!(layout.scrollbar, Some(Rect::new(9.0, 0.0, 1.0, 5.0)));
        assert_eq!(layout.area, rect);
    }

    #[test]
    fn an_explicit_scrollbar_width_overrides_the_default_gutter() {
        let t = term(5, 20, Some(scrollbar(Some(3))));
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(layout.grid_cols, 7);
    }

    /// Pixel cells are the editor font's — its advance and its line
    /// height — and the gutter is the `Style::scrollbar_width` token.
    #[test]
    fn pixel_layout_uses_editor_metrics_and_the_style_scrollbar_width() {
        let t = term(5, 20, Some(scrollbar(None)));
        let style = crate::Style {
            scrollbar_width: 12.0,
            ..crate::Style::default()
        };
        let ctx = LayoutCtx::new(24.0, 8.0, false)
            .with_editor_line_height(16.0)
            .with_style(&style);
        let layout = Primitive::layout(&t, &ctx, Rect::new(10.0, 20.0, 100.0, 64.0));
        assert_eq!(layout.cell_size, TerminalCellSize::new(8.0, 16.0));
        assert_eq!(layout.viewport_width, 88.0);
        assert_eq!(layout.grid_cols, 11);
        assert_eq!(layout.grid_rows, 4);
        assert_eq!(layout.scrollbar, Some(Rect::new(98.0, 20.0, 12.0, 64.0)));
        assert_eq!(
            layout.cell_bounds(1, 2),
            Some(Rect::new(16.0, 16.0, 8.0, 16.0)),
            "cell bounds stay local to the pane"
        );
    }

    #[test]
    fn a_terminal_fills_its_pane_rather_than_measuring_its_snapshot() {
        let t = term(3, 4, None);
        assert_eq!(
            Primitive::measure(&t, &cells_ctx(), crate::primitive::Size::new(80.0, 24.0)),
            crate::NaturalSize::NONE
        );
    }

    #[test]
    fn a_left_press_on_a_cell_focuses_captures_and_starts_a_selection() {
        let t = term(5, 10, None);
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        let mut state = TerminalState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(t.id, &mut host);
        let outcome = t.handle(
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 3.5, 2.5),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(TerminalEvent::SelectStart { row: 2, col: 3 })
        );
        assert_eq!(state.drag, Some((2, 3)));
        assert_eq!((host.focus_requests, host.captures), (1, 1));
    }

    #[test]
    fn a_press_on_the_scrollbar_gutter_only_focuses() {
        let t = term(5, 10, Some(scrollbar(None)));
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        let mut state = TerminalState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(t.id, &mut host);
        let outcome = t.handle(
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 9.5, 1.0),
            &layout,
            &mut cx,
        );
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.drag, None);
        assert_eq!((host.focus_requests, host.captures), (1, 0));
    }

    /// A drag reports each new cell once, clamps onto the grid when the
    /// pointer leaves the pane, and the release ends the selection.
    #[test]
    fn a_drag_extends_once_per_cell_and_the_release_ends_it() {
        let t = term(5, 10, None);
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        let mut state = TerminalState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(t.id, &mut host);
        let mut send =
            |state: &mut TerminalState, ev: RoutedEvent| t.handle(state, &ev, &layout, &mut cx);

        assert_eq!(
            send(
                &mut state,
                pointer(PointerKind::Down(MouseButton::Left), 0.0, 0.0)
            ),
            Handled::Event(TerminalEvent::SelectStart { row: 0, col: 0 })
        );
        assert_eq!(
            send(&mut state, pointer(PointerKind::Move, 4.2, 1.0)),
            Handled::Event(TerminalEvent::SelectExtend { row: 1, col: 4 })
        );
        assert_eq!(
            send(&mut state, pointer(PointerKind::Move, 4.8, 1.9)),
            Handled::Consumed,
            "a move within the same cell reports nothing new"
        );
        assert_eq!(
            send(&mut state, pointer(PointerKind::Move, 50.0, 40.0)),
            Handled::Event(TerminalEvent::SelectExtend { row: 4, col: 9 })
        );
        assert_eq!(
            send(
                &mut state,
                pointer(PointerKind::Up(MouseButton::Left), 50.0, 40.0)
            ),
            Handled::Event(TerminalEvent::SelectEnd)
        );
        assert_eq!(state.drag, None);
        assert_eq!(
            send(&mut state, pointer(PointerKind::Move, 1.0, 1.0)),
            Handled::Ignored,
            "no drag, no selection event"
        );
        assert_eq!((host.captures, host.releases), (1, 1));
    }

    #[test]
    fn a_wheel_notch_scrolls_one_row_negative_into_history() {
        let t = term(5, 10, None);
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        let mut state = TerminalState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(t.id, &mut host);
        assert_eq!(
            t.handle(&mut state, &wheel(1.0), &layout, &mut cx),
            Handled::Event(TerminalEvent::Scroll { delta: -1 })
        );
        assert_eq!(
            t.handle(&mut state, &wheel(-3.0), &layout, &mut cx),
            Handled::Event(TerminalEvent::Scroll { delta: 3 })
        );
        assert_eq!(
            t.handle(&mut state, &wheel(0.0), &layout, &mut cx),
            Handled::Ignored
        );
    }

    /// Keys bubble to the host, which forwards them to the PTY.
    #[test]
    fn keys_are_left_for_the_host() {
        let t = term(5, 10, None);
        let layout = Primitive::layout(&t, &cells_ctx(), Rect::new(0.0, 0.0, 10.0, 5.0));
        let mut state = TerminalState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(t.id, &mut host);
        let key = RoutedEvent::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(
            t.handle(&mut state, &key, &layout, &mut cx),
            Handled::Ignored
        );
    }

    #[test]
    fn terminal_events_ride_ui_event_terminal() {
        let id = WidgetId::new("term");
        assert_eq!(
            TerminalEvent::SelectEnd.into_ui_event(id),
            crate::UiEvent::Terminal(id, TerminalEvent::SelectEnd)
        );
    }

    /// Event parity's in-crate legs: a scroll-up assistive action routed
    /// through [`crate::a11y::route_widget_action`], the same action
    /// handed straight to [`Interactive::handle`] (a binding host's
    /// dispatch shape), and one upward wheel notch all resolve to the
    /// same [`crate::UiEvent`]. The `TuiDriver` leg lives in
    /// `tests/widget_contract_terminal.rs`.
    #[test]
    fn route_widget_action_scroll_matches_direct_handle_and_the_wheel() {
        let t = term(5, 10, None);
        let id = t.id;
        let rect = Rect::new(0.0, 0.0, 10.0, 5.0);
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, rect, Box::new(t.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = cells_ctx();

        let routed =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::ScrollUp);

        let layout = Primitive::layout(&t, &ctx, rect);
        let mut host = MockHost::default();
        let mut cx = EventCx::new(id, &mut host);
        let mut direct = Vec::new();
        let mut via_wheel = Vec::new();
        if let Handled::Event(ev) = t.handle(
            &mut TerminalState::default(),
            &RoutedEvent::Action(A11yAction::ScrollUp),
            &layout,
            &mut cx,
        ) {
            direct.push(ev.into_ui_event(id));
        }
        if let Handled::Event(ev) =
            t.handle(&mut TerminalState::default(), &wheel(1.0), &layout, &mut cx)
        {
            via_wheel.push(ev.into_ui_event(id));
        }

        let expected = vec![crate::UiEvent::Terminal(
            id,
            TerminalEvent::Scroll { delta: -1 },
        )];
        assert_eq!(routed, expected);
        assert_eq!(direct, expected);
        assert_eq!(via_wheel, expected);
    }

    #[test]
    fn advertises_focus_and_scroll_actions() {
        assert_eq!(
            term(1, 1, None).a11y_actions(&TerminalState::default()),
            vec![
                A11yAction::Focus,
                A11yAction::ScrollUp,
                A11yAction::ScrollDown
            ]
        );
    }

    #[test]
    fn the_terminal_kind_decodes_through_widget_kinds() {
        let payload = serde_json::to_value(term(1, 2, Some(scrollbar(None)))).unwrap();
        match crate::widget_kinds::decode_widget("terminal", payload) {
            Ok(widget) => assert_eq!(widget.id().map(|id| id.as_str()), Some("term")),
            Err(err) => panic!("expected a decoded terminal, got Err({err})"),
        }
    }

    #[test]
    fn the_divider_is_one_unit_wide_and_lays_out_its_rect() {
        let d = TerminalDivider::new(WidgetId::new("divider"));
        let rect = Rect::new(40.0, 0.0, 1.0, 24.0);
        assert_eq!(Primitive::layout(&d, &cells_ctx(), rect).bounds, rect);
        assert_eq!(
            Primitive::measure(&d, &cells_ctx(), crate::primitive::Size::new(80.0, 24.0)),
            crate::NaturalSize::new(Some(1.0), None)
        );
    }
}

/// The cell painter's output, ported from the TUI rasteriser's own tests
/// and driven through [`crate::primitive::Primitive::paint_cells`].
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::*;
    use crate::primitive::{LayoutCtx, Primitive};
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    fn blank_cell() -> TerminalCell {
        TerminalCell {
            text: " ".to_string(),
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
        }
    }

    fn make_terminal(rows: usize, cols: usize, sb: Option<TerminalScrollbar>) -> Terminal {
        Terminal {
            id: WidgetId::new("test-term"),
            cells: vec![vec![blank_cell(); cols]; rows],
            scrollbar: sb,
        }
    }

    fn paint(term: &Terminal, width: u16, height: u16) -> Buffer {
        let rect = Rect::new(0.0, 0.0, width as f32, height as f32);
        let layout = Primitive::layout(term, &LayoutCtx::new(1.0, 1.0, false), rect);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, width, height));
        Primitive::paint_cells(
            term,
            &layout,
            &mut crate::tui::BufferCellSurface::new(&mut buf),
            &crate::PaintEnv::new(&Theme::default(), &crate::Style::default()),
        );
        buf
    }

    fn color(c: Color) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }

    /// The top-most and bottom-most rows holding the thumb glyph (`█`)
    /// in the scrollbar column.
    fn find_thumb_extent(buf: &Buffer, sb_col: u16, height: u16) -> Option<(u16, u16)> {
        let rows: Vec<u16> = (0..height)
            .filter(|&y| buf[(sb_col, y)].symbol() == "█")
            .collect();
        rows.first().copied().zip(rows.last().copied())
    }

    /// A `Block` cursor paints as a full colour invert.
    #[test]
    fn block_cursor_paints_inverted_with_no_underline() {
        let mut c = blank_cell();
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let buf = paint(&term, 1, 1);
        let cell = &buf[(0, 0)];
        assert_eq!(
            cell.fg,
            color(Color::rgb(0, 0, 0)),
            "inverted: fg becomes bg"
        );
        assert_eq!(
            cell.bg,
            color(Color::rgb(200, 200, 200)),
            "inverted: bg becomes fg"
        );
        assert!(!cell.modifier.contains(Modifier::UNDERLINED));
    }

    /// `Underline`/`Bar` cursors don't invert but do underline the cell
    /// in its own foreground — a terminal cell has no sub-cell geometry
    /// for a real thin bar.
    #[test]
    fn underline_and_bar_cursor_paint_uninverted_with_underline_modifier() {
        for shape in [TerminalCursorShape::Underline, TerminalCursorShape::Bar] {
            let mut c = blank_cell();
            c.is_cursor = true;
            c.cursor_shape = shape;
            let term = Terminal {
                id: WidgetId::new("t"),
                cells: vec![vec![c]],
                scrollbar: None,
            };
            let buf = paint(&term, 1, 1);
            let cell = &buf[(0, 0)];
            assert_eq!(cell.fg, color(Color::rgb(200, 200, 200)), "{shape:?}");
            assert_eq!(cell.bg, color(Color::rgb(0, 0, 0)), "{shape:?}");
            assert!(cell.modifier.contains(Modifier::UNDERLINED), "{shape:?}");
            assert_eq!(cell.underline_color, color(Color::rgb(200, 200, 200)));
        }
    }

    /// A steady (`cursor_blinking: false`) `Underline` cursor is always
    /// visible; a blinking one toggles on the wall clock, which a test
    /// can't pin without flaking.
    #[test]
    fn steady_underline_cursor_is_always_visible() {
        let mut c = blank_cell();
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Underline;
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        assert!(paint(&term, 1, 1)[(0, 0)]
            .modifier
            .contains(Modifier::UNDERLINED));
    }

    #[test]
    fn bold_and_italic_cells_carry_their_modifiers() {
        let mut c = blank_cell();
        c.text = "B".to_string();
        c.bold = true;
        c.italic = true;
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let buf = paint(&term, 1, 1);
        assert_eq!(buf[(0, 0)].symbol(), "B");
        assert!(buf[(0, 0)]
            .modifier
            .contains(Modifier::BOLD | Modifier::ITALIC));
    }

    /// A base character plus a combining mark paints as one glyph in one
    /// cell, not as the base character alone.
    #[test]
    fn a_combining_mark_grapheme_paints_as_one_cell() {
        let mut c = blank_cell();
        c.text = "e\u{0301}".to_string();
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![c, blank_cell()]],
            scrollbar: None,
        };
        let buf = paint(&term, 2, 1);
        assert_eq!(buf[(0, 0)].symbol(), "e\u{0301}");
        assert_eq!(buf[(1, 0)].symbol(), " ");
    }

    #[test]
    fn a_control_character_paints_as_a_blank() {
        let mut c = blank_cell();
        c.text = "\u{7}".to_string();
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        assert_eq!(paint(&term, 1, 1)[(0, 0)].symbol(), " ");
    }

    /// Column 9 is the scrollbar gutter, so no cell paints there even
    /// though the snapshot carries twenty columns.
    #[test]
    fn the_scrollbar_gutter_shows_no_cell_content() {
        let mut term = make_terminal(
            5,
            20,
            Some(TerminalScrollbar {
                total_lines: 100,
                visible_lines: 5,
                scroll_offset: 0,
                inverted: false,
                width: None,
            }),
        );
        for row in &mut term.cells {
            for c in row.iter_mut() {
                c.text = "x".to_string();
            }
        }
        let buf = paint(&term, 10, 5);
        assert_eq!(buf[(8, 0)].symbol(), "x");
        assert_ne!(buf[(9, 0)].symbol(), "x");
    }

    #[test]
    fn inverted_offset_zero_thumb_at_bottom() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        let buf = paint(&make_terminal(20, 39, Some(sb)), 40, 20);
        let (top, bot) = find_thumb_extent(&buf, 39, 20).expect("thumb should be painted");
        assert!(bot >= 18, "thumb bottom ({bot}) should be at/near row 19");
        assert!(top > 0, "thumb top ({top}) should not be at row 0");
    }

    #[test]
    fn inverted_offset_max_thumb_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 480,
            inverted: true,
            width: None,
        };
        let buf = paint(&make_terminal(20, 39, Some(sb)), 40, 20);
        let (top, _) = find_thumb_extent(&buf, 39, 20).expect("thumb should be painted");
        assert_eq!(top, 0);
    }

    #[test]
    fn non_inverted_offset_zero_thumb_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 0,
            inverted: false,
            width: None,
        };
        let buf = paint(&make_terminal(20, 39, Some(sb)), 40, 20);
        let (top, _) = find_thumb_extent(&buf, 39, 20).expect("thumb should be painted");
        assert_eq!(top, 0);
    }

    #[test]
    fn the_divider_paints_a_separator_column() {
        let theme = Theme::default();
        let d = TerminalDivider::new(WidgetId::new("divider"));
        let rect = Rect::new(2.0, 1.0, 1.0, 3.0);
        let layout = Primitive::layout(&d, &LayoutCtx::new(1.0, 1.0, false), rect);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 5, 5));
        Primitive::paint_cells(
            &d,
            &layout,
            &mut crate::tui::BufferCellSurface::new(&mut buf),
            &crate::PaintEnv::new(&theme, &crate::Style::default()),
        );
        for y in 1..4 {
            assert_eq!(buf[(2, y)].symbol(), "│");
            assert_eq!(buf[(2, y)].fg, color(theme.separator));
            assert_eq!(buf[(2, y)].bg, color(theme.background));
        }
        assert_eq!(buf[(2, 0)].symbol(), " ");
        assert_eq!(buf[(2, 4)].symbol(), " ");
    }
}
