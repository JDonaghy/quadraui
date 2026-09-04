//! GTK rasteriser for [`crate::Minimap`]: fixed row pitch, per-column
//! colour blocks (#667; supersedes the file-length-dependent font scaling
//! from #382).
//!
//! Rows tile at a fixed [`ROW_PITCH_PX`], independent of the file's
//! length ([`crate::MinimapSizing::FixedPitch`]) — see
//! `Minimap::layout`'s module docs for the slide behaviour when a file
//! needs more rows than the strip holds at that pitch. At that pitch,
//! [`is_legible`] is false, so painting lands in
//! [`MinimapRenderMode::ColumnBlocks`]: one 1px-wide block per non-blank
//! character column, coloured by whichever aggregated span covers it,
//! rather than one bar per line — this is what preserves the indent
//! silhouette VS Code's `renderCharacters: false` mode reads as (#667).
//! [`MinimapRenderMode::Characters`] — real glyphs painted through Pango
//! at a scaled-down **absolute** size (`FontDescription::set_absolute_size`)
//! — is kept for pitches at or above
//! [`crate::primitives::minimap::LEGIBILITY_FLOOR_PX`] (not reached
//! by [`ROW_PITCH_PX`] today, but still a real, tested code path: nothing
//! stops a future caller from requesting a taller fixed pitch). The mode
//! switch ([`render_mode`]) is a pure function of the row pitch so it's
//! deterministic and directly testable without a live surface.
//!
//! Both branches bound their per-row cost to [`COLUMN_CAPACITY`] columns:
//! `ColumnBlocks`' walk stops there, and `Characters` truncates the text
//! it hands to Pango there too, so a pathologically long line costs no
//! more to paint or shape than a short one (#667 pt. 3).
//!
//! Colour lookups (both branches) walk `syntax_spans` once per paint via
//! [`SpanCursor`], not once per row — `aggregate_spans` documents its
//! output as sorted by `(line_idx, start_col)`, and `visible_lines` is
//! itself always ascending in `start_line_idx`, so a single merge-walk
//! is O(rows + spans) total rather than the old O(rows * spans) rescan
//! (#667 pt. 4).
//!
//! #738: the legibility/render-mode thresholds (`is_legible`,
//! `render_mode`, `minimap_font_px`), the shared `ROW_PITCH_PX` /
//! `COLUMN_CAPACITY` geometry constants, and the span-lookup helpers
//! (`SpanCursor`, `color_at_column`, `truncate_to_columns`) used to be
//! defined in this module. They now live in
//! [`crate::primitives::minimap`] so `win::minimap` (and, eventually, a
//! macOS rasteriser) consume the exact same decision logic instead of
//! growing an independent interpretation of either threshold — this
//! module only imports them.

use gtk4::cairo::Context;
use gtk4::pango;

use super::{cairo_rgb, set_source};
use crate::event::Rect as QRect;
use crate::primitives::minimap::{
    color_at_column, minimap_font_px, render_mode, truncate_to_columns, Minimap, MinimapLayout,
    MinimapRenderMode, MinimapSizing, MinimapSpan, SpanCursor, VisibleMinimapLine, COLUMN_CAPACITY,
    ROW_PITCH_PX,
};
use crate::theme::Theme;

/// GTK shows one buffer line per painted row — no cross-line colour
/// reduction (see [`crate::MinimapGrid`]'s doc for why TUI differs).
pub const LINES_PER_ROW: usize = 1;

/// Compute the GTK pixel-unit layout for a [`Minimap`] without painting.
pub fn gtk_minimap_layout(minimap: &Minimap, x: f64, y: f64, w: f64, h: f64) -> MinimapLayout {
    minimap.layout_with_sizing(
        QRect::new(x as f32, y as f32, w as f32, h as f32),
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(ROW_PITCH_PX as f32),
    )
}

