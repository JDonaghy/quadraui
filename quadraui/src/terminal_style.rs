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

use crate::primitives::terminal::{TerminalCell, TerminalCursorShape};
use crate::text_util::display_width;
use crate::theme::Theme;
use crate::types::Color;
use crate::Rect;

/// Resolve the `(background, foreground)` colours to paint one terminal
/// cell, applying the cursor / find / selection overlay precedence
/// ladder once for every backend.
///
/// Precedence, highest first:
/// 1. `is_cursor` with [`TerminalCursorShape::Block`] (quadraui#338) —
///    invert: bg becomes the cell's own fg, fg becomes the cell's own
///    bg. `dim` is ignored here — the cursor block is already a strong
///    visual overlay, so faint text under it would be contradictory
///    (full-intensity fg suddenly reads as low-contrast). Gated on
///    [`cursor_blink_visible`]: a blinking cursor cell falls through to
///    the rest of the ladder during its "off" phase instead of always
///    inverting.
/// 2. `is_find_active` — bg becomes [`Theme::find_active_bg`], fg
///    becomes [`Theme::find_active_fg`].
/// 3. `is_find_match` — bg becomes [`Theme::find_match_bg`]; fg is left
///    as the cell's own.
/// 4. `selected` — bg becomes `theme.selection_bg`; fg is left as the
///    cell's own.
/// 5. none of the above — the cell's own `bg` / `fg`, unchanged.
///
/// A cursor with [`TerminalCursorShape::Underline`] or
/// [`TerminalCursorShape::Bar`] does **not** short-circuit this ladder —
/// unlike `Block`, inverting the *whole* cell for a thin accent would
/// just look like another solid block. Those two shapes instead fall
/// through to steps 2-5 for colour, and the accent itself is painted
/// separately: pixel-based rasterisers via [`cursor_accent_rect`] (see
/// [`crate::primitives::terminal`]'s `paint`), the TUI rasteriser via a
/// best-effort `Modifier::UNDERLINED` overlay (`tui::terminal::draw_terminal`)
/// — see that primitive's "Embedded TUI applicability" note on why TUI
/// can't distinguish `Bar` from `Underline` in a character-cell grid.
///
/// After the ladder above resolves `(bg, fg)`, `cell.dim` (SGR 2,
/// faint — quadraui#345) blends `fg` 50% toward the *resolved* `bg`
/// (not necessarily the cell's own `bg`; a dim cell inside a selection
/// or find highlight still fades toward that overlay's background) via
/// [`dim_fg`]. This is folded into colour resolution rather than
/// exposed as a per-backend text-run attribute like bold/italic/
/// underline: faint has no font-weight equivalent, and blending toward
/// the resolved background is theme-correct on both dark and light
/// themes with zero new `PaintSurface` surface area.
pub fn resolve_cell_style(cell: &TerminalCell, theme: &Theme) -> (Color, Color) {
    if cell.is_cursor
        && cell.cursor_shape == TerminalCursorShape::Block
        && cursor_blink_visible(cell.cursor_blinking)
    {
        return (cell.fg, cell.bg);
    }

    let (bg, fg) = if cell.is_find_active {
        (theme.find_active_bg(), theme.find_active_fg())
    } else if cell.is_find_match {
        (theme.find_match_bg(), cell.fg)
    } else if cell.selected {
        (theme.selection_bg, cell.fg)
    } else {
        (cell.bg, cell.fg)
    };

    if cell.dim {
        (bg, dim_fg(fg, bg))
    } else {
        (bg, fg)
    }
}

/// Half-period, in milliseconds, of a blinking cursor's on/off cycle —
/// used by [`cursor_blink_visible`]. Matches the ~530ms interval common
/// among real terminal emulators (e.g. xterm's default `cursorBlinkXOR`
/// timing) closely enough that the two are visually indistinguishable;
/// exact parity isn't required since DECSCUSR itself never specifies a
/// rate.
const CURSOR_BLINK_HALF_PERIOD_MS: u128 = 530;

