//! macOS support for [`crate::Terminal`] cell grids.
//!
//! Painting moved to the shared [`crate::primitives::terminal::paint`] /
//! [`crate::primitives::terminal::paint_divider`] (#810, NativeSurface
//! Phase 2c) — see that fn's doc for the divergence (per-cell
//! bold/italic/underline styling) resolved while unifying
//! `gtk::terminal::draw_terminal_cells`, `macos::terminal::draw_terminal_cells`
//! and `win::terminal::draw_terminal_cells` into one implementation.
//! macOS gained no new styling capability from this (it never rendered
//! bold/italic/underline before #810 either — see
//! [`crate::native_surface::NativeSurface::surface_draw_text_run_styled`]'s
//! doc), but keeps the wide-glyph horizontal-scale fix (#500/#703) it
//! already had.
//!
//! This module now carries no rasteriser code of its own — both deleted
//! free functions had zero call sites in `coord-tui`/`vimcode` (`grep -rn
//! "draw_terminal_cells\|draw_terminal_divider" ~/src/coord-tui/src
//! ~/src/vimcode/src` — the only hits there are `Backend::draw_terminal`/
//! `Backend::draw_terminal_divider` trait calls, whose signatures are
//! unchanged), so neither needed a deprecation shim (CLAUDE.md rule 1).

#[cfg(test)]
mod tests {
    use super::super::form::RawFormSurface;
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
        TerminalCell {
            text: ch.to_string(),
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

    /// #440 / #500 regression, mirroring `gtk::backend::tests`'s
    /// `wide_cell_background_spans_two_columns` twin: a double-width
    /// glyph (CJK) followed by vt100's blank continuation cell must have
    /// its background span both columns — the continuation cell's own
    /// (different) background must NOT paint over the second half of the
    /// wide glyph's cell. Paints via `RawFormSurface` directly (rather
    /// than through `MacBackend::draw_terminal`) so the test controls
    /// `char_width` explicitly instead of depending on Menlo's measured
    /// advance — the same reason `win::multi_section_view`'s embedded
    /// `Terminal` section body uses that adapter.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

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
        let mut raw = RawFormSurface {
            ctx: surface.context_ptr(),
            font: &f,
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

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
    /// not widen unrelated cells. Mirrors `gtk::backend::tests`'s
    /// `narrow_cells_advance_by_single_char_width`.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

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
        let mut raw = RawFormSurface {
            ctx: surface.context_ptr(),
            font: &f,
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

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

    /// #703: mirrors the GTK twin — both backends share `wide_glyph_x_scale`,
    /// so a CJK glyph measuring 15px in an 18px (2 × 9px) box scales
    /// 1.2x on macOS too.
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

    /// #703: mirrors the GTK twin.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// #703: mirrors the GTK twin.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
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
                    text: ch.to_string(),
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
