//! Shared terminal-cell rendering helpers consumed by every rasteriser
//! that paints [`crate::primitives::terminal::Terminal`] cell grids
//! (`tui`, `gtk`, `macos` today — `win`'s terminal rasteriser still
//! carries its own copy, see the note at the bottom of this doc comment).
//!
//! Unconditionally compiled (no feature gate), matching [`crate::theme`]
//! and [`crate::text_util`]: the logic here has no platform dependency,
//! only [`crate::primitives::terminal::TerminalCell`] and
//! [`crate::theme::Theme`].
//!
//! # Overlay ladder (#500)
//!
//! Before this module, `tui/terminal.rs`, `gtk/terminal.rs`, and
//! `macos/terminal.rs` each carried their own copy of the same
//! cursor → find-active → find-match → selection precedence ladder, with
//! the two find-highlight colours hardcoded as magic RGB literals in
//! three places instead of one. [`resolve_cell_style`] is now the single
//! definition; the colours live on [`Theme`] as methods
//! ([`Theme::find_active_bg`], [`Theme::find_active_fg`],
//! [`Theme::find_match_bg`]) rather than fields — see the "adding a
//! field here is a breaking change" note on `Theme` itself and the #620
//! precedent it documents. A method costs nothing downstream; a new
//! field would be `error[E0063]: missing field` in `coord-tui`'s
//! exhaustive palette literals on their very next build.
//!
//! # Wide-glyph advance (#500, fix vehicle for #440's macOS half)
//!
//! [`crate::terminal_engine::TerminalSession::to_terminal`] builds its
//! cell grid straight from vt100's model: a double-width character
//! (CJK, emoji, ...) occupies its own column plus one trailing
//! "continuation" column that vt100 reports as an empty cell. A
//! *pixel*-based rasteriser (GTK, macOS — anything that doesn't map one
//! terminal column to one grid cell like TUI does) must claim that
//! continuation column as part of the wide glyph's box, or the
//! continuation column's own (possibly different) background paints
//! over the glyph's right half. That was #439's original GTK bug; #440
//! tracked the same defect on macOS, which advanced a flat `char_width`
//! per column with no wide-glyph awareness at all.
//! [`wide_cell_advance`] is the shared classification both rasterisers
//! now use.
//!
//! **TUI does not call this** — its `cells[row][col]` grid already maps
//! 1:1 to terminal columns, so writing the wide glyph into its column
//! and the vt100-supplied blank into the next column (exactly what
//! [`crate::tui::terminal::draw_terminal`] already does) is already
//! correct with no special-casing: `ratatui::buffer::Buffer`'s own
//! diff/render logic understands multi-width symbols from the glyph's
//! own `cell_width()` and skips re-emitting a blank continuation column
//! it didn't ask for.
//!
//! # Wide-glyph scale (#500 GTK-only → shared by #703)
//!
//! [`wide_cell_advance`] settles the *box* a double-width glyph gets;
//! it says nothing about how well the glyph fills it. A pixel-based
//! rasteriser's fallback font for CJK / emoji typically lays the glyph
//! out at its own natural advance, which is rarely exactly two cells —
//! a CJK glyph might measure 15px in an 18px (2 × 9px) box, some
//! colour-emoji fonts overshoot it. [`wide_glyph_x_scale`] is the
//! horizontal scale factor that stretches or shrinks the glyph to fill
//! the box exactly, so consecutive wide glyphs pack tightly with no
//! ragged inter-glyph gap. GTK's `wide_glyph_x_scale` (#439 follow-up)
//! was the first and, until #703, only implementation; this is that
//! same function, lifted rather than re-described so `macos` and `win`
//! stop needing their own copy of the decision. Each backend still owns
//! *applying* the scale (Cairo's `cr.scale`, Core Text's text-matrix `a`
//! component via `macos::text::draw_text_scaled_x`, Direct2D's
//! render-target transform via `win::text::with_horizontal_scale`) —
//! that part is unavoidably native-handle-shaped and stays put.
//!
//! # Divider geometry (#703)
//!
//! `gtk`, `macos`, and `win` each painted a terminal-split divider as a
//! hardcoded "1 unit wide, from `(x, y)` down to `y + height`"
//! rectangle — the same three magic numbers typed three times, and
//! `win`'s free function additionally diverged in signature (`rect:
//! Rect` instead of `x, y, height`). [`divider_geometry`] is the single
//! definition of that rectangle; each backend's `draw_terminal_divider`
//! now only computes it and hands the four numbers to its own paint
//! primitive (`cr.rectangle` / `CGContextFillRect` / `FillRectangle`).

