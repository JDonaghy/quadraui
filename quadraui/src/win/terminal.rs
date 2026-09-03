//! Direct2D / DirectWrite rasteriser for [`crate::Terminal`] cell grids
//! (issue #30).
//!
//! Mirrors [`crate::macos::terminal::draw_terminal_cells`]'s structure:
//! walk `term.cells[row][col]`, fill each cell's background, then paint
//! the glyph (skipped for `' '`/`'\0'`). Overlay flags (`is_cursor`,
//! `is_find_active`, `is_find_match`, `selected`) override the cell's
//! `bg`/`fg` via [`crate::terminal_style::resolve_cell_style`] — the
//! ladder shared with `tui`/`gtk`/`macos` (#500; this rasteriser
//! previously carried its own copy with the overlay colours hardcoded as
//! magic RGB literals instead, fixed by #703). Bold cells paint through
//! [`DWrite::draw_text_styled`], since that variant already exists on
//! this backend's `DWrite` (#25). `italic` / `underline` are not yet
//! applied: `DWrite` has no italic text format or underline attribute
//! wired up today (only GTK's Pango `AttrList` does) — a follow-up for
//! whichever consumer needs it.
//!
//! GTK's #417 dirty-row repaint cache is a follow-up optimisation, not
//! required for parity — this rasteriser repaints the whole grid every
//! frame, the same posture `macos::terminal` shipped with.
//!
//! # Wide characters (#500's shared box fix, #703's shared scale)
//!
//! Before #703, this rasteriser advanced a flat `char_width` per grid
//! column with no wide-glyph awareness at all — the same bug GTK fixed
//! in #439 and macOS in #500, except here nobody had fixed it yet, so
//! every double-width character (CJK, emoji, ...) got its right half
//! painted over by vt100's blank "continuation" column (see
//! [`crate::terminal_engine::TerminalSession::to_terminal`]) and its
//! glyph clipped to a single-column-wide `DrawText` layout rect.
//! [`draw_terminal_cells`] now uses
//! [`crate::terminal_style::wide_cell_advance`] to claim the
//! continuation column as part of the wide glyph's box (matching
//! `gtk`/`macos`), and
//! [`crate::terminal_style::wide_glyph_x_scale`] plus
//! [`super::text::with_horizontal_scale`] to stretch or shrink the glyph
//! to fill that box exactly (matching GTK's follow-up and macOS's #703
//! adoption of it) — Direct2D has no per-draw scale parameter, only a
//! render-target-wide transform, hence the dedicated helper.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod terminal;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, with_horizontal_scale, DWrite};
use crate::event::Rect;
use crate::primitives::terminal::Terminal;
use crate::terminal_style::{
    divider_geometry, resolve_cell_style, wide_cell_advance, wide_glyph_x_scale,
};
use crate::theme::Theme;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, y)` on `target`. `cell_area_w` clips per-row painting — cells
/// past the right edge stop being drawn rather than wrapping.
/// `cell_area_h` clips per-column painting vertically — rows whose top
/// falls at or below the pane bottom stop being drawn rather than
/// bleeding into whatever sits below the terminal (mirrors
/// `gtk`/`macos`'s quadraui#437 fix). `line_height` and `char_width` are
/// the per-cell dimensions in DIPs.
///
/// Callers that render with a scrollbar pass `cell_area_w = rect.width -
/// scrollbar_width` so the cell grid stops at the scrollbar gutter; the
/// gutter itself is painted by [`super::scrollbar::draw_scrollbar`] from
/// [`super::backend::WinBackend::draw_terminal`].
#[allow(clippy::too_many_arguments)]
pub fn draw_terminal_cells(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    term: &Terminal,
    x: f32,
    y: f32,
    cell_area_w: f32,
    cell_area_h: f32,
    line_height: f32,
    char_width: f32,
    theme: &Theme,
) {
    if cell_area_w <= 0.0 || cell_area_h <= 0.0 || line_height <= 0.0 || char_width <= 0.0 {
        return;
    }
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = y + row_idx as f32 * line_height;
        // Stop once a row's top has reached the pane bottom — such a row
        // belongs to a taller (pre-resize) grid and would bleed past the
        // pane. A row that merely straddles the bottom edge is still drawn
        // (and clipped by whatever the host paints over it).
        if row_y >= y + cell_area_h {
            break;
        }
        let mut cell_x = x;
        let mut col = 0usize;
        while col < row.len() {
            let cell = &row[col];
            // Double-width glyphs (CJK, emoji, ...) get a two-column
            // cell: the vt100 grid already reserves the following column
            // as an empty continuation placeholder, so claim it here
            // rather than letting it paint its own (mismatched)
            // background over the glyph's right half — mirrors
            // `gtk`/`macos::terminal`'s #439/#500 fix.
            let (cell_w, cols_advanced) = wide_cell_advance(cell.ch, char_width as f64);
            let cell_w = cell_w as f32;
            let is_wide = cols_advanced == 2;

            if cell_x + cell_w > x + cell_area_w {
                break;
            }
            let (cell_bg, cell_fg) = resolve_cell_style(cell, theme);
            let _ = fill_rect(
                target,
                Rect::new(cell_x, row_y, cell_w, line_height),
                cell_bg,
            );

            if cell.ch != ' ' && cell.ch != '\0' {
                let s = cell.ch.to_string();
                let cell_rect = Rect::new(cell_x, row_y, cell_w, line_height);
                if is_wide {
                    // The font DirectWrite falls back to for CJK / emoji
                    // rarely lays the glyph out at exactly two cells —
                    // scale it to fill `cell_w` instead of leaving a
                    // ragged gap or overlap, mirroring `gtk`/`macos`'s
                    // glyph-scaling follow-up (#439 / #500 / #703).
                    let natural_w = dwrite
                        .measure_text_styled(&s, cell.bold)
                        .map(|(w, _)| w)
                        .unwrap_or(cell_w);
                    let scale_x = wide_glyph_x_scale(natural_w as f64, cell_w as f64) as f32;
                    if (scale_x - 1.0).abs() > f32::EPSILON {
                        with_horizontal_scale(target, scale_x, cell_x, || {
                            let _ =
                                dwrite.draw_text_styled(target, &s, cell_rect, cell_fg, cell.bold);
                        });
                    } else {
                        let _ = dwrite.draw_text_styled(target, &s, cell_rect, cell_fg, cell.bold);
                    }
                } else {
                    let _ = dwrite.draw_text_styled(target, &s, cell_rect, cell_fg, cell.bold);
                }
            }

            cell_x += cell_w;
            col += cols_advanced;
        }
    }
}

