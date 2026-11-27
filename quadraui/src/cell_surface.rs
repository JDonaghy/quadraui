//! `CellSurface` — the cell-grid counterpart of [`crate::PaintSurface`].
//!
//! # The problem this solves
//!
//! [`crate::PaintSurface`]'s own module doc explains why the TUI backend
//! stays out of that trait entirely: a terminal paints a cell grid, not a
//! pixel canvas, so there is no sub-cell `Rect`, no fractional stroke
//! width, and glyph selection (box-drawing corners, double-width CJK) has
//! no pixel-shaped equivalent. A cell painter still needs a shared drawing
//! seam, and `CellSurface` is it, the same way `PaintSurface` is the
//! pixel backends' one: a widget's cell painter takes
//! `&mut dyn CellSurface` rather than a concrete ratatui type, and lives
//! in the widget's own module.
//!
//! # Scope
//!
//! Every cell painter writes through this trait's verbs alone: glyphs
//! ([`CellSurface::put_char`] and friends), fills and borders, and the
//! in-place decorations ([`CellSurface::tint_bg`],
//! [`CellSurface::set_bg`], [`CellSurface::add_modifier`]) that recolour
//! or mark a cell another painter already wrote.
//! `crate::tui::BufferCellSurface` is the one implementor, over a ratatui
//! buffer.
//!
//! # Why this stays unconditional
//!
//! Every type this trait's required methods trade in ([`crate::Rect`],
//! [`crate::Color`], [`crate::Image`], [`crate::backend::ColorDepth`]) is
//! already compiled unconditionally, so the trait itself needs no `tui`
//! feature gate — mirroring [`crate::PaintSurface`], which is also
//! compiled unconditionally despite having no in-crate implementor on a
//! `tui`-only build.

use crate::backend::ColorDepth;
use crate::{Color, Image, Rect};

/// Per-cell styling: colours plus the modifier flags every terminal SGR
/// sequence can express. Mirrors the fields `crate::tui::set_cell_styled`
/// already threads through one at a time (`fg`, `bg`, `Modifier`,
/// `underline_color`), gathered into one `Copy` struct so a `CellSurface`
/// method takes one argument instead of five.
///
/// `fg`/`bg` of `None` reset to the terminal's own default foreground/
/// background (SGR `39`/`49`) rather than naming a colour at all — the
/// same "no explicit colour" state a bare, un-styled terminal cell
/// already starts a session in, so a `CellSurface` caller painting
/// mostly-default text doesn't have to look up and repeat a theme colour
/// at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CellStyle {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    /// `Some(color)` underlines in that colour; `None` draws no
    /// underline. There is no "underline, default colour" state distinct
    /// from `bold`/`italic`'s plain `bool` because an underline is the
    /// one modifier every terminal emulator also lets a caller recolour
    /// independently of the glyph's own foreground (SGR `58`/`59`) — the
    /// same `underline_color: Option<RatatuiColor>` shape
    /// `crate::tui::set_cell_styled` already takes.
    pub underline: Option<Color>,
    pub dim: bool,
    pub reverse: bool,
}

/// Which box-drawing glyph family [`CellSurface::draw_border`] paints
/// with. Terminal emulators ship all four as part of the standard Unicode
/// box-drawing block, so this is a closed, terminal-native set rather
/// than an open-ended "pick any glyphs" parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderSet {
    /// `┌─┐│└─┘` — sharp corners.
    Square,
    /// `╭─╮│╰─╯` — the rounded corners the TUI dialog, board, tree and
    /// focus-ring borders paint.
    Rounded,
    /// `╔═╗║╚═╝` — double-line.
    Double,
    /// `┏━┓┃┗━┛` — heavy/bold-line.
    Heavy,
}

/// Terminal cursor shape, for [`CellSurface::set_cursor`]. Mirrors the
/// three shapes every terminal emulator's DECSCUSR escape
/// (`crossterm::cursor::SetCursorStyle`) can select between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// The cell-grid drawing seam underneath `crate::tui::backend::TuiBackend`
/// — see the module doc for scope and why TUI (not GTK/macOS/Win) is the
/// one implementor.
pub trait CellSurface {
    /// `(width, height)` of this surface's cell grid, in whole columns
    /// and rows.
    fn size(&self) -> (u16, u16);

