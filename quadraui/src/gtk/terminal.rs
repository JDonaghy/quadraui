//! GTK rasteriser for [`crate::Terminal`] cell grids.
//!
//! Iterates rows then per-cell, painting cell background then per-cell
//! glyph (skipped for spaces and `\0`). Overlay flags
//! (`is_cursor`, `is_find_active`, `is_find_match`, `selected`)
//! override the cell's `bg`/`fg` to match the legacy bespoke
//! renderer's behaviour. Bold / italic / underline applied via Pango
//! `AttrList` per cell.
//!
//! # Wide characters (#439)
//!
//! [`crate::terminal_engine::TerminalSession::to_terminal`] builds its
//! cell grid straight from vt100's model: a double-width character
//! (CJK, emoji, ...) occupies its *own* column plus one trailing
//! "continuation" column that vt100 reports as an empty cell (`ch =
//! ' '`). Before #439, this rasteriser advanced by exactly
//! `char_width` per grid column regardless of glyph width, so a
//! double-width glyph — which Pango draws at roughly double the
//! advance width — got its right half painted over by the
//! continuation column's background fill. [`draw_terminal_cells`] now
//! detects wide glyphs with `unicode-width`, paints their background
//! across two columns, and skips the continuation column so it's
//! never independently painted on top of the glyph.
//!
//! The two-column cell is the *box*; the glyph inside it still needs to
//! fill that box. The font Pango falls back to for CJK / emoji typically
//! lays a glyph out at its own natural advance — often narrower than two
//! terminal cells (a CJK glyph measuring 15px inside an 18px box), which
//! left a ragged gap before the next wide glyph. [`draw_terminal_cells`]
//! therefore scales each wide glyph horizontally (see
//! [`wide_glyph_x_scale`]) so it spans exactly `cell_w`, giving the tight,
//! even two-cell packing a normal terminal produces.

use gtk4::cairo::Context;
use gtk4::pango;
use gtk4::pango::AttrList;
use unicode_width::UnicodeWidthChar;