use crate::primitives::terminal::TerminalCell;
use crate::text_util::is_wide_char;
use crate::theme::Theme;
use crate::types::Color;

/// Resolve the `(background, foreground)` colours to paint one terminal
/// cell, applying the cursor / find / selection overlay precedence
/// ladder once for every backend.
///
/// Precedence, highest first:
/// 1. `is_cursor` — invert: bg becomes the cell's own fg, fg becomes the
///    cell's own bg.
/// 2. `is_find_active` — bg becomes [`Theme::find_active_bg`], fg
///    becomes [`Theme::find_active_fg`].
/// 3. `is_find_match` — bg becomes [`Theme::find_match_bg`]; fg is left
///    as the cell's own.
/// 4. `selected` — bg becomes `theme.selection_bg`; fg is left as the
///    cell's own.
/// 5. none of the above — the cell's own `bg` / `fg`, unchanged.
pub fn resolve_cell_style(cell: &TerminalCell, theme: &Theme) -> (Color, Color) {
    if cell.is_cursor {
        (cell.fg, cell.bg)
    } else if cell.is_find_active {
        (theme.find_active_bg(), theme.find_active_fg())
    } else if cell.is_find_match {
        (theme.find_match_bg(), cell.fg)
    } else if cell.selected {
        (theme.selection_bg, cell.fg)
    } else {
        (cell.bg, cell.fg)
    }
}

/// Pixel box width and grid-column stride for one cell in a pixel-based
/// rasteriser's per-row paint loop.
///
/// Returns `(cell_w, cols_advanced)`. When `ch` classifies as a wide
/// glyph (double-width per [`crate::text_util::is_wide_char`]), it
/// claims its own column plus the following vt100-supplied blank
/// continuation column: `cell_w = char_width * 2.0`, `cols_advanced =
/// 2`. Every other cell advances by exactly one column at `char_width`.
///
/// Callers walk a row with an index (`while col < row.len()`), painting
/// the background across `cell_w` and then stepping `col += cols`, so
/// the continuation column is claimed rather than independently
/// painted on top of the glyph — see this module's doc comment. Not
/// used by the TUI rasteriser.
pub fn wide_cell_advance(ch: char, char_width: f64) -> (f64, usize) {
    if is_wide_char(ch) {
        (char_width * 2.0, 2)
    } else {
        (char_width, 1)
    }
}

/// Horizontal scale factor to draw a double-width glyph so it fills its
/// two-column (`cell_w`) box exactly.
///
/// `natural_w` is the glyph's laid-out width in the caller's native unit
/// (Pango pixels, Core Text points, DirectWrite DIPs — all `f64` here,
/// callers cast as needed). Returns `cell_w / natural_w` so the rendered
/// glyph spans exactly two cells — stretching a narrow CJK glyph out to
/// the full box and shrinking an over-wide emoji back into it. Returns
/// `1.0` (no scaling) when `natural_w` is non-positive (empty /
/// zero-advance layout) so callers never divide by zero or blow a
/// degenerate glyph up to infinity.
///
/// Originally GTK-only (`gtk::terminal`'s private `wide_glyph_x_scale`,
/// #439 follow-up); lifted here unchanged so `macos` and `win` can apply
/// the same decision instead of leaving wide glyphs unscaled (#703).
pub fn wide_glyph_x_scale(natural_w: f64, cell_w: f64) -> f64 {
    if natural_w <= 0.0 {
        1.0
    } else {
        cell_w / natural_w
    }
}