    /// Write a single-width `char` at `(x, y)` in `style`. Out-of-bounds
    /// coordinates are silently ignored — matches
    /// `crate::tui::set_cell_styled`'s own bounds check, so a caller painting
    /// right up to a surface's edge never has to clamp first.
    fn put_char(&mut self, x: u16, y: u16, ch: char, style: CellStyle);

    /// Write a double-width `char` (CJK, most Nerd Font glyphs) at
    /// `(x, y)`, and blank `(x + 1, y)` — the continuation cell every
    /// double-width glyph occupies on a terminal grid — so the renderer's
    /// diff doesn't paint a stray character on top of the wide glyph's
    /// second column. Mirrors `crate::tui::set_cell_wide_styled`.
    fn put_wide(&mut self, x: u16, y: u16, ch: char, style: CellStyle);

    /// Write one grapheme cluster — a base character plus any combining
    /// marks, e.g. `"e\u{0301}"` — into the single cell at `(x, y)`, as
    /// one glyph. A cluster containing an ASCII control character writes a
    /// blank instead. The cell after a double-width cluster is left as
    /// the caller painted it (a terminal grid supplies its own
    /// continuation cell). Out-of-bounds coordinates are ignored.
    ///
    /// Defaults to writing the cluster's first `char` through
    /// [`Self::put_char`] — correct for a surface whose cells hold one
    /// `char` each.
    fn put_grapheme(&mut self, x: u16, y: u16, grapheme: &str, style: CellStyle) {
        let ch = grapheme.chars().next().unwrap_or(' ');
        self.put_char(x, y, ch, style);
    }

    /// Write `text` starting at `(x, y)`, stopping before the glyph that
    /// would cross `max_cols` display columns (never splitting a
    /// double-width glyph's two columns), and return the number of
    /// columns actually written. Display-width aware throughout — a CJK
    /// run counts two columns per character, matching
    /// `crate::tui::text::display_width`.
    fn put_str_clipped(
        &mut self,
        x: u16,
        y: u16,
        text: &str,
        max_cols: u16,
        style: CellStyle,
    ) -> u16;

    /// Fill every cell inside `rect` with a blank glyph styled `style` —
    /// a background wash, the cell-grid equivalent of
    /// [`crate::PaintSurface::surface_fill_rect`].
    fn fill(&mut self, rect: Rect, style: CellStyle);

    /// Paint a box-drawing border around `rect` using `set`'s glyph
    /// family, styled `style`. `rect` must be at least 2×2 cells for a
    /// border to be drawable at all; a smaller rect paints nothing.
    fn draw_border(&mut self, rect: Rect, set: BorderSet, style: CellStyle);

    /// Read back the glyph and style currently at `(x, y)`, or `None` if
    /// out of bounds. Needed by a composite widget's junction glyph (e.g.
    /// a split divider choosing `┬` vs `┼` based on what its neighbour
    /// already painted at the crossing cell) — the one case a write-only
    /// surface can't support.
    fn get(&self, x: u16, y: u16) -> Option<(char, CellStyle)>;

    /// Blend `color` over the background of the cell at `(x, y)` at
    /// `color.a / 255`, leaving its glyph, foreground and modifiers as
    /// they are — the cell-grid form of a translucent
    /// [`crate::PaintSurface::surface_fill_rect`]. A cell whose background
    /// is not a literal colour, or that lies out of bounds, is left alone.
    ///
    /// Default: read the cell back through [`Self::get`] and rewrite it
    /// with [`Self::put_char`], which keeps only the cell's first char.
    fn tint_bg(&mut self, x: u16, y: u16, color: Color) {
        let Some((ch, mut style)) = self.get(x, y) else {
            return;
        };
        if let Some(base) = style.bg {
            style.bg = Some(base.blend(color, color.a as f64 / 255.0));
            self.put_char(x, y, ch, style);
        }
    }

    /// Request the terminal cursor be shown at `(x, y)` in `shape` once
    /// this frame is presented.
    fn set_cursor(&mut self, x: u16, y: u16, shape: CursorShape);

