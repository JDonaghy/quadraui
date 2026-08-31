//! GTK rasteriser for [`crate::Minimap`]: font scaling (#382).
//!
//! Real glyphs are painted through Pango at a scaled-down **absolute**
//! size (`FontDescription::set_absolute_size`) — never via `cr.scale()`
//! on the editor's existing layout, which would blur hinted text and
//! scale stroke widths along with it (VS Code's `minimap.renderCharacters:
//! true`). Below [`LEGIBILITY_FLOOR_PX`], shaping tiny glyphs buys
//! nothing and Pango collapses them into mush, so the rasteriser falls
//! back to VS Code's `renderCharacters: false` mode: one filled colour
//! bar per line, width proportional to the line's trimmed length. The
//! mode switch ([`render_mode`]) is a pure function of the row pitch so
//! it's deterministic and directly testable without a live surface.

use gtk4::cairo::Context;
use gtk4::pango;

use super::{cairo_rgb, set_source};
use crate::event::Rect as QRect;
use crate::primitives::minimap::{Minimap, MinimapLayout, VisibleMinimapLine};
use crate::theme::Theme;
use crate::types::Color;

/// GTK shows one buffer line per painted row — no cross-line colour
/// reduction (see [`crate::MinimapGrid`]'s doc for why TUI differs).
pub const LINES_PER_ROW: usize = 1;

/// Below this absolute pixel size, Pango glyph shaping reads as
/// indistinct mush rather than recognisable code shapes — the
/// rasteriser falls back to solid colour bars instead ([`render_mode`]).
pub const LEGIBILITY_FLOOR_PX: f64 = 4.0;

/// Assumed "wide" line length for the colour-bar fallback's width
/// proportion — a heuristic, not a measured value (below the legibility
/// floor there's no live font metric to measure against).
const ASSUMED_WIDE_LINE_CHARS: f64 = 120.0;

/// Compute the GTK pixel-unit layout for a [`Minimap`] without painting.
pub fn gtk_minimap_layout(minimap: &Minimap, x: f64, y: f64, w: f64, h: f64) -> MinimapLayout {
    minimap.layout(
        QRect::new(x as f32, y as f32, w as f32, h as f32),
        LINES_PER_ROW,
    )
}

/// Render technique a row's pitch selects — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimapRenderMode {
    /// Real Pango glyphs at an absolute scaled-down size.
    Characters,
    /// One filled bar per row, proportional to trimmed line length.
    ColorBars,
}

/// Pure function of the row pitch: is `line_px` legible enough to shape
/// real text? [`draw_minimap`] calls this exact function to pick a
/// branch, so it is the one source of truth for the threshold — tests
/// exercise it directly instead of needing a live Cairo surface.
pub fn is_legible(line_px: f64) -> bool {
    line_px >= LEGIBILITY_FLOOR_PX
}

/// [`MinimapRenderMode`] for a given row pitch. See [`is_legible`].
pub fn render_mode(line_px: f64) -> MinimapRenderMode {
    if is_legible(line_px) {
        MinimapRenderMode::Characters
    } else {
        MinimapRenderMode::ColorBars
    }
}

/// The absolute Pango font size (in device pixels) for a row of pitch
/// `line_px` — clamped to a sane band so a pathologically short or tall
/// minimap doesn't request a zero or absurd font size. This is a pure
/// function of the pitch, not a fixed constant: painting the same
/// buffer at two different `bounds.height` values must yield two
/// different sizes (that's the "font scaling, not a fixed 6px" contract
/// from #382).
pub fn minimap_font_px(line_px: f64) -> f64 {
    line_px.clamp(1.0, 64.0)
}

/// Fraction of the minimap's width a colour bar should fill for a line
/// whose trimmed length is `trimmed_chars`, in the [`MinimapRenderMode::ColorBars`]
/// fallback. There is no live font metric to measure against below the
/// legibility floor, so this is a heuristic proportion, not an exact
/// character-width computation.
pub fn bar_width_fraction(trimmed_chars: usize) -> f64 {
    (trimmed_chars as f64 / ASSUMED_WIDE_LINE_CHARS).clamp(0.0, 1.0)
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

    let row_px = h / layout.visible_lines.len() as f64;
    let mode = render_mode(row_px);
    let saved_font = pango_layout.font_description();

    for vline in &layout.visible_lines {
        let Some(line) = minimap.lines.get(vline.start_line_idx) else {
            continue;
        };
        match mode {
            MinimapRenderMode::Characters => paint_row_glyphs(
                cr,
                pango_layout,
                saved_font.as_ref(),
                vline,
                row_px,
                &line.text,
                vline.start_line_idx,
                minimap,
                theme,
            ),
            MinimapRenderMode::ColorBars => {
                paint_row_bar(cr, vline, &line.text, vline.start_line_idx, minimap, theme)
            }
        }
    }

    pango_layout.set_attributes(None);
    pango_layout.set_font_description(saved_font.as_ref());

    layout
}