/// Geometry for a terminal-split divider line: a hairline rectangle
/// exactly one native unit wide (1 px for GTK/Direct2D DIPs, 1pt for
/// Core Text), spanning from `(x, y)` down to `y + height`.
///
/// Shared by every pixel-based rasteriser's `draw_terminal_divider` free
/// function (`gtk::terminal`, `macos::terminal`, `win::terminal`) so the
/// "1 unit wide" invariant is defined once instead of copied into three
/// near-identical `rectangle` calls (#703). Not used by
/// `tui::terminal::draw_terminal_divider`, which draws a themed `'│'`
/// character cell rather than pixel geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DividerGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Compute the [`DividerGeometry`] for a divider starting at `(x, y)`
/// with the given `height`. Width is always `1.0`.
pub fn divider_geometry(x: f64, y: f64, height: f64) -> DividerGeometry {
    DividerGeometry {
        x,
        y,
        width: 1.0,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
        TerminalCell {
            ch,
            fg,
            bg,
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        }
    }

    #[test]
    fn default_cell_uses_own_colors() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let c = cell('a', fg, bg);
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (bg, fg));
    }

    #[test]
    fn cursor_cell_inverts_fg_bg() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_cursor = true;
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (fg, bg));
    }

    #[test]
    fn find_active_cell_uses_theme_highlight() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_find_active = true;
        let theme = Theme::default();
        assert_eq!(
            resolve_cell_style(&c, &theme),
            (theme.find_active_bg(), theme.find_active_fg())
        );
    }

    #[test]
    fn find_match_cell_keeps_own_fg() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_find_match = true;
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (theme.find_match_bg(), fg));
    }

    #[test]
    fn selected_cell_uses_theme_selection_bg_and_own_fg() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.selected = true;
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (theme.selection_bg, fg));
    }

    #[test]
    fn cursor_takes_precedence_over_every_other_overlay() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_cursor = true;
        c.is_find_active = true;
        c.is_find_match = true;
        c.selected = true;
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (fg, bg));
    }

    #[test]
    fn find_active_takes_precedence_over_find_match_and_selected() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_find_active = true;
        c.is_find_match = true;
        c.selected = true;
        let theme = Theme::default();
        assert_eq!(
            resolve_cell_style(&c, &theme),
            (theme.find_active_bg(), theme.find_active_fg())
        );
    }

    #[test]
    fn narrow_char_advances_one_column() {
        assert_eq!(wide_cell_advance('a', 10.0), (10.0, 1));
        assert_eq!(wide_cell_advance(' ', 10.0), (10.0, 1));
    }

    #[test]
    fn wide_char_advances_two_columns_at_double_width() {
        assert_eq!(wide_cell_advance('日', 10.0), (20.0, 2));
        assert_eq!(wide_cell_advance('中', 9.0), (18.0, 2));
    }

    // ── wide_glyph_x_scale (#500, lifted from GTK by #703) ─────────────

    /// A CJK glyph measuring 15px inside an 18px (2 × 9px) box → 1.2×.
    #[test]
    fn narrow_wide_glyph_is_stretched_to_fill_two_cells() {
        let cell_w = 18.0;
        let scale = wide_glyph_x_scale(15.0, cell_w);
        assert!(
            (scale - 1.2).abs() < 1e-9,
            "15px glyph in an 18px box should scale 1.2x, got {scale}"
        );
        assert!((15.0 * scale - cell_w).abs() < 1e-9);
    }

    /// A wide glyph that already fills its box (emoji measuring exactly
    /// two cells) is left untouched — scale factor 1.0.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// A wide glyph *wider* than two cells (some colour-emoji fonts) is
    /// shrunk back into the box so it can't overlap the next glyph.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    /// A zero / negative advance (empty or degenerate layout) must not
    /// divide by zero or explode — it falls back to no scaling.
    #[test]
    fn degenerate_glyph_width_falls_back_to_no_scale() {
        assert_eq!(wide_glyph_x_scale(0.0, 18.0), 1.0);
        assert_eq!(wide_glyph_x_scale(-3.0, 18.0), 1.0);
    }

    // ── divider_geometry (#703) ─────────────────────────────────────────

    #[test]
    fn divider_geometry_is_one_unit_wide() {
        let g = divider_geometry(50.0, 5.0, 24.0);
        assert_eq!(
            g,
            DividerGeometry {
                x: 50.0,
                y: 5.0,
                width: 1.0,
                height: 24.0,
            }
        );
    }
}