    /// Emit an out-of-band graphics escape (e.g. the Kitty graphics
    /// protocol) to paint `image` into `rect`'s cells directly, bypassing
    /// glyph-based rendering. Returns `true` if the surface actually
    /// emitted the escape, `false` if it has no such capability — the
    /// same "this call did nothing, paint your own fallback" contract
    /// `crate::backend::ImagePaintResult::Unsupported` carries for pixel
    /// backends.
    ///
    /// Default: unconditionally unsupported — correct for any surface
    /// with no out-of-band graphics channel at all.
    fn emit_graphics(&mut self, rect: Rect, image: &Image) -> bool {
        let _ = (rect, image);
        false
    }

    /// This surface's actual colour fidelity — see [`ColorDepth`].
    /// Informational only: every `put_*`/`fill`/`draw_border` call still
    /// takes a full RGB [`Color`] regardless of this value — quantising
    /// to the real terminal's depth happens downstream, once, at
    /// `crate::tui::color::DepthLimitedBackend`'s choke point (see that
    /// type's module doc), not per cell. A caller reads this to pre-adapt
    /// a *palette choice* instead (e.g. avoiding two colours that would
    /// quantise to the same `Ansi16` value), not to quantise anything
    /// itself.
    fn color_depth(&self) -> ColorDepth;

    /// Replace the background of the cell at `(x, y)` with `color`,
    /// leaving its glyph, foreground and modifiers as they are — the
    /// opaque counterpart of [`Self::tint_bg`], for an overlay that
    /// recolours cells another painter already filled. Out-of-bounds
    /// coordinates are ignored.
    ///
    /// Default: read the cell back through [`Self::get`] and rewrite it
    /// with [`Self::put_char`], which keeps only the cell's first char.
    fn set_bg(&mut self, x: u16, y: u16, color: Color) {
        let Some((ch, mut style)) = self.get(x, y) else {
            return;
        };
        style.bg = Some(color);
        self.put_char(x, y, ch, style);
    }

    /// Add `modifier` to the cell at `(x, y)`, leaving its glyph, colours
    /// and other modifiers as they are — decoration over content another
    /// painter already wrote, such as a boundary underline across a row
    /// or a reverse-video selection. Writing the cell afresh instead
    /// would split a double-width glyph's continuation cell from its
    /// glyph. Out-of-bounds coordinates are ignored.
    ///
    /// Default: read the cell back through [`Self::get`] and rewrite it
    /// with [`Self::put_char`], which keeps only the cell's first char; an
    /// [`CellModifier::Underline`] with no colour underlines in the
    /// cell's foreground.
    fn add_modifier(&mut self, x: u16, y: u16, modifier: CellModifier) {
        let Some((ch, mut style)) = self.get(x, y) else {
            return;
        };
        match modifier {
            CellModifier::Reverse => style.reverse = true,
            CellModifier::Underline { color } => {
                style.underline = color.or(style.underline).or(style.fg);
            }
        }
        self.put_char(x, y, ch, style);
    }
}

/// A modifier [`CellSurface::add_modifier`] adds to an already-painted
/// cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellModifier {
    /// Reverse video: the cell's foreground and background swap.
    Reverse,
    /// An underline. `Some(color)` recolours it; `None` keeps whatever
    /// underline colour the cell already carries.
    Underline { color: Option<Color> },
}

/// A [`CellStyle`] that names both colours and no modifiers — the style
/// almost every cell painter's plain glyph wants.
///
/// `#[cfg(feature = "tui")]`: every caller is a widget's `cell_paint`
/// module, and TUI is the only backend with a cell grid to paint onto.
#[cfg(feature = "tui")]
pub(crate) fn solid_style(fg: Color, bg: Color) -> CellStyle {
    CellStyle {
        fg: Some(fg),
        bg: Some(bg),
        ..CellStyle::default()
    }
}

/// Write `text` on `rect`'s first row, clipped to `rect`'s width — the
/// cell-grid counterpart of
/// [`crate::PaintSurface::surface_draw_text_run`] for a single-line
/// control.
///
/// `#[cfg(feature = "tui")]` for the same reason [`solid_style`] is.
#[cfg(feature = "tui")]
pub(crate) fn draw_text_row(
    surface: &mut dyn CellSurface,
    rect: Rect,
    text: &str,
    style: CellStyle,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    surface.put_str_clipped(x0, y0, text, w, style);
}

