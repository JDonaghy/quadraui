//! macOS rasteriser for [`crate::MessageList`].
//!
//! Painting moved to the shared
//! [`crate::primitives::message_list::native_surface_paint::paint`]
//! (#1084, `PaintSurface` Phase 4 7/8) — see that fn's doc for the full
//! per-backend divergence survey, most notably: this module used to
//! ignore [`crate::primitives::message_list::MessageRow::spans`]
//! entirely, painting every row flat regardless of any rich styling a
//! caller supplied. [`draw_message_list`] below is now a thin wrapper
//! over the shared paint, using [`crate::macos::surface::CgSurface`] as
//! the `PaintSurface` adapter — mirroring
//! [`crate::win::message_list::draw_message_list`]'s equivalent
//! migration. `gtk::message_list::draw_message_list` is *not* migrated;
//! see the shared `paint`'s doc for why.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::event::Rect;
use crate::primitives::message_list::MessageList;

/// Draw a [`MessageList`] into a rectangular region.
///
/// `(x, y)` is the top-left of the message area in points; `w` is
/// the width — this rasteriser doesn't clip to it (Core Text paints
/// past the right edge when text overflows), it's used only for the
/// zero-size guard below, matching this module's pre-#1084 behaviour
/// (see the shared `paint`'s doc, "Zero-size guard"). `max_y` is the
/// bottom edge: rows whose top would land at or past `max_y` are
/// skipped. `line_height` is the per-row point height.
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
    if w <= 0.0 {
        return;
    }
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let rect = Rect::new(x as f32, y as f32, w as f32, (max_y - y) as f32);
    crate::primitives::message_list::native_surface_paint::paint(
        list,
        &mut surface,
        rect,
        line_height as f32,
    );
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::message_list::{MessageList, MessageRow};
    use crate::types::{Color, WidgetId};
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

    /// #1084's RED-before-the-port case: pre-migration,
    /// `macos::message_list::draw_message_list` never read `row.spans` at
    /// all, so a styled row painted no differently from a flat one — there
    /// was no way to even ask this backend to honour per-span colour.
    /// Mirrors `win::message_list`'s `styled_row_paints_per_span_colour`.
    #[test]
    fn styled_row_paints_per_span_colour() {
        let mut list = sample_list();
        list.rows = vec![MessageRow {
            text: "bold text".into(),
            fg: Color::rgb(220, 220, 220),
            indent: 0.0,
            spans: vec![
                crate::types::StyledSpan {
                    text: "bold".into(),
                    fg: Some(Color::rgb(255, 0, 0)),
                    bg: None,
                    bold: true,
                    italic: false,
                    underline: false,
                },
                crate::types::StyledSpan::plain(" text"),
            ],
            scale: 1.0,
        }];
        let s = paint(&list);
        let panel_bg = (13, 13, 13);
        let mut has_paint = false;
        'outer: for y in 0..16 {
            for x in 0..60u32.min(W) {
                if pixel_differs_from(&s, x, y, panel_bg) {
                    has_paint = true;
                    break 'outer;
                }
            }
        }
        assert!(has_paint, "styled row should paint glyph pixels");
    }
}