/// Draw a vertical divider line for a terminal split pane. Paints a
/// 1-DIP-wide line at `x` from `y` to `y + height` using
/// `theme.separator`. Geometry comes from
/// [`crate::terminal_style::divider_geometry`], shared with the
/// `gtk`/`macos` twins (#703) — this signature used to take a whole
/// `Rect` (with `width` silently ignored) while the other two backends
/// took `x, y, height` directly; converged here so all three match.
/// [`super::backend::WinBackend::draw_terminal_divider`] is the call
/// site that adapts the `Backend` trait's `Rect`-shaped parameter down
/// to these three numbers.
pub fn draw_terminal_divider(
    target: &ID2D1RenderTarget,
    x: f32,
    y: f32,
    height: f32,
    theme: &Theme,
) {
    let g = divider_geometry(x as f64, y as f64, height as f64);
    let _ = fill_rect(
        target,
        Rect::new(g.x as f32, g.y as f32, g.width as f32, g.height as f32),
        theme.separator,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 120;
    const CHAR_W: f32 = 10.0;
    const LINE_H: f32 = 20.0;

    fn cell(ch: char, fg: Color, bg: Color) -> crate::primitives::terminal::TerminalCell {
        crate::primitives::terminal::TerminalCell {
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

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0).expect("create DWrite").0
    }

    /// A cell's background must paint at its own grid position before
    /// the glyph is drawn — probing a corner the glyph doesn't reach
    /// isolates the fill from font rasterisation.
    #[test]
    fn cell_bg_paints_at_its_grid_position() {
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (magenta.r, magenta.g, magenta.b));
    }

    /// #703 regression, mirroring `gtk`/`macos::terminal`'s
    /// `wide_cell_background_spans_two_columns`: a double-width glyph
    /// (CJK) followed by vt100's blank continuation cell must have its
    /// background span both columns — the continuation cell's own
    /// (different) background must NOT paint over the second half of the
    /// wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so antialiased glyph ink can't shift the
        // probed colour — same trick as the GTK/macOS twin tests.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// #703: mirrors `gtk::terminal`'s `narrow_wide_glyph_is_stretched_to_fill_two_cells`
    /// — every backend now shares `wide_glyph_x_scale`, so a CJK glyph
    /// measuring 15px in an 18px (2 × 9px) box scales 1.2x here too.
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

    /// #703: mirrors `gtk::terminal`'s `exact_fit_wide_glyph_is_not_scaled`.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// #703: mirrors `gtk::terminal`'s `over_wide_glyph_is_shrunk_into_box`.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    /// Companion regression, mirroring `gtk`/`macos::terminal`'s
    /// `narrow_cells_advance_by_single_char_width`: ordinary narrow
    /// (single-width) cells must still advance by exactly `char_width` —
    /// the wide-cell fix must not widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    /// The cursor overlay swaps fg/bg: the cell background paints in
    /// the cell's *foreground* colour.
    #[test]
    fn cursor_cell_inverts_fg_bg() {
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (fg.r, fg.g, fg.b));
    }

    /// A find-active match paints its cell background orange —
    /// matching the GTK/macOS overlay convention.
    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (255, 165, 0));
    }

    /// A selected cell uses `theme.selection_bg`.
    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            )
        );
    }

    /// Rows past the pane bottom must be clipped, not bled — the
    /// interactive-resize regression `gtk`/`macos` already guard
    /// (quadraui#437 / #484).
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        let bg_sentinel = Color::rgb(1, 2, 3);
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
        let pane_h = 60.0_f32;
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, W as f32, H as f32), bg_sentinel);
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, pane_h, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let inside = surface.pixel_at(4, 4);
        assert_eq!(
            (inside.r, inside.g, inside.b),
            (magenta.r, magenta.g, magenta.b)
        );

        let below = surface.pixel_at(4, H - 4);
        assert_eq!(
            (below.r, below.g, below.b),
            (bg_sentinel.r, bg_sentinel.g, bg_sentinel.b),
            "rows below the pane bottom must be clipped, not bled"
        );
    }

    /// A bold cell paints via `DWrite::draw_text_styled` rather than
    /// silently dropping the attribute — smoke-tested by confirming the
    /// paint call succeeds and the cell's own bg still lands correctly
    /// (glyph shape/weight isn't independently probable via pixel
    /// colour without OCR, so this only guards the plumbing).
    #[test]
    fn bold_cell_paints_without_error() {
        let mut c = cell('B', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.bold = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (20, 20, 20));
    }

    /// [`draw_terminal_divider`] paints a 1-DIP line at `rect.x`,
    /// ignoring `rect.width`.
    #[test]
    fn divider_paints_a_one_dip_line() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(
                    target,
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    Color::rgb(0, 0, 0),
                );
                draw_terminal_divider(target, 50.0, 0.0, H as f32, &theme);
            })
            .expect("paint");

        let on_line = surface.pixel_at(50, 10);
        let off_line = surface.pixel_at(52, 10);
        assert_eq!(
            (on_line.r, on_line.g, on_line.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );
        assert_eq!((off_line.r, off_line.g, off_line.b), (0, 0, 0));
    }
}
