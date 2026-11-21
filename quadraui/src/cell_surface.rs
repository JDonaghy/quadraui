//! `CellSurface` — the cell-grid counterpart of [`crate::PaintSurface`].
//!
//! # The problem this solves
//!
//! [`crate::PaintSurface`]'s own module doc explains why the TUI backend
//! stays out of that trait entirely: a terminal paints a cell grid, not a
//! pixel canvas, so there is no sub-cell `Rect`, no fractional stroke
//! width, and glyph selection (box-drawing corners, double-width CJK) has
//! no pixel-shaped equivalent. [`crate::tui::backend::TuiBackend`] still
//! needs *some* shared drawing seam though — today it has only an
//! informal one: `set_cell`/`set_cell_wide`/`set_cell_styled`/
//! `draw_styled_text` in `crate::tui` plus `truncate_to_width`/
//! `truncate_to_width_ellipsis` in `crate::tui::text`, called directly by
//! each of the ~52 `tui/<widget>.rs` rasterisers against a raw
//! `&mut ratatui::buffer::Buffer`.
//!
//! `CellSurface` formalises that informal API into a trait, the same way
//! `PaintSurface` formalised each pixel backend's private fill/stroke/text
//! helpers. A widget's cell painter can then take `&mut dyn CellSurface`
//! instead of a concrete ratatui type, living in the widget's own module
//! rather than reaching into `crate::tui` for free functions.
//!
//! # Scope — additive only
//!
//! This module adds the trait and `crate::tui::TuiCellSurface`'s
//! implementation of it. Nothing in this crate paints through it yet;
//! every `tui/<widget>.rs` rasteriser keeps calling `crate::tui::set_cell`
//! and friends directly. Moving them onto `CellSurface` is later, per-widget
//! work.
//!
//! # Why this stays unconditional
//!
//! Every type this trait's required methods trade in ([`crate::Rect`],
//! [`crate::Color`], [`crate::Image`], [`crate::backend::ColorDepth`]) is
//! already compiled unconditionally, so the trait itself needs no `tui`
//! feature gate — mirroring [`crate::PaintSurface`], which is also
//! compiled unconditionally despite having no in-crate implementor on a
//! `tui`-only build. Only [`CellSurface::ratatui_buffer`] — the one method
//! that names a `ratatui` type directly — is gated on `feature = "tui"`.

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
    /// `╭─╮│╰─╯` — the rounded corners every existing TUI dialog/board/
    /// tree-rename border already paints via its own direct `set_cell`
    /// calls (`crate::tui::dialog`, `crate::tui::board`,
    /// `crate::tui::focus_ring`).
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
    /// `crate::tui::set_cell`'s own bounds check, so a caller painting
    /// right up to a surface's edge never has to clamp first.
    fn put_char(&mut self, x: u16, y: u16, ch: char, style: CellStyle);

    /// Write a double-width `char` (CJK, most Nerd Font glyphs) at
    /// `(x, y)`, and blank `(x + 1, y)` — the continuation cell every
    /// double-width glyph occupies on a terminal grid — so the renderer's
    /// diff doesn't paint a stray character on top of the wide glyph's
    /// second column. Mirrors `crate::tui::set_cell_wide`.
    fn put_wide(&mut self, x: u16, y: u16, ch: char, style: CellStyle);

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

    /// In-crate migration hatch: while a `tui/<widget>.rs` rasteriser
    /// still takes `&mut ratatui::buffer::Buffer` directly (every one of
    /// them does today — see the module doc's "Scope" section), a caller
    /// that only holds `&mut dyn CellSurface` can recover the real buffer
    /// through this method and hand it to the old-style rasteriser
    /// unchanged. Deleted once every rasteriser has migrated onto
    /// [`CellSurface`] itself; never use it in new code.
    ///
    /// Default: `None` — correct for a surface with no backing
    /// `ratatui::buffer::Buffer` at all.
    #[doc(hidden)]
    #[cfg(feature = "tui")]
    fn ratatui_buffer(&mut self) -> Option<&mut ratatui::buffer::Buffer> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Implements every required method as a no-op/empty-answer, so the
    /// two defaults (`emit_graphics`, `ratatui_buffer`) run their real
    /// trait body rather than an override — proving what a surface with
    /// no out-of-band graphics channel and no backing `ratatui::Buffer`
    /// reports by default.
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

    #[cfg(feature = "tui")]
    #[test]
    fn ratatui_buffer_default_is_none() {
        let mut surface = DummySurface;
        assert!(surface.ratatui_buffer().is_none());
    }
}
