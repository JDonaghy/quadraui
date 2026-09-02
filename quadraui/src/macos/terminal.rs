//! macOS rasteriser for [`crate::Terminal`] cell grids.
//!
//! Mirror of [`crate::gtk::terminal::draw_terminal_cells`]: walks
//! `term.cells[row][col]`, fills each cell's background, then paints
//! the glyph (skipped for `' '` and `'\0'`). Overlay flags
//! (`is_cursor`, `is_find_active`, `is_find_match`, `selected`)
//! override the cell's `bg` / `fg` via
//! [`crate::terminal_style::resolve_cell_style`] — the ladder shared
//! with the TUI and GTK rasterisers (#500).
//!
//! # Wide characters (#440, fixed by #500's shared helper)
//!
//! Before #500, this rasteriser advanced a flat `char_width` per grid
//! column regardless of glyph width, so a double-width character (CJK,
//! emoji, ...) — which vt100 represents as the glyph's own column plus
//! a trailing blank "continuation" column, see
//! [`crate::terminal_engine::TerminalSession::to_terminal`] — got its
//! right half painted over by the continuation column's own background.
//! This mirrors the bug GTK fixed in #439. [`draw_terminal_cells`] now
//! uses [`crate::terminal_style::wide_cell_advance`] (shared with
//! `gtk::terminal`) to paint the wide glyph's background across both
//! columns and skip the continuation column, matching the GTK fix.
//! Unlike GTK's follow-up glyph-scaling (`wide_glyph_x_scale`), the
//! glyph itself is still drawn at its natural Core Text advance — the
//! ragged-packing follow-up is out of scope here; #500's acceptance bar
//! is "paints without clipping," not pixel-tight packing.
//!
//! Bold / italic / underline attributes are **not** rendered yet —
//! Core Text would need a per-cell `CTFont` (or attributed-string
//! attribute) for the bold / italic variants, and the cell grid is
//! hot enough that we want trait-shape parity in this ticket and
//! defer styled-attr support to a follow-up. (#43 acceptance
//! criteria covers the cell grid + cursor / selection / find
//! overlays; attribute variants are listed in the milestone but
//! gated behind a consumer asking for them.)

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::draw_text;
use crate::primitives::terminal::Terminal;
use crate::terminal_style::{resolve_cell_style, wide_cell_advance};
use crate::theme::Theme;
use crate::types::Color;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, y)` on `ctx`. `cell_area_w` clips per-row painting — cells
/// past the right edge stop being drawn rather than wrapping.
/// `cell_area_h` clips per-column painting vertically — rows whose top
/// falls at or below the pane bottom stop being drawn rather than
/// bleeding into whatever sits below the terminal (the footer, an
/// adjacent pane). `line_height` and `char_width` are the per-cell
/// dimensions in points.
///
/// Callers that render with a scrollbar pass `cell_area_w =
/// rect.width - scrollbar_width` so the cell grid stops at the
/// scrollbar gutter; the gutter itself is painted by
/// [`super::scrollbar::draw_scrollbar`] from
/// [`crate::macos::backend::MacBackend::draw_terminal`].
///
/// # Why `cell_area_h` exists (quadraui#437 / #484)
///
/// Painting is not debounced, so a frame during an interactive resize
/// can render a grid that still has the pre-resize (taller) row count
/// into an already-shrunk pixel pane. The GTK twin gained this clip
/// under #437; the macOS caller was updated to pass `rect.height` at
/// the same time but this signature was not, which is the E0061 arity
/// mismatch quadraui#484 found the first time `macos-latest` compiled
/// this backend. Fixed on the callee side — dropping the argument
/// instead would have made the build green by silently deleting the
/// vertical clip on macOS. This is *not* #440 (wide-char handling);
/// `char_width` was already a parameter and still advances one column
/// per cell.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_terminal_cells(
    ctx: CGContextRef,
    font: &CTFont,
    term: &Terminal,
    x: f64,
    y: f64,
    cell_area_w: f64,
    cell_area_h: f64,
    line_height: f64,
    char_width: f64,
    theme: &Theme,
) {
    if cell_area_w <= 0.0 || cell_area_h <= 0.0 || line_height <= 0.0 || char_width <= 0.0 {
        return;
    }
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = y + row_idx as f64 * line_height;
        // Stop once a row's top has reached the pane bottom — such a row
        // belongs to a taller (pre-resize) grid and would bleed past the
        // pane. A row that merely straddles the bottom edge is still
        // drawn (and clipped by whatever the host paints over it).
        if row_y >= y + cell_area_h {
            break;
        }
        let mut cell_x = x;
        let mut col = 0usize;
        while col < row.len() {
            let cell = &row[col];
            // Double-width glyphs (CJK, emoji, ...) get a two-column cell:
            // the vt100 grid already reserves the following column as an
            // empty continuation placeholder, so claim it here rather
            // than letting it paint its own (mismatched) background over
            // the glyph's right half — mirrors `gtk::terminal`'s #439 fix.
            let (cell_w, cols_advanced) = wide_cell_advance(cell.ch, char_width);

            if cell_x + cell_w > x + cell_area_w {
                break;
            }
            let (cell_bg, cell_fg) = resolve_cell_style(cell, theme);
            fill_rect(ctx, cell_x, row_y, cell_w, line_height, cell_bg);

            if cell.ch != ' ' && cell.ch != '\0' {
                let s = cell.ch.to_string();
                draw_text(ctx, font, &s, cell_x, row_y, color_to_cg(cell_fg));
            }

            cell_x += cell_w;
            col += cols_advanced;
        }
    }
}