/// Dominant colour for `start_line_idx`, from whichever aggregated
/// [`crate::MinimapSpan`] covers the most columns of that line (GTK's
/// `1x1` aggregation grid means at most a handful of spans per line —
/// see [`crate::aggregate_spans`]). Falls back to `default_fg`.
fn dominant_color(minimap: &Minimap, start_line_idx: usize, default_fg: Color) -> Color {
    minimap
        .syntax_spans
        .iter()
        .filter(|s| s.line_idx == start_line_idx)
        .max_by_key(|s| s.end_col.saturating_sub(s.start_col))
        .map(|s| s.color)
        .unwrap_or(default_fg)
}

#[allow(clippy::too_many_arguments)]
fn paint_row_glyphs(
    cr: &Context,
    pango_layout: &pango::Layout,
    saved_font: Option<&pango::FontDescription>,
    vline: &VisibleMinimapLine,
    row_px: f64,
    text: &str,
    start_line_idx: usize,
    minimap: &Minimap,
    theme: &Theme,
) {
    let mut font = saved_font.cloned().unwrap_or_default();
    font.set_absolute_size(minimap_font_px(row_px) * pango::SCALE as f64);
    pango_layout.set_font_description(Some(&font));
    pango_layout.set_text(text);

    let spans: Vec<_> = minimap
        .syntax_spans
        .iter()
        .filter(|s| s.line_idx == start_line_idx)
        .collect();

    if spans.is_empty() {
        pango_layout.set_attributes(None);
    } else {
        let to_u16 = |c: u8| -> u16 { ((c as u16) << 8) | c as u16 };
        let attrs = pango::AttrList::new();
        for span in &spans {
            let (start, end) = char_range_to_byte_range(text, span.start_col, span.end_col);
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

fn paint_row_bar(
    cr: &Context,
    vline: &VisibleMinimapLine,
    text: &str,
    start_line_idx: usize,
    minimap: &Minimap,
    theme: &Theme,
) {
    let trimmed = text.trim_end().chars().count();
    let frac = bar_width_fraction(trimmed);
    if frac <= 0.0 {
        return;
    }
    let color = dominant_color(minimap, start_line_idx, theme.muted_fg);
    set_source(cr, color);
    cr.rectangle(
        vline.bounds.x as f64,
        vline.bounds.y as f64,
        vline.bounds.width as f64 * frac,
        vline.bounds.height as f64,
    );
    cr.fill().ok();
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
    use crate::primitives::minimap::{MinimapHit, MinimapLine, MinimapSpan};
    use crate::types::WidgetId;
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

    // ── font scaling, not a fixed size ──────────────────────────────

    #[test]
    fn same_buffer_at_two_heights_yields_two_different_font_sizes() {
        let lines: Vec<&str> = vec!["fn main() {}"; 8];
        let mm = minimap_from(lines, 8);

        let tall = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 80.0); // pitch 10px
        let short = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 16.0); // pitch 2px

        let tall_px = 80.0 / tall.visible_lines.len() as f64;
        let short_px = 16.0 / short.visible_lines.len() as f64;
        assert_ne!(tall_px, short_px);

        let tall_font = minimap_font_px(tall_px);
        let short_font = minimap_font_px(short_px);
        assert_ne!(
            tall_font, short_font,
            "font size must track row pitch, not a fixed constant"
        );
    }

    // ── legibility floor ─────────────────────────────────────────────

    #[test]
    fn legibility_floor_switches_render_mode_on_both_sides() {
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX - 0.1),
            MinimapRenderMode::ColorBars
        );
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX),
            MinimapRenderMode::Characters
        );
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX + 4.0),
            MinimapRenderMode::Characters
        );
    }

    #[test]
    fn below_floor_paints_bars_and_shapes_no_text_above_it_shapes_text() {
        let mm = minimap_from(vec!["fn main() {}"; 4], 4);
        let (cr, layout) = surface_and_layout();

        // 4 rows over a 4px-tall area -> pitch 1px, well under the floor.
        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 4.0, &mm, &Theme::default());
        assert_eq!(
            layout.text(),
            "",
            "below the legibility floor, draw_minimap must never call set_text"
        );

        // Same buffer, 4 rows over a 400px-tall area -> pitch 100px, well
        // above the floor.
        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 400.0, &mm, &Theme::default());
        assert_eq!(
            layout.text(),
            "fn main() {}",
            "above the legibility floor, draw_minimap must shape the row's text"
        );
    }

    // ── colour aggregation lookup ────────────────────────────────────

    #[test]
    fn dominant_color_picks_the_widest_span_for_the_line() {
        let mut mm = minimap_from(vec!["abcdef"], 1);
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 0,
            end_col: 1,
            color: red,
        });
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 1,
            end_col: 6,
            color: blue,
        });
        assert_eq!(dominant_color(&mm, 0, Color::rgb(1, 1, 1)), blue);
    }

    #[test]
    fn dominant_color_falls_back_to_default_with_no_spans() {
        let mm = minimap_from(vec!["abc"], 1);
        let fallback = Color::rgb(9, 9, 9);
        assert_eq!(dominant_color(&mm, 0, fallback), fallback);
    }

    // ── paint/click round trip ────────────────────────────────────────

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        let mm = minimap_from(vec!["x"; 8], 8);
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
        assert_eq!(
            layout.hit_test(20.0, 50.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(20.0, 0.0),
            MinimapHit::Seek { fraction: 0.0 }
        );
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
}