/// Draw a [`Minimap`] onto `cr`. Returns the layout for host click
/// dispatch (`layout.hit_test(x, y)` -> [`crate::MinimapHit`]).
#[allow(clippy::too_many_arguments)]
pub fn draw_minimap(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let layout = gtk_minimap_layout(minimap, x, y, w, h);

    if layout.visible_lines.is_empty() {
        return layout;
    }

    set_source(cr, theme.background);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    let hl = &layout.viewport_highlight;
    if hl.height > 0.0 {
        let (r, g, b) = cairo_rgb(theme.accent_bg);
        cr.set_source_rgba(r, g, b, 0.25);
        cr.rectangle(hl.x as f64, hl.y as f64, hl.width as f64, hl.height as f64);
        cr.fill().ok();
    }

    // Row pitch already lives on the layout, post-cap (`Minimap::layout`,
    // #663) -- read it back rather than recomputing `h /
    // visible_lines.len()`, which would silently undo the cap and let a
    // short file's font balloon again. All rows share one pitch, so the
    // first is representative.
    let row_px = layout
        .visible_lines
        .first()
        .map(|v| v.bounds.height as f64)
        .unwrap_or(0.0);
    let mode = render_mode(row_px);
    let saved_font = pango_layout.font_description();

    // Clip all row painting to the strip: a legible pitch can still shape
    // a line longer than `w`, and below-floor colour blocks are already
    // width-bounded but glyphs are not -- without this, text bled across
    // whatever the host painted beside the minimap (#663).
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    // One merge-walk over `syntax_spans` for the whole paint, not one
    // linear rescan per row (#667 pt. 4) -- `visible_lines` is always
    // ascending in `start_line_idx` (see `Minimap::layout`), matching
    // `SpanCursor`'s own non-decreasing-query requirement.
    let mut spans = SpanCursor::new(&minimap.syntax_spans);

    for vline in &layout.visible_lines {
        let Some(line) = minimap.lines.get(vline.start_line_idx) else {
            continue;
        };
        let row_spans = spans.row_spans(vline.start_line_idx);
        match mode {
            MinimapRenderMode::Characters => paint_row_glyphs(
                cr,
                pango_layout,
                saved_font.as_ref(),
                vline,
                row_px,
                &line.text,
                row_spans,
                theme,
            ),
            MinimapRenderMode::ColumnBlocks => {
                paint_row_blocks(cr, vline, &line.text, row_spans, theme)
            }
        }
    }

    pango_layout.set_attributes(None);
    pango_layout.set_font_description(saved_font.as_ref());
    // Reset the ellipsize/width state `paint_row_glyphs` set below so it
    // can't leak onto whatever the shared layout paints next.
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    cr.restore().ok();

    layout
}

#[allow(clippy::too_many_arguments)]
fn paint_row_glyphs(
    cr: &Context,
    pango_layout: &pango::Layout,
    saved_font: Option<&pango::FontDescription>,
    vline: &VisibleMinimapLine,
    row_px: f64,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    let mut font = saved_font.cloned().unwrap_or_default();
    font.set_absolute_size(minimap_font_px(row_px) * pango::SCALE as f64);
    pango_layout.set_font_description(Some(&font));
    // Bound the shaped run to the row's own width and ellipsize rather
    // than shaping (and then relying on the Cairo clip to hide) glyphs
    // that will never be visible -- keeps shaping cheap even for a very
    // long line; the `cr.clip()` in `draw_minimap` is the hard guarantee.
    pango_layout.set_width((vline.bounds.width * pango::SCALE as f32).round() as i32);
    pango_layout.set_ellipsize(pango::EllipsizeMode::End);

    // Never shape more than the strip's column capacity worth of a line
    // (#667 pt. 3) -- a pathologically long line must cost no more to
    // shape than a short one, mirroring `paint_row_blocks`' own walk cap.
    let truncated = truncate_to_columns(text, COLUMN_CAPACITY);
    pango_layout.set_text(truncated);

    if row_spans.is_empty() {
        pango_layout.set_attributes(None);
    } else {
        let to_u16 = |c: u8| -> u16 { ((c as u16) << 8) | c as u16 };
        let attrs = pango::AttrList::new();
        for span in row_spans {
            let (start, end) = char_range_to_byte_range(truncated, span.start_col, span.end_col);
            let mut a = pango::AttrColor::new_foreground(
                to_u16(span.color.r),
                to_u16(span.color.g),
                to_u16(span.color.b),
            );
            a.set_start_index(start);
            a.set_end_index(end);
            attrs.insert(a);
        }
        pango_layout.set_attributes(Some(&attrs));
    }

    set_source(cr, theme.foreground);
    cr.move_to(vline.bounds.x as f64, vline.bounds.y as f64);
    super::painted_text::show_layout(cr, pango_layout);
}