use crate::primitives::terminal::Terminal;
use crate::theme::Theme;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, content_y)` on `cr`. The caller is responsible for filling
/// the surrounding background (vimcode does this with
/// `theme.terminal_bg` before calling so the area outside the cell
/// grid stays consistent).
///
/// `cell_area_w` clips per-row painting — cells past the right edge
/// stop being drawn rather than wrapping. `line_height` and
/// `char_width` are the per-cell dimensions in DIPs.
#[allow(clippy::too_many_arguments)]
pub fn draw_terminal_cells(
    cr: &Context,
    layout: &pango::Layout,
    term: &Terminal,
    x: f64,
    content_y: f64,
    cell_area_w: f64,
    line_height: f64,
    char_width: f64,
    theme: &Theme,
) {
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = content_y + row_idx as f64 * line_height;
        let mut cell_x = x;
        let mut col = 0usize;
        while col < row.len() {
            let cell = &row[col];
            // Double-width glyphs (CJK, emoji, ...) get a two-column cell:
            // the vt100 grid already reserves the following column as an
            // empty continuation placeholder, so claim it here rather
            // than letting it paint its own (mismatched) background over
            // the glyph's right half.
            let is_wide = matches!(UnicodeWidthChar::width(cell.ch), Some(2));
            let cell_w = if is_wide {
                char_width * 2.0
            } else {
                char_width
            };

            if cell_x + cell_w > x + cell_area_w {
                break;
            }
            let (br, bg, bb) = (cell.bg.r, cell.bg.g, cell.bg.b);
            let (fr, fg2, fb) = (cell.fg.r, cell.fg.g, cell.fg.b);
            let (draw_br, draw_bg, draw_bb) = if cell.is_cursor {
                (fr, fg2, fb)
            } else if cell.is_find_active {
                (255u8, 165u8, 0u8)
            } else if cell.is_find_match {
                (100u8, 80u8, 20u8)
            } else if cell.selected {
                (
                    theme.selection_bg.r,
                    theme.selection_bg.g,
                    theme.selection_bg.b,
                )
            } else {
                (br, bg, bb)
            };
            cr.set_source_rgb(
                draw_br as f64 / 255.0,
                draw_bg as f64 / 255.0,
                draw_bb as f64 / 255.0,
            );
            cr.rectangle(cell_x, row_y, cell_w, line_height);
            cr.fill().ok();

            if cell.ch != ' ' && cell.ch != '\0' {
                let (draw_fr, draw_fg, draw_fb) = if cell.is_cursor {
                    (br, bg, bb)
                } else if cell.is_find_active {
                    (0u8, 0u8, 0u8)
                } else {
                    (fr, fg2, fb)
                };
                cr.set_source_rgb(
                    draw_fr as f64 / 255.0,
                    draw_fg as f64 / 255.0,
                    draw_fb as f64 / 255.0,
                );

                let attrs = AttrList::new();
                if cell.bold {
                    attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
                }
                if cell.italic {
                    attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
                }
                if cell.underline {
                    attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
                }
                layout.set_attributes(Some(&attrs));
                let s = cell.ch.to_string();
                layout.set_text(&s);

                if is_wide {
                    // A double-width glyph occupies a two-column (`cell_w`)
                    // box, but the font Pango falls back to for CJK / emoji
                    // usually lays the glyph out at its *own* natural advance
                    // — narrower than two terminal cells (e.g. a CJK glyph
                    // measuring 15px inside an 18px box) or, for some emoji
                    // fonts, wider. Drawn left-aligned at the cell origin,
                    // that leaves a ragged gap before the next wide glyph:
                    // the #439 follow-up defect where consecutive CJK/emoji
                    // looked abnormally spread out. Scale the glyph
                    // horizontally so it fills exactly `cell_w`, giving the
                    // tight, even two-cell packing a normal terminal
                    // produces. Narrow cells already match `char_width`
                    // (measured from the same monospace font), so only wide
                    // cells need this.
                    let (natural_w, _) = layout.pixel_size();
                    let scale_x = wide_glyph_x_scale(natural_w, cell_w);
                    if (scale_x - 1.0).abs() > f64::EPSILON {
                        cr.save().ok();
                        cr.translate(cell_x, row_y);
                        cr.scale(scale_x, 1.0);
                        cr.move_to(0.0, 0.0);
                        pangocairo::functions::show_layout(cr, layout);
                        cr.restore().ok();
                    } else {
                        cr.move_to(cell_x, row_y);
                        pangocairo::functions::show_layout(cr, layout);
                    }
                } else {
                    cr.move_to(cell_x, row_y);
                    pangocairo::functions::show_layout(cr, layout);
                }
                layout.set_attributes(None);
            }

            cell_x += cell_w;
            col += if is_wide { 2 } else { 1 };
        }
    }
}

/// Horizontal scale factor to draw a double-width glyph so it fills its
/// two-column (`cell_w`) box exactly.
///
/// `natural_w` is the glyph's laid-out pixel width as reported by Pango.
/// Returns `cell_w / natural_w` so the rendered glyph spans exactly two
/// cells — stretching a narrow CJK glyph out to the full box and shrinking
/// an over-wide emoji back into it. Returns `1.0` (no scaling) when
/// `natural_w` is non-positive (empty / zero-advance layout) so we never
/// divide by zero or blow a degenerate glyph up to infinity.
fn wide_glyph_x_scale(natural_w: i32, cell_w: f64) -> f64 {
    if natural_w <= 0 {
        1.0
    } else {
        cell_w / natural_w as f64
    }
}

/// Draw a vertical divider line for a terminal split pane.
/// Paints a 1-pixel-wide line at `x` from `y` to `y + height`
/// using `theme.separator` colour.
pub fn draw_terminal_divider(cr: &Context, x: f64, y: f64, height: f64, theme: &Theme) {
    let (r, g, b) = (
        theme.separator.r as f64 / 255.0,
        theme.separator.g as f64 / 255.0,
        theme.separator.b as f64 / 255.0,
    );
    cr.set_source_rgb(r, g, b);
    cr.rectangle(x, y, 1.0, height);
    cr.fill().ok();
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless paint tests: verify `draw_terminal_cells` background-fill
// behaviour for wide (double-width) vs narrow cells without a display.
// Uses a Cairo `ImageSurface` and reads back pixel data directly, mirroring
// the pattern in `gtk::tab_bar` / `gtk::multi_section_view`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::terminal::TerminalCell;
    use crate::types::{Color, WidgetId};
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
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme::default();
            draw_terminal_cells(
                &cr,
                &pango_layout,
                term,
                0.0,
                0.0,
                W as f64,
                LINE_H,
                CHAR_W,
                &theme,
            );
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
        // `wide_glyph_x_scale` unit tests below.
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

    /// #439 follow-up: a wide glyph whose natural Pango advance is *narrower*
    /// than its two-cell box must be stretched to fill it exactly, so
    /// consecutive CJK/emoji pack tightly with no ragged inter-glyph gap.
    /// A CJK glyph measuring 15px inside an 18px (2 × 9px) box → 1.2×.
    #[test]
    fn narrow_wide_glyph_is_stretched_to_fill_two_cells() {
        let cell_w = 18.0;
        let scale = wide_glyph_x_scale(15, cell_w);
        assert!(
            (scale - 1.2).abs() < 1e-9,
            "15px glyph in an 18px box should scale 1.2×, got {scale}"
        );
        // And the rendered advance is exactly the two-cell box width.
        assert!((15.0 * scale - cell_w).abs() < 1e-9);
    }

    /// A wide glyph that already fills its box (emoji measuring exactly two
    /// cells) is left untouched — scale factor 1.0.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18, 18.0), 1.0);
    }

    /// A wide glyph *wider* than two cells (some colour-emoji fonts) is
    /// shrunk back into the box so it can't overlap the next glyph.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24, 18.0);
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
        assert_eq!(wide_glyph_x_scale(0, 18.0), 1.0);
        assert_eq!(wide_glyph_x_scale(-3, 18.0), 1.0);
    }
}
