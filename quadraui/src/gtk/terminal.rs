//! GTK support for [`crate::Terminal`] cell grids.
//!
//! Painting moved to the shared [`crate::primitives::terminal::paint`] /
//! [`crate::primitives::terminal::paint_divider`] (#810, NativeSurface
//! Phase 2c) — see that fn's doc for the divergences (per-cell
//! bold/italic/underline styling, and #492's incidental `painted_text`
//! tracking fix) resolved while unifying
//! `gtk::terminal::draw_terminal_cells`,
//! `macos::terminal::draw_terminal_cells` and
//! `win::terminal::draw_terminal_cells` into one implementation. GTK's
//! own [`crate::native_surface::NativeSurface::surface_draw_text_run_styled`]
//! override applies all three style flags via Pango's `AttrList`,
//! matching this module's pre-#810 behaviour exactly.
//!
//! This module now carries no rasteriser code of its own — both deleted
//! free functions had zero call sites in `coord-tui`/`vimcode` (`grep -rn
//! "draw_terminal_cells\|draw_terminal_divider" ~/src/coord-tui/src
//! ~/src/vimcode/src` — the only hits there are `Backend::draw_terminal`/
//! `Backend::draw_terminal_divider` trait calls, whose signatures are
//! unchanged) or in `kubeui-gtk`, so neither needed a deprecation shim
//! (CLAUDE.md rule 1).
//!
//! # Wide characters (#439)
//!
//! [`crate::terminal_engine::TerminalSession::to_terminal`] builds its
//! cell grid straight from vt100's model: a double-width character
//! (CJK, emoji, ...) occupies its *own* column plus one trailing
//! "continuation" column that vt100 reports as an empty cell (`ch =
//! ' '`). `primitives::terminal::paint` detects wide glyphs via
//! [`crate::terminal_style::wide_cell_advance`] (shared with
//! `macos`/`win`, #500), paints their background across two columns,
//! skips the continuation column, and scales the glyph horizontally
//! (via `NativeSurface::surface_draw_text_run_styled`'s `scale_x`, see
//! [`crate::terminal_style::wide_glyph_x_scale`]) so it spans exactly
//! `cell_w` — the same #439/#703 fix this module's own rasteriser used
//! to apply directly.

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless paint tests: verify terminal-cell background-fill and
// wide-glyph behaviour without a display. Uses a Cairo `ImageSurface`
// and reads back pixel data directly, driving the real
// `GtkBackend::draw_terminal` path (mirrors `gtk::backend::tests`'s own
// `draw_chart`/`draw_text_display` driver tests, #810).

#[cfg(test)]
mod tests {
    use crate::event::{Rect as QRect, Viewport};
    use crate::gtk::backend::GtkBackend;
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 40;
    const CHAR_W: f64 = 10.0;
    const LINE_H: f64 = 20.0;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    /// ARgb32 stores each pixel as [B, G, R, A] in native (little-endian)
    /// byte order; `stride` is in bytes and may include padding.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
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

    fn paint(term: &Terminal) -> ImageSurface {
        let mut backend = GtkBackend::new();
        backend.set_current_theme(Theme::default());
        backend.set_current_line_height(LINE_H);
        backend.set_current_char_width(CHAR_W);
        Backend::begin_frame(&mut backend, Viewport::new(W as f32, H as f32, 1.0));
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                b.draw_terminal(QRect::new(0.0, 0.0, W as f32, H as f32), term);
            });
        }
        surface
    }

    /// #439 regression: a double-width glyph (CJK) followed by vt100's
    /// blank continuation cell must have its background span both
    /// columns — the continuation cell's own (different) background must
    /// NOT paint over the second half of the wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // '日' is a double-width CJK character. Its vt100-derived
        // continuation cell carries a *different* background (cyan) to
        // prove the rasteriser doesn't just get lucky on matching colours.
        //
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so the glyph is invisible. This isolates the
        // thing under test — the two-column *background* fill — from the
        // glyph raster itself: with wide glyphs now scaled to fill the full
        // two-cell box (`wide_glyph_x_scale`), antialiased glyph ink reaches
        // deep into the second column and its exact colour depends on which
        // fallback font Pango picks, which varies by machine. A white glyph
        // here made the probe read a magenta/white blend on some hosts and
        // fail spuriously. Painting the glyph in the background colour keeps
        // the probe a pure, deterministic background read on every host,
        // while the glyph-scaling maths stays covered by the dedicated
        // `wide_glyph_x_scale` unit tests in `terminal_style`.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must not
    /// widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe; 'A' keeps a visible (white) glyph as it isn't probed.
        // This tests the *advance* logic — that 'A' occupies exactly one
        // char_width and doesn't bleed into 'B''s column — independent of
        // glyph rendering.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    // #417's dirty-row *decision* (`TermPaintCache`) lives entirely in
    // `GtkBackend::draw_terminal` and is untouched by #810 — see
    // `gtk::backend::tests::term_row_417` and its many neighbouring
    // tests for that integration's own coverage. The paint-level "only
    // repaint the rows in `dirty_rows`" contract itself is covered by
    // `primitives::terminal::native_surface_paint::tests::dirty_rows_filter_skips_untouched_rows`,
    // which runs on every leg (no Cairo needed) rather than duplicated
    // here as a third real-pixel copy.
}