/// Paint one 1px-wide block per non-blank character column of `text`,
/// each coloured by whichever span covers it — VS Code's
/// `renderCharacters: false` look, which (unlike a single per-line bar)
/// preserves the line's indent and internal-gap silhouette (#667 pt. 2).
/// Stops after [`COLUMN_CAPACITY`] columns, so a pathologically long
/// line costs no more to paint than a short one.
fn paint_row_blocks(
    cr: &Context,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    for (col, ch) in text.chars().enumerate().take(COLUMN_CAPACITY) {
        if ch.is_whitespace() {
            continue;
        }
        let color = color_at_column(row_spans, col, theme.foreground);
        set_source(cr, color);
        cr.rectangle(
            vline.bounds.x as f64 + col as f64,
            vline.bounds.y as f64,
            1.0,
            vline.bounds.height as f64,
        );
        cr.fill().ok();
    }
}

/// Convert a `[start_col, end_col)` character range into UTF-8 byte
/// offsets Pango attributes need. Out-of-range columns clamp to the
/// text's own char count rather than panicking.
fn char_range_to_byte_range(text: &str, start_col: usize, end_col: usize) -> (u32, u32) {
    let mut start_byte = text.len();
    let mut end_byte = text.len();
    let mut char_idx = 0usize;
    for (byte_idx, _) in text.char_indices() {
        if char_idx == start_col {
            start_byte = start_byte.min(byte_idx);
        }
        if char_idx == end_col {
            end_byte = byte_idx;
        }
        char_idx += 1;
    }
    if start_col == 0 {
        start_byte = 0;
    }
    if end_col >= char_idx {
        end_byte = text.len();
    }
    (
        start_byte.min(text.len()) as u32,
        end_byte.min(text.len()) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine};
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn minimap_from(lines: Vec<&str>, total_buffer_lines: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, t)| MinimapLine {
                    text: t.into(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines,
        }
    }

    fn surface_and_layout() -> (CairoContext, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (cr, layout)
    }

    // ── fixed pitch, not file-length-dependent (#667) ────────────────

    #[test]
    fn row_pitch_is_independent_of_the_strip_height() {
        // Inverts the old #382/#663 contract on purpose: the same
        // buffer painted into strips of two very different heights must
        // now resolve to the *same* row pitch. See `Minimap::layout`'s
        // module docs for why (`MinimapSizing::FixedPitch`).
        let lines: Vec<&str> = vec!["fn main() {}"; 8];
        let mm = minimap_from(lines, 8);

        let tall = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 800.0);
        let short = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 16.0);

        let tall_px = tall.visible_lines[0].bounds.height as f64;
        let short_px = short.visible_lines[0].bounds.height as f64;
        assert_eq!(tall_px, ROW_PITCH_PX);
        assert_eq!(short_px, ROW_PITCH_PX);
    }

    #[test]
    fn row_pitch_is_independent_of_the_file_length() {
        // The other half of the same guarantee: a 3-line file and a
        // 10,000-line file painted into the *same* strip must also
        // resolve to the same row pitch.
        let short_mm = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long_mm = minimap_from(long_refs, 10_000);

        let short_layout = gtk_minimap_layout(&short_mm, 0.0, 0.0, 40.0, 400.0);
        let long_layout = gtk_minimap_layout(&long_mm, 0.0, 0.0, 40.0, 400.0);

        assert_eq!(
            short_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
        assert_eq!(
            long_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
    }

    #[test]
    fn short_file_in_a_tall_strip_does_not_stretch() {
        // A 3-line file at the fixed pitch occupies 6px of a 200px
        // strip -- it must not stretch to fill it (the pre-#667 `Fill`
        // behaviour, now GTK-inapplicable).
        let mm = minimap_from(vec!["//! Placeholder module"; 3], 3);
        let layout = gtk_minimap_layout(&mm, 0.0, 0.0, 200.0, 200.0);
        assert_eq!(layout.visible_lines.len(), 3);
        let last = layout.visible_lines.last().unwrap();
        assert!(
            (last.bounds.y + last.bounds.height) < 200.0,
            "a short file must not stretch its rows to fill the whole strip"
        );
    }

    // ── legibility floor ─────────────────────────────────────────────
    //
    // `is_legible`/`render_mode`'s own threshold behaviour is covered by
    // `primitives::minimap::tests::legibility_floor_switches_render_mode_on_both_sides`
    // (#738) — this module keeps only the paint-level regression that a
    // real Cairo/Pango pass actually respects that threshold.

    #[test]
    fn default_fixed_pitch_stays_below_the_floor_and_never_shapes_text() {
        // ROW_PITCH_PX (2px) is below LEGIBILITY_FLOOR_PX (4px), so the
        // default GTK minimap always lands in ColumnBlocks -- and,
        // unlike pre-#667, that no longer changes with strip height.
        let mm = minimap_from(vec!["fn main() {}"; 4], 4);
        let (cr, layout) = surface_and_layout();

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 4.0, &mm, &Theme::default());
        assert_eq!(layout.text(), "");

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 400.0, &mm, &Theme::default());
        assert_eq!(
            layout.text(),
            "",
            "the fixed pitch stays below the legibility floor regardless of strip height"
        );
    }

    #[test]
    fn characters_branch_truncates_to_the_column_capacity_before_shaping() {
        // The Characters branch is unreachable through the default fixed
        // pitch today, but it must still bound its shaping cost (#667 pt.
        // 3) -- exercised directly since `draw_minimap` can't reach it
        // via `ROW_PITCH_PX` alone.
        let long_line = "y".repeat(10_000);
        let (cr, pango_layout) = surface_and_layout();
        let vline = VisibleMinimapLine {
            start_line_idx: 0,
            bounds: QRect::new(0.0, 0.0, 400.0, 20.0),
        };
        paint_row_glyphs(
            &cr,
            &pango_layout,
            None,
            &vline,
            20.0,
            &long_line,
            &[],
            &Theme::default(),
        );
        assert_eq!(
            pango_layout.text().chars().count(),
            COLUMN_CAPACITY,
            "must shape no more than COLUMN_CAPACITY characters even for a 10,000-char line"
        );
    }

    // ── per-column colour blocks (#667 pt. 2) ─────────────────────────

    #[test]
    fn column_blocks_skip_indentation_and_internal_gaps() {
        // 4-space indent, "x", 2-space internal gap, "y": the walk must
        // paint no block over the indent or the gap -- only over the two
        // non-blank columns, preserving the indent silhouette.
        let mm = minimap_from(vec!["    x  y"], 1);
        let theme = Theme {
            background: Color::rgb(10, 10, 10),
            foreground: Color::rgb(200, 200, 200),
            ..Theme::default()
        };

        let mut surface = ImageSurface::create(Format::ARgb32, 20, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(&cr, &pango_layout, 0.0, 0.0, 20.0, 4.0, &mm, &theme);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let bg = (theme.background.r, theme.background.g, theme.background.b);
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);

        for col in 0..4 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "indent column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 4, 0),
            fg,
            "'x' column must paint a block"
        );
        for col in 5..7 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "gap column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 7, 0),
            fg,
            "'y' column must paint a block"
        );
    }

    #[test]
    fn column_blocks_stop_at_the_column_capacity() {
        let long_line = "x".repeat(10_000);
        let mm = minimap_from(vec![&long_line], 1);
        let theme = Theme {
            background: Color::rgb(1, 1, 1),
            foreground: Color::rgb(250, 250, 250),
            ..Theme::default()
        };
        let strip_w = (COLUMN_CAPACITY + 50) as i32;

        let mut surface = ImageSurface::create(Format::ARgb32, strip_w, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                strip_w as f64,
                4.0,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        let bg = (theme.background.r, theme.background.g, theme.background.b);
        assert_eq!(
            pixel(&data, stride, (COLUMN_CAPACITY - 1) as i32, 0),
            fg,
            "the last in-capacity column must still paint"
        );
        assert_eq!(
            pixel(&data, stride, COLUMN_CAPACITY as i32, 0),
            bg,
            "the walk must stop at COLUMN_CAPACITY and paint no further"
        );
    }

    // `color_at_column`/`SpanCursor`'s own behaviour is covered by
    // `primitives::minimap::tests::color_at_column_*` /
    // `span_cursor_matches_a_full_linear_scan_per_row` (#738) — nothing
    // GTK-specific left to re-test here.

    // ── paint/click round trip ────────────────────────────────────────

    /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
    /// — `minimap_layout` is documented **ABSOLUTE**
    /// (`docs/PRIMITIVE_RULES.md` "Coordinate frames for `*_layout`
    /// methods", issue #505), so `hit_test` must be called with
    /// coordinates shifted by the same `x`/`y` origin the strip was
    /// painted at.
    fn paint_and_click_round_trip_at(x: f64, y: f64) {
        let mm = minimap_from(vec!["x"; 8], 8);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            x,
            y,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, (y + 50.0) as f32),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, y as f32),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        paint_and_click_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_minimap_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn char_range_to_byte_range_handles_multibyte_text() {
        // "café" — 'é' is 2 bytes (bytes 3-4), so the byte length (5)
        // diverges from the char count (4).
        let (start, end) = char_range_to_byte_range("café", 3, 4);
        assert_eq!(start, 3);
        assert_eq!(end, 5);
    }

    #[test]
    fn empty_minimap_is_a_no_op() {
        let mm = minimap_from(vec![], 0);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert!(layout.visible_lines.is_empty());
    }

    // ── strip clipping (#663) ───────────────────────────────────────────

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A].
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    #[test]
    fn wide_line_blocks_never_paint_right_of_bounds() {
        // Pre-#663 there was no `cr.clip()` and no Pango layout width, so
        // a line wider than the strip painted straight across whatever
        // sat beside it (in vimcode's case, the neighbouring editor
        // pane). Paint a pathologically long line into a strip embedded
        // in a wider white canvas and assert nothing right of the strip
        // is touched.
        const STRIP_W: i32 = 40;
        const CANVAS_W: i32 = 200;
        const H: i32 = 40;

        let long_line = "x".repeat(500);
        let mm = minimap_from(vec![&long_line], 1);

        let mut surface =
            ImageSurface::create(Format::ARgb32, CANVAS_W, H).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0); // white sentinel
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme {
                background: Color::rgb(20, 20, 20),
                foreground: Color::rgb(255, 255, 255),
                ..Theme::default()
            };
            // The fixed pitch (ROW_PITCH_PX, 2px) is below the
            // legibility floor, so this exercises the ColumnBlocks
            // branch -- the column-capacity walk on its own already
            // bounds how far right it paints, but the clip is still the
            // hard guarantee this test checks.
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                STRIP_W as f64,
                H as f64,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for y in 0..H {
            for x in STRIP_W..CANVAS_W {
                assert_eq!(
                    pixel(&data, stride, x, y),
                    (255, 255, 255),
                    "pixel ({x},{y}) is right of bounds.x + bounds.width and must be untouched"
                );
            }
        }
    }
}