/// Whether a cursor overlay should be visible *right now*.
///
/// `blinking = false` (the common case — most programs never send a
/// blinking DECSCUSR variant) always returns `true`: steady cursors are
/// always on. `blinking = true` reads the wall clock and toggles every
/// [`CURSOR_BLINK_HALF_PERIOD_MS`], so repeated calls across paint
/// frames produce a real blink with no state threaded through
/// `Terminal`/`TerminalCell` — the "paint-time toggle driven by the
/// runtime tick" this primitive's module doc describes (quadraui#338).
///
/// Deliberately impure (reads [`std::time::SystemTime::now`]) — callers
/// that need a deterministic snapshot for a test should construct a
/// `TerminalCell` with `cursor_blinking: false` instead of trying to
/// pin the wall clock.
pub fn cursor_blink_visible(blinking: bool) -> bool {
    if !blinking {
        return true;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (now_ms / CURSOR_BLINK_HALF_PERIOD_MS).is_multiple_of(2)
}

/// Whether a pixel-based rasteriser should paint a separate accent shape
/// (underline / bar) for `cell` right now — `true` only for a cursor
/// cell whose shape isn't `Block` (which is rendered entirely through
/// [`resolve_cell_style`]'s colour invert) and that isn't mid-blink-off.
pub fn cursor_accent_visible(cell: &TerminalCell) -> bool {
    cell.is_cursor
        && cell.cursor_shape != TerminalCursorShape::Block
        && cursor_blink_visible(cell.cursor_blinking)
}

/// Thickness, in surface-native units (px for GTK/Direct2D/Core
/// Graphics), of a bar/underline cursor accent — thin enough to read as
/// a line rather than a second block at typical terminal font sizes.
pub const CURSOR_ACCENT_THICKNESS: f32 = 2.0;

/// Geometry for a bar/underline cursor accent inside one cell's box,
/// for pixel-based rasterisers (GTK/macOS/win, via
/// [`crate::primitives::terminal`]'s shared `paint`). `cell_x`/`cell_y`
/// are the cell's top-left corner, `cell_w`/`line_height` its box size
/// — the same values the caller already computed for the background
/// fill. Returns `None` for [`TerminalCursorShape::Block`] (no separate
/// accent; see [`resolve_cell_style`]).
pub fn cursor_accent_rect(
    shape: TerminalCursorShape,
    cell_x: f32,
    cell_y: f32,
    cell_w: f32,
    line_height: f32,
) -> Option<Rect> {
    match shape {
        TerminalCursorShape::Block => None,
        TerminalCursorShape::Underline => Some(Rect::new(
            cell_x,
            cell_y + (line_height - CURSOR_ACCENT_THICKNESS).max(0.0),
            cell_w,
            CURSOR_ACCENT_THICKNESS.min(line_height),
        )),
        TerminalCursorShape::Bar => Some(Rect::new(
            cell_x,
            cell_y,
            CURSOR_ACCENT_THICKNESS.min(cell_w),
            line_height,
        )),
    }
}

/// Blend `fg` 50% toward `bg`, per-channel. Used by [`resolve_cell_style`]
/// to render SGR 2 (faint/dim) text — quadraui#345. Blending toward the
/// background (rather than toward black, which is what some terminals do)
/// keeps faint text legibly faded on light themes too, where darkening
/// would instead *increase* contrast.
fn dim_fg(fg: Color, bg: Color) -> Color {
    Color::rgba(
        ((fg.r as u16 + bg.r as u16) / 2) as u8,
        ((fg.g as u16 + bg.g as u16) / 2) as u8,
        ((fg.b as u16 + bg.b as u16) / 2) as u8,
        fg.a,
    )
}

/// Pixel box width and grid-column stride for one cell in a pixel-based
/// rasteriser's per-row paint loop.
///
/// Returns `(cell_w, cols_advanced)`. When `text` (the cell's full
/// grapheme cluster, [`TerminalCell::text`]) classifies as a wide glyph
/// (double-width per [`crate::text_util::display_width`]), it claims its
/// own column plus the following vt100-supplied blank continuation
/// column: `cell_w = char_width * 2.0`, `cols_advanced = 2`. Every other
/// cell advances by exactly one column at `char_width`. `display_width`
/// (rather than measuring only the first `char`) is what keeps this
/// correct for a grapheme cluster — a wide base character followed by
/// zero-width combining marks still sums to 2, and a narrow base
/// character with trailing combining marks still sums to 1
/// (quadraui#337).
///
/// Callers walk a row with an index (`while col < row.len()`), painting
/// the background across `cell_w` and then stepping `col += cols`, so
/// the continuation column is claimed rather than independently
/// painted on top of the glyph — see this module's doc comment. Not
/// used by the TUI rasteriser.
pub fn wide_cell_advance(text: &str, char_width: f64) -> (f64, usize) {
    if display_width(text) >= 2 {
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

    // ── dim / faint (SGR 2, quadraui#345) ───────────────────────────────

    /// A dim cell's foreground blends 50% toward its own background —
    /// this is the core regression test for #345: faint text must no
    /// longer render at full brightness.
    #[test]
    fn dim_cell_blends_fg_halfway_to_bg() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell('a', fg, bg);
        c.dim = true;
        let theme = Theme::default();
        let (resolved_bg, resolved_fg) = resolve_cell_style(&c, &theme);
        assert_eq!(resolved_bg, bg, "dim must not change the background");
        assert_eq!(resolved_fg, Color::rgb(100, 100, 100));
    }

    /// A non-dim cell is completely unaffected by the new field — no
    /// regression for the overwhelming majority of cells that never set
    /// SGR 2.
    #[test]
    fn non_dim_cell_is_unaffected() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let c = cell('a', fg, bg);
        assert!(!c.dim);
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (bg, fg));
    }

    /// `dim` blends toward whichever background the overlay ladder
    /// already picked — here the selection highlight — not the cell's
    /// own (different) background, so faint text inside a selection
    /// still fades toward the highlight rather than the pane's base bg.
    #[test]
    fn dim_blends_toward_resolved_overlay_background_not_cells_own_bg() {
        let fg = Color::rgb(200, 200, 200);
        let cell_bg = Color::rgb(0, 0, 0);
        let mut c = cell('a', fg, cell_bg);
        c.dim = true;
        c.selected = true;
        let theme = Theme::default();
        let (resolved_bg, resolved_fg) = resolve_cell_style(&c, &theme);
        assert_eq!(resolved_bg, theme.selection_bg);
        let expected_fg = Color::rgb(
            ((fg.r as u16 + theme.selection_bg.r as u16) / 2) as u8,
            ((fg.g as u16 + theme.selection_bg.g as u16) / 2) as u8,
            ((fg.b as u16 + theme.selection_bg.b as u16) / 2) as u8,
        );
        assert_eq!(resolved_fg, expected_fg);
    }

    /// Cursor precedence still wins over `dim` — an inverted cursor cell
    /// renders at full contrast, not faded (see `resolve_cell_style`'s
    /// doc for why blending under an inverted cursor would be
    /// contradictory).
    #[test]
    fn dim_does_not_apply_under_cursor_overlay() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.dim = true;
        c.is_cursor = true;
        let theme = Theme::default();
        assert_eq!(resolve_cell_style(&c, &theme), (fg, bg));
    }

    // ── Cursor shape (quadraui#338) ─────────────────────────────────────

    /// `Underline`/`Bar` cursor cells do NOT invert — unlike `Block`,
    /// they fall through to the rest of the overlay ladder (here: no
    /// other overlay set, so the cell's own colours). The shape's visual
    /// distinctness comes from the separate accent painted by
    /// `cursor_accent_rect`/the TUI underline overlay, not a colour
    /// change here.
    #[test]
    fn underline_and_bar_cursor_do_not_invert() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let theme = Theme::default();

        let mut underline = cell('a', fg, bg);
        underline.is_cursor = true;
        underline.cursor_shape = TerminalCursorShape::Underline;
        assert_eq!(resolve_cell_style(&underline, &theme), (bg, fg));

        let mut bar = cell('a', fg, bg);
        bar.is_cursor = true;
        bar.cursor_shape = TerminalCursorShape::Bar;
        assert_eq!(resolve_cell_style(&bar, &theme), (bg, fg));
    }

    /// Unlike `Block`, a non-inverting cursor shape lets the rest of the
    /// ladder apply — a `Bar` cursor sitting on a find-active match still
    /// shows the find-active highlight for colour purposes.
    #[test]
    fn bar_cursor_does_not_block_lower_precedence_overlays() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);
        let mut c = cell('a', fg, bg);
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Bar;
        c.is_find_active = true;
        let theme = Theme::default();
        assert_eq!(
            resolve_cell_style(&c, &theme),
            (theme.find_active_bg(), theme.find_active_fg())
        );
    }

    /// A steady (non-blinking) cursor is always visible, regardless of
    /// wall-clock time — `cursor_blink_visible(false)` is a constant
    /// `true`, keeping every existing (non-blinking) call site's
    /// behaviour exactly unchanged.
    #[test]
    fn steady_cursor_always_blink_visible() {
        assert!(cursor_blink_visible(false));
        assert!(cursor_blink_visible(false));
    }

    /// `cursor_accent_visible` is `false` for a `Block` cursor (painted
    /// entirely via colour invert, no separate accent) and for any
    /// non-cursor cell, `true` for a steady `Bar`/`Underline` cursor.
    #[test]
    fn cursor_accent_visible_gates_on_shape_and_is_cursor() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(10, 10, 10);

        let mut not_cursor = cell('a', fg, bg);
        not_cursor.cursor_shape = TerminalCursorShape::Bar;
        assert!(!cursor_accent_visible(&not_cursor));

        let mut block = cell('a', fg, bg);
        block.is_cursor = true;
        assert!(!cursor_accent_visible(&block));

        let mut bar = cell('a', fg, bg);
        bar.is_cursor = true;
        bar.cursor_shape = TerminalCursorShape::Bar;
        assert!(cursor_accent_visible(&bar));

        let mut underline = cell('a', fg, bg);
        underline.is_cursor = true;
        underline.cursor_shape = TerminalCursorShape::Underline;
        assert!(cursor_accent_visible(&underline));
    }

    /// `cursor_accent_rect` returns `None` for `Block` (no separate
    /// accent — see `resolve_cell_style`) and a geometrically sane rect
    /// for `Underline`/`Bar`: a thin strip pinned to the bottom edge for
    /// `Underline`, to the left edge for `Bar`, both fully inside the
    /// cell's box.
    #[test]
    fn cursor_accent_rect_geometry() {
        let (cell_x, cell_y, cell_w, line_h) = (10.0, 20.0, 8.0, 16.0);

        assert_eq!(
            cursor_accent_rect(TerminalCursorShape::Block, cell_x, cell_y, cell_w, line_h),
            None
        );

        let underline = cursor_accent_rect(
            TerminalCursorShape::Underline,
            cell_x,
            cell_y,
            cell_w,
            line_h,
        )
        .expect("Underline has an accent rect");
        assert_eq!(underline.x, cell_x);
        assert_eq!(underline.width, cell_w);
        assert!(underline.y >= cell_y && underline.y + underline.height <= cell_y + line_h);

        let bar = cursor_accent_rect(TerminalCursorShape::Bar, cell_x, cell_y, cell_w, line_h)
            .expect("Bar has an accent rect");
        assert_eq!(bar.x, cell_x);
        assert_eq!(bar.y, cell_y);
        assert_eq!(bar.height, line_h);
        assert!(bar.width <= cell_w);
    }

    /// A degenerate (zero-size) cell box must not produce a negative-size
    /// or out-of-bounds accent rect — `cursor_accent_rect` clamps the
    /// accent thickness to the box itself.
    #[test]
    fn cursor_accent_rect_clamps_to_a_tiny_cell() {
        let underline = cursor_accent_rect(TerminalCursorShape::Underline, 0.0, 0.0, 1.0, 1.0)
            .expect("Underline has an accent rect");
        assert!(underline.height <= 1.0);
        assert!(underline.y >= 0.0);

        let bar = cursor_accent_rect(TerminalCursorShape::Bar, 0.0, 0.0, 1.0, 1.0)
            .expect("Bar has an accent rect");
        assert!(bar.width <= 1.0);
    }

    #[test]
    fn narrow_char_advances_one_column() {
        assert_eq!(wide_cell_advance("a", 10.0), (10.0, 1));
        assert_eq!(wide_cell_advance(" ", 10.0), (10.0, 1));
    }

    #[test]
    fn wide_char_advances_two_columns_at_double_width() {
        assert_eq!(wide_cell_advance("日", 10.0), (20.0, 2));
        assert_eq!(wide_cell_advance("中", 9.0), (18.0, 2));
    }

    /// A narrow base character with a trailing combining mark (a
    /// multi-codepoint grapheme cluster) is still one column — the
    /// combining mark must not be counted as extra width
    /// (quadraui#337).
    #[test]
    fn combining_mark_grapheme_advances_one_column() {
        assert_eq!(wide_cell_advance("e\u{0301}", 10.0), (10.0, 1));
    }

    /// A wide base character followed by zero-width combining marks
    /// still advances two columns.
    #[test]
    fn wide_char_with_combining_mark_advances_two_columns() {
        assert_eq!(wide_cell_advance("日\u{0301}", 10.0), (20.0, 2));
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