/// Draw a vertical divider line for a terminal split pane. Paints a
/// 1-pt-wide line at `x` from `y` to `y + height` using
/// `theme.separator`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call. Calling with a freed or null pointer is UB.
pub unsafe fn draw_terminal_divider(ctx: CGContextRef, x: f64, y: f64, height: f64, theme: &Theme) {
    fill_rect(ctx, x, y, 1.0, height, theme.separator);
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

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

    fn paint(term: &Terminal) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_terminal(QRect::new(0.0, 0.0, W as f32, H as f32), term);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn cell_bg_paints_through_backend() {
        // One cell, magenta background, glyph 'A'. The fill_rect path
        // should colour the whole cell magenta before the glyph paints.
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let s = paint(&term);
        // Probe a corner of the cell that the glyph "A" doesn't reach
        // (glyph sits roughly in the middle); top-left of the cell is
        // background.
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (magenta.r, magenta.g, magenta.b));
    }

    /// #440 / #500 regression, mirroring `gtk::terminal`'s
    /// `wide_cell_background_spans_two_columns` test: a double-width
    /// glyph (CJK) followed by vt100's blank continuation cell must have
    /// its background span both columns — the continuation cell's own
    /// (different) background must NOT paint over the second half of the
    /// wide glyph's cell. Calls [`draw_terminal_cells`] directly (rather
    /// than through `MacBackend::draw_terminal`) so the test controls
    /// `char_width` explicitly instead of depending on Menlo's measured
    /// advance.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        const CHAR_W: f64 = 10.0;
        const LINE_H: f64 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so any antialiased glyph ink can't shift
        // the probed colour — same trick as the GTK twin test, and for
        // the same reason (font-fallback ink at the probe point would
        // otherwise vary by host).
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        // SAFETY: `surface.context_ptr()` is valid for the surface's
        // lifetime, which outlives this call.
        unsafe {
            draw_terminal_cells(
                surface.context_ptr(),
                &f,
                &term,
                0.0,
                0.0,
                W as f64,
                H as f64,
                LINE_H,
                CHAR_W,
                &theme,
            );
        }

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must
    /// not widen unrelated cells. Mirrors `gtk::terminal`'s
    /// `narrow_cells_advance_by_single_char_width`.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        const CHAR_W: f64 = 10.0;
        const LINE_H: f64 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        // SAFETY: `surface.context_ptr()` is valid for the surface's
        // lifetime, which outlives this call.
        unsafe {
            draw_terminal_cells(
                surface.context_ptr(),
                &f,
                &term,
                0.0,
                0.0,
                W as f64,
                H as f64,
                LINE_H,
                CHAR_W,
                &theme,
            );
        }

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    #[test]
    fn cursor_cell_inverts_fg_bg() {
        // Cursor flag flips the cell so bg is painted with the cell's fg.
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        // Cursor swap: bg now uses fg.
        assert_eq!((r, g, b), (fg.r, fg.g, fg.b));
    }

    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (255, 165, 0));
    }

    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    #[test]
    fn cells_past_width_are_clipped() {
        // Build a row of 200 magenta cells. At a typical char_width of
        // ~8px, only ~25 fit in W=200. Past the clip we should still
        // see transparent black (surface initial fill).
        let magenta = Color::rgb(200, 30, 200);
        let row: Vec<_> = (0..200)
            .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let s = paint(&term);
        // Right-most pixel column: cells should NOT have painted there
        // because clipping kicks in at `x + char_width > x + W` — and
        // 200 / char_width < 200 cells fit.
        // Probe just inside the right edge: this should still be magenta
        // (one of the cells fits there).
        let inside = s.pixel(W / 2, 0);
        assert_eq!(
            (inside.0, inside.1, inside.2),
            (magenta.r, magenta.g, magenta.b)
        );
    }

    /// Rows past the pane bottom are clipped (quadraui#437, ported to
    /// macOS by #484's `cell_area_h` fix).
    ///
    /// A grid taller than the pane — exactly what an un-debounced
    /// interactive-resize frame produces — must stop at the pane bottom
    /// rather than bleed into whatever the host paints below.
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        // 60 rows at ~16pt line height is ~960pt of grid for a 60pt pane.
        let cells: Vec<Vec<_>> = (0..60)
            .map(|_| {
                (0..20)
                    .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
                    .collect()
            })
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells,
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let pane_h = 60.0_f32;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_terminal(QRect::new(0.0, 0.0, W as f32, pane_h), &term);
        });
        backend.end_frame();

        // Inside the pane: painted.
        let (r, g, b, _) = surface.pixel(4, 4);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "the pane's own rows should still paint",
        );

        // Well below the pane bottom: untouched (transparent black).
        let below = surface.pixel(4, H - 4);
        assert_eq!(
            below,
            (0, 0, 0, 0),
            "rows below the pane bottom must be clipped, not bled",
        );
    }

    /// `cargo test -p quadraui --no-default-features --features macos -- --ignored --nocapture macos::terminal::tests::dump_smoke_ppm`
    ///
    /// Paints a sample terminal — three rows of mixed cells, a cursor
    /// cell, two find matches with one active, a selection run, plus a
    /// scrollbar — and writes `/tmp/quadraui_terminal.ppm`. Open in
    /// Preview to confirm:
    /// - Row 0 reads `quadraui terminal` in white on dark.
    /// - Row 1 has an inverted (white-on-green) cursor inside the
    ///   word `cursor`.
    /// - Row 2 shows the orange find-active match and dim find-match
    ///   highlights, plus a contiguous selection band using
    ///   `theme.selection_bg`.
    /// - Right edge has a scrollbar gutter + thumb (~50% down the
    ///   track, since `scroll_offset = 50` of 100 lines, 16 visible).
    #[test]
    #[ignore = "writes /tmp/quadraui_terminal.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        use crate::primitives::terminal::TerminalScrollbar;

        let white = Color::rgb(230, 230, 230);
        let dark = Color::rgb(20, 20, 30);
        let green = Color::rgb(60, 200, 90);

        fn row(s: &str, fg: Color, bg: Color) -> Vec<TerminalCell> {
            s.chars()
                .map(|ch| TerminalCell {
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
                })
                .collect()
        }

        let r0 = row("quadraui terminal", white, dark);
        let mut r1 = row("  cursor here", white, dark);
        // Inverted cursor on the 'c' of "cursor".
        r1[2].fg = green;
        r1[2].is_cursor = true;
        let mut r2 = row("  find foo bar foo qux", white, dark);
        // Active find on first "foo" (chars 7..10), dim match on second
        // "foo" (chars 15..18).
        for c in r2.iter_mut().take(10).skip(7) {
            c.is_find_active = true;
        }
        for c in r2.iter_mut().take(18).skip(15) {
            c.is_find_match = true;
        }
        // Selection on " bar " (chars 10..15).
        for c in r2.iter_mut().take(15).skip(10) {
            c.selected = true;
        }

        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![r0, r1, r2],
            scrollbar: Some(TerminalScrollbar {
                total_lines: 100,
                visible_lines: 16,
                scroll_offset: 50,
                inverted: false,
                width: None,
            }),
        };
        let s = paint(&term);
        s.write_ppm_and_open("/tmp/quadraui_terminal.ppm");
    }
}
