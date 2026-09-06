//! Direct2D / DirectWrite support for [`crate::Terminal`] cell grids
//! (issue #30).
//!
//! Painting moved to the shared [`crate::primitives::terminal::paint`] /
//! [`crate::primitives::terminal::paint_divider`] (#810, NativeSurface
//! Phase 2c) — see that fn's doc for the divergence (per-cell
//! bold/italic/underline styling: this backend keeps `bold` via
//! `DWrite::draw_text_styled`, `italic`/`underline` are still not wired
//! up) resolved while unifying `gtk::terminal::draw_terminal_cells`,
//! `macos::terminal::draw_terminal_cells` and
//! `win::terminal::draw_terminal_cells` into one implementation.
//!
//! This module now carries no rasteriser code of its own — both deleted
//! free functions had zero call sites in `coord-tui`/`vimcode` (`grep -rn
//! "draw_terminal_cells\|draw_terminal_divider" ~/src/coord-tui/src
//! ~/src/vimcode/src` — the only hits there are `Backend::draw_terminal`/
//! `Backend::draw_terminal_divider` trait calls, whose signatures are
//! unchanged); [`crate::win::multi_section_view`]'s embedded `Terminal`
//! section body — the one in-tree caller of the old free function
//! outside `WinBackend` itself — now goes through
//! [`super::form::RawFormSurface`] instead (see that struct's doc).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod terminal;` and `backend.rs`'s
//! module docs.

#[cfg(test)]
mod tests {
    use crate::event::Rect;
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::win::form::RawFormSurface;
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::{fill_rect, DWrite};

    const W: u32 = 200;
    const H: u32 = 120;
    const CHAR_W: f32 = 10.0;
    const LINE_H: f32 = 20.0;

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

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0).expect("create DWrite").0
    }

    /// Paints `term` via `RawFormSurface` — the same adapter
    /// `win::multi_section_view`'s embedded `Terminal` section body
    /// uses — so these tests keep controlling `char_width`/`line_height`
    /// explicitly instead of depending on a live `WinBackend`'s measured
    /// font metrics.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, term: &Terminal, theme: &Theme) {
        surface
            .paint(|target| {
                let mut raw = RawFormSurface { target, dwrite };
                crate::primitives::terminal::paint(
                    term, &mut raw, theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
                );
            })
            .expect("paint");
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
        paint(&surface, &dwrite, &term, &theme);

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
        paint(&surface, &dwrite, &term, &theme);

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
        paint(&surface, &dwrite, &term, &theme);

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
        paint(&surface, &dwrite, &term, &theme);

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
        paint(&surface, &dwrite, &term, &theme);

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
        paint(&surface, &dwrite, &term, &theme);

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
                let mut raw = RawFormSurface {
                    target,
                    dwrite: &dwrite,
                };
                crate::primitives::terminal::paint(
                    &term, &mut raw, &theme, 0.0, 0.0, W as f32, pane_h, LINE_H, CHAR_W, None,
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
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (20, 20, 20));
    }

    /// [`crate::primitives::terminal::paint_divider`] paints a 1-DIP
    /// line at `x`.
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
                let dwrite = dwrite();
                let mut raw = RawFormSurface {
                    target,
                    dwrite: &dwrite,
                };
                crate::primitives::terminal::paint_divider(&mut raw, 50.0, 0.0, H as f32, &theme);
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