/// Stroke a single-cell `┌─┐│└─┘` frame around `rect`, degrading
/// gracefully when `rect` is too small for a real box: a one-row rect
/// gets `┌─┐`, a one-column rect gets the left edge's glyphs, and a 1×1
/// rect gets `┌`.
///
/// Not [`CellSurface::draw_border`]: that verb paints nothing under 2×2
/// cells, which is exactly the size an input field's chrome collapses to
/// on a cell grid — a single-row [`crate::ComboBox`] field, a
/// [`crate::NumberInput`] stepper cell on a two-row control — and the
/// frame is the affordance that makes those read as controls at all.
/// Lives here rather than in either widget so the two cell painters
/// share one frame routine.
///
/// `#[cfg(feature = "tui")]` for the same reason [`solid_style`] is.
#[cfg(feature = "tui")]
pub(crate) fn stroke_square_frame(surface: &mut dyn CellSurface, rect: Rect, style: CellStyle) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }
    let x1 = x0 + w.saturating_sub(1);
    let y1 = y0 + h.saturating_sub(1);
    for x in x0..=x1 {
        surface.put_char(x, y0, '─', style);
        if y1 != y0 {
            surface.put_char(x, y1, '─', style);
        }
    }
    for y in y0..=y1 {
        surface.put_char(x0, y, '│', style);
        if x1 != x0 {
            surface.put_char(x1, y, '│', style);
        }
    }
    surface.put_char(x0, y0, '┌', style);
    if x1 != x0 {
        surface.put_char(x1, y0, '┐', style);
    }
    if y1 != y0 {
        surface.put_char(x0, y1, '└', style);
        if x1 != x0 {
            surface.put_char(x1, y1, '┘', style);
        }
    }
}

/// Shared `U+2800`-block braille dot packing, used by every cell painter
/// that needs sub-cell resolution: [`crate::Chart`]'s line charts,
/// [`crate::Canvas`]'s shape degrade and [`crate::Minimap`]'s density view.
///
/// There must be exactly one copy of this bit-order logic: getting it wrong
/// transposes the whole image in a way that still looks plausible.
#[cfg(feature = "tui")]
pub(crate) mod braille {

    /// Braille dot offsets: `(row_in_cell, col_in_cell) -> bit index`.
    /// Standard Unicode braille ordering — a terminal cell is 2 dots wide
    /// (`col_in_cell` in `0..2`) by 4 dots tall (`row_in_cell` in `0..4`).
    pub(crate) const BRAILLE_OFFSETS: [(usize, usize); 8] = [
        (0, 0), // bit 0
        (1, 0), // bit 1
        (2, 0), // bit 2
        (0, 1), // bit 3
        (1, 1), // bit 4
        (2, 1), // bit 5
        (3, 0), // bit 6
        (3, 1), // bit 7
    ];

    /// Pack one 2x4 dot cell into its `U+2800`-block braille codepoint.
    /// `dot_at(row, col)` is queried for every one of the 8 dots
    /// (`row` in `0..4`, `col` in `0..2`) and should return whether that dot
    /// is set. An all-`false` cell packs to `U+2800` itself (blank braille),
    /// not a space — callers that want to skip painting an empty cell decide
    /// that themselves by comparing the result to `'\u{2800}'`.
    pub(crate) fn pack_braille_cell(mut dot_at: impl FnMut(usize, usize) -> bool) -> char {
        let mut code: u32 = 0x2800;
        for (bit, &(row, col)) in BRAILLE_OFFSETS.iter().enumerate() {
            if dot_at(row, col) {
                code |= 1 << bit;
            }
        }
        char::from_u32(code).unwrap_or(' ')
    }

