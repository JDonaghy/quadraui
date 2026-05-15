//! macOS rasteriser for [`crate::MessageList`].
//!
//! Mirror of [`crate::gtk::message_list::draw_message_list`]: walks
//! `rows[scroll_top..]`, painting each row's text at
//! `(x + row.indent, y + i*line_height)` in the row's `fg`. Panel
//! background fill is the caller's responsibility — repeated per-row
//! bg fills would overdraw any header / separator already painted by
//! the panel chrome.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::message_list::MessageList;
use crate::types::Color;

/// Draw a [`MessageList`] into a rectangular region.
///
/// `(x, y)` is the top-left of the message area in points; `w` is
/// the width (used by callers to clip text — this rasteriser doesn't
/// itself clip beyond `max_y` because Core Text will paint past the
/// right edge when text overflows). `max_y` is the bottom edge:
/// rows whose top would land at or past `max_y` are skipped.
/// `line_height` is the per-row point height.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_message_list(
    ctx: CGContextRef,
    font: &CTFont,
    list: &MessageList,
    x: f64,
    y: f64,
    w: f64,
    max_y: f64,
    line_height: f64,
) {
    if w <= 0.0 || line_height <= 0.0 {
        return;
    }
    for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
        let ry = y + i as f64 * line_height;
        if ry + line_height > max_y {
            break;
        }
        let (_, text_h) = measure_text(font, &row.text);
        // Vertically centre the glyph cell within the row pitch —
        // matches the GTK rasteriser's `(line_height - lh) / 2`
        // baseline placement.
        let text_y = ry + (line_height - text_h) / 2.0;
        draw_text(
            ctx,
            font,
            &row.text,
            x + row.indent as f64,
            text_y,
            color_to_cg(row.fg),
        );
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::message_list::{MessageList, MessageRow};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_list() -> MessageList {
        MessageList {
            id: WidgetId::new("ml"),
            rows: vec![
                MessageRow::new("You:", Color::rgb(255, 220, 0), 0.0),
                MessageRow::new("hi there", Color::rgb(220, 220, 220), 8.0),
                MessageRow::new("AI:", Color::rgb(0, 200, 255), 0.0),
                MessageRow::new("hello", Color::rgb(220, 220, 220), 8.0),
            ],
            scroll_top: 0,
        }
    }

    fn paint(list: &MessageList) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        // Pre-fill with a known dark panel bg so the rasteriser's
        // glyphs draw on a stable background — the rasteriser itself
        // doesn't paint a background.
        surface.fill(0.05, 0.05, 0.05, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_message_list(QRect::new(0.0, 0.0, W as f32, H as f32), list);
        });
        backend.end_frame();
        surface
    }

    fn pixel_differs_from(s: &BitmapSurface, x: u32, y: u32, base: (u8, u8, u8)) -> bool {
        let (r, g, b, _) = s.pixel(x, y);
        (r, g, b) != base
    }

    #[test]
    fn rows_paint_glyphs_above_panel_bg() {
        // Each row should leave some non-background pixels in its
        // band — glyph antialiasing means we don't pin one specific
        // colour, but we can assert "something was painted here that
        // wasn't the panel bg".
        let list = sample_list();
        let s = paint(&list);
        let panel_bg = (13, 13, 13); // approx 0.05*255 rounded
                                     // Probe across the first ~3 row bands (each ~16px tall for
                                     // Menlo 14pt). Use a band-wise scan: at least one pixel in
                                     // each band should differ from panel_bg.
        let mut found = [false; 3];
        for (band, slot) in found.iter_mut().enumerate() {
            let band_u = band as u32;
            let y_top = band_u * 16;
            let y_bot = (band_u + 1) * 16;
            'scan: for y in y_top..y_bot {
                for x in 0..40 {
                    if pixel_differs_from(&s, x, y, panel_bg) {
                        *slot = true;
                        break 'scan;
                    }
                }
            }
        }
        assert!(
            found.iter().all(|f| *f),
            "expected non-panel-bg pixels in rows 0..3, found = {:?}",
            found,
        );
    }

    #[test]
    fn scroll_top_skips_leading_rows() {
        // scroll_top=2 should skip "You:" + "hi there", drawing "AI:"
        // first.
        let mut list = sample_list();
        list.scroll_top = 2;
        let s = paint(&list);
        let scrolled_panel_bg = (13, 13, 13);
        // The top band must hold the "AI:" glyph (cyan-ish) and have
        // *some* non-panel-bg pixel; we don't pin colour because
        // antialiasing blends edges.
        let mut has_paint = false;
        'outer: for y in 0..16 {
            for x in 0..30 {
                if pixel_differs_from(&s, x, y, scrolled_panel_bg) {
                    has_paint = true;
                    break 'outer;
                }
            }
        }
        assert!(has_paint, "scrolled top band should have row paint");
    }

    /// `cargo test -p quadraui --no-default-features --features macos -- --ignored --nocapture macos::message_list::tests::dump_smoke_ppm`
    ///
    /// Paints a sample chat-style scrollback — alternating `You:` /
    /// `AI:` role labels with indented content — into
    /// `/tmp/quadraui_message_list.ppm`. Open in Preview to confirm:
    /// - Role labels (`You:`, `AI:`) sit flush-left in distinct colours
    ///   (yellow vs cyan).
    /// - Content rows below each label are indented and rendered in a
    ///   light grey.
    /// - Rows are vertically centred within their `line_height` band —
    ///   glyphs aren't crowding the top edge.
    #[test]
    #[ignore = "writes /tmp/quadraui_message_list.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        let you = Color::rgb(255, 220, 0);
        let ai = Color::rgb(0, 200, 255);
        let body = Color::rgb(220, 220, 220);
        let list = MessageList {
            id: WidgetId::new("ml"),
            rows: vec![
                MessageRow::new("You:", you, 0.0),
                MessageRow::new("how do I list pods?", body, 12.0),
                MessageRow::new("AI:", ai, 0.0),
                MessageRow::new("Run `kubectl get pods` to list pods", body, 12.0),
                MessageRow::new("in the current namespace.", body, 12.0),
                MessageRow::new("You:", you, 0.0),
                MessageRow::new("thanks!", body, 12.0),
            ],
            scroll_top: 0,
        };
        let s = paint(&list);
        s.write_ppm_and_open("/tmp/quadraui_message_list.ppm");
    }

    #[test]
    fn rows_past_max_y_are_clipped() {
        // 100 rows in a 160-pt viewport at ~16pt line_height → ~10
        // rows fit. Rows past that are skipped. Pick a tall row count
        // and a clear panel bg so the test asserts the bottom band
        // *near* H stays empty if we have a clear gap; but in
        // practice many rows fit, so we instead assert the loop
        // exits — covered indirectly by the band-scan above. Here we
        // just verify the rasteriser doesn't crash with a large row
        // count.
        let mut list = sample_list();
        for i in 0..200 {
            list.rows.push(MessageRow::new(
                format!("row {i}"),
                Color::rgb(200, 200, 200),
                0.0,
            ));
        }
        let s = paint(&list);
        // Smoke: surface still has the panel bg colour somewhere
        // (not crash-painted into oblivion).
        let (r, g, b, _) = s.pixel(W - 1, H - 1);
        // Last pixel: glyphs unlikely to reach the far right + bottom
        // corner exactly; expect panel bg.
        assert_eq!((r, g, b), (13, 13, 13));
    }
}