    /// Threshold a dot's coverage (`covered` non-whitespace source columns out
    /// of `bucket_width` total) against a 4x4 ordered-dither (Bayer) matrix,
    /// indexed by the dot's own `(row, col)` position within the whole
    /// rendered grid (not just within its cell) so that the dither pattern
    /// tiles consistently across the entire minimap strip rather than
    /// repeating identically inside every cell.
    ///
    /// [`crate::Minimap`]'s density view decides "is this dot set?" with a
    /// boolean OR over the dot's source-column bucket (`any(|c|
    /// !c.is_whitespace())`), which saturates every dot from the end of a
    /// line's indent onward as soon as the bucket widens past one column —
    /// every row of real code runs to the strip's right edge with no
    /// line-length signal at all. An ordered dither spreads that "is there
    /// *any* code here" boolean across many dots' worth of threshold instead:
    /// a sparsely-covered bucket only lights up the small subset of dot
    /// positions whose threshold value happens to be low, while a
    /// fully-covered bucket lights up every position — which is what produces
    /// a raggedy, VS-Code-like right edge instead of a solid wall.
    ///
    /// Pure integer arithmetic — one multiply and one compare, no floating
    /// point, no lookahead, no per-call allocation. A fully-covered bucket
    /// (`covered == bucket_width`) always returns `true` and an
    /// all-whitespace bucket (`covered == 0`) always returns `false` —
    /// dithering only has any effect strictly *between* those two extremes.
    ///
    /// Re-exported from [`crate::primitives::minimap`] rather than defined
    /// here: that module's `sample_blocks` uses the exact same matrix and
    /// threshold formula to make an analogous decision one granularity
    /// coarser — whether a *block*'s per-column coverage (several real
    /// buffer lines folded into one output row) reads back non-blank. A
    /// single shared definition makes both compositions provably the same
    /// dither policy applied twice, not two that happen to agree today.
    pub(crate) use crate::primitives::minimap::dither_threshold_met;

    /// Set every dot on the straight line between `(x0, y0)` and `(x1, y1)`
    /// in `grid` (indexed `grid[row][col]`, i.e. `grid[y][x]`), including
    /// both endpoints. Shared by every rasteriser in this crate that needs to
    /// connect two dot-grid coordinates with a solid stroke — [`crate::Chart`]'s
    /// line charts and [`crate::Canvas`]'s `DrawOp::Line`/`DrawOp::Path`
    /// degrade both call this rather than keeping their own,
    /// independently-tuned copy of a line-rasterisation loop, which is
    /// exactly the kind of drift this module's own doc comment already warns
    /// about for the bit-packing table above.
    ///
    /// Out-of-bounds coordinates (`iy >= grid.len()` or `ix >= grid[0].len()`)
    /// are silently skipped rather than panicking — a caller that clamps its
    /// own endpoints to the grid never hits this, but the interpolated path
    /// between two in-bounds endpoints cannot itself leave the grid (each
    /// step is a convex combination of two in-bounds points), so this guard
    /// is defensive rather than load-bearing.
    ///
    /// **`(x0, y0) == (x1, y1)` is a no-op** — every caller already sets a
    /// coincident single point's own dot itself before calling this for the
    /// *connecting* segment, so a caller that wants a lone point rendered
    /// when both endpoints coincide must set it separately; see
    /// [`crate::Canvas`]'s `DrawOp::Line` handling for an example.
    pub(crate) fn interpolate_dots(
        grid: &mut [Vec<bool>],
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
    ) {
        let dx = (x1 as isize - x0 as isize).abs();
        let dy = (y1 as isize - y0 as isize).abs();
        let steps = dx.max(dy);
        if steps == 0 {
            return;
        }
        for step in 0..=steps {
            let t = step as f64 / steps as f64;
            let ix = (x0 as f64 + t * (x1 as f64 - x0 as f64)).round() as usize;
            let iy = (y0 as f64 + t * (y1 as f64 - y0 as f64)).round() as usize;
            if iy < grid.len() && ix < grid[0].len() {
                grid[iy][ix] = true;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn all_dots_off_packs_to_blank_braille() {
            assert_eq!(pack_braille_cell(|_, _| false), '\u{2800}');
        }

        #[test]
        fn all_dots_on_packs_to_full_braille_block() {
            assert_eq!(pack_braille_cell(|_, _| true), '\u{28FF}');
        }

        #[test]
        fn single_dot_sets_the_documented_bit() {
            // bit 0 -> (row 0, col 0)
            assert_eq!(pack_braille_cell(|r, c| (r, c) == (0, 0)), '\u{2801}');
            // bit 3 -> (row 0, col 1)
            assert_eq!(pack_braille_cell(|r, c| (r, c) == (0, 1)), '\u{2808}');
            // bit 6 -> (row 3, col 0)
            assert_eq!(pack_braille_cell(|r, c| (r, c) == (3, 0)), '\u{2840}');
            // bit 7 -> (row 3, col 1)
            assert_eq!(pack_braille_cell(|r, c| (r, c) == (3, 1)), '\u{2880}');
        }

        // ── dither_threshold_met ───────────────────────────────────────────

        #[test]
        fn zero_coverage_never_meets_the_threshold() {
            for row in 0..4 {
                for col in 0..4 {
                    assert!(!dither_threshold_met(0, 6, row, col));
                }
            }
        }

        #[test]
        fn zero_width_bucket_never_meets_the_threshold() {
            assert!(!dither_threshold_met(0, 0, 0, 0));
            assert!(!dither_threshold_met(5, 0, 0, 0));
        }

        #[test]
        fn full_coverage_always_meets_the_threshold() {
            // A fully-covered bucket must light up at every dot position,
            // including the matrix's own largest threshold (15) -- otherwise
            // a solid run of non-whitespace would still leave holes.
            for row in 0..4 {
                for col in 0..4 {
                    assert!(
                        dither_threshold_met(6, 6, row, col),
                        "full coverage must always set the dot at ({row}, {col})"
                    );
                }
            }
        }

        #[test]
        fn partial_coverage_sets_only_some_dot_positions() {
            // 1-of-6 covered: only the positions whose Bayer threshold is low
            // enough should light up, and it must not be all-or-nothing across
            // the matrix -- that's the whole point of dithering a partially
            // covered bucket instead of booleans-OR-ing it.
            let set_count = (0..4)
                .flat_map(|row| (0..4).map(move |col| (row, col)))
                .filter(|&(row, col)| dither_threshold_met(1, 6, row, col))
                .count();
            assert!(
                set_count > 0 && set_count < 16,
                "expected a strict subset of the 16 dot positions to light up, got {set_count}/16"
            );
        }

        #[test]
        fn threshold_is_indexed_by_absolute_position_not_just_local_position() {
            // Position (0, 0) and (4, 4) share the same `& 3` fold, so they
            // must resolve identically -- confirms the matrix tiles rather
            // than being looked up some other way.
            assert_eq!(
                dither_threshold_met(3, 6, 0, 0),
                dither_threshold_met(3, 6, 4, 4)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Implements every required method as a no-op/empty-answer, so the
    /// `emit_graphics` default runs its real trait body rather than an
    /// override — proving what a surface with no out-of-band graphics
    /// channel reports by default.
    #[derive(Default)]
    struct DummySurface;

    impl CellSurface for DummySurface {
        fn size(&self) -> (u16, u16) {
            (0, 0)
        }
        fn put_char(&mut self, _x: u16, _y: u16, _ch: char, _style: CellStyle) {}
        fn put_wide(&mut self, _x: u16, _y: u16, _ch: char, _style: CellStyle) {}
        fn put_str_clipped(
            &mut self,
            _x: u16,
            _y: u16,
            _text: &str,
            _max_cols: u16,
            _style: CellStyle,
        ) -> u16 {
            0
        }
        fn fill(&mut self, _rect: Rect, _style: CellStyle) {}
        fn draw_border(&mut self, _rect: Rect, _set: BorderSet, _style: CellStyle) {}
        fn get(&self, _x: u16, _y: u16) -> Option<(char, CellStyle)> {
            None
        }
        fn set_cursor(&mut self, _x: u16, _y: u16, _shape: CursorShape) {}
        fn color_depth(&self) -> ColorDepth {
            ColorDepth::TrueColor
        }
    }

    fn test_image() -> Image {
        Image {
            id: crate::WidgetId::new("img"),
            source: crate::ImageSource::Path("/tmp/logo.png".into()),
            intrinsic_size: None,
            fit: crate::ImageFit::Fill,
            fallback_text: "[LOGO]".into(),
        }
    }

    #[test]
    fn emit_graphics_default_reports_unsupported() {
        let mut surface = DummySurface;
        assert!(!surface.emit_graphics(Rect::new(0.0, 0.0, 4.0, 4.0), &test_image()));
    }
}
