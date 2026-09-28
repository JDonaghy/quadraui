//! Direct2D / DirectWrite rasteriser for [`crate::MessageList`] (issue
//! #30).
//!
//! Painting moved to the shared
//! [`crate::primitives::message_list::native_surface_paint::paint`]
//! (#1084, `NativeSurface` Phase 4 7/8) — see that fn's doc for the full
//! per-backend divergence survey (most notably: `macos::message_list`
//! used to ignore `row.spans` entirely; this module's own italic/
//! underline/scale gap, described below, is unchanged by the move).
//! [`draw_message_list`] below is now a thin wrapper over the shared
//! paint, using [`crate::win::surface::D2dSurface`] as the `NativeSurface`
//! adapter. `gtk::message_list::draw_message_list` is *not* migrated; see
//! the shared `paint`'s doc for why.
//!
//! # Styled rows
//!
//! When a row's `spans` vector is **non-empty**, each span paints in its
//! own `fg` (falling back to `row.fg`) and `bold` weight, via
//! [`NativeSurface::surface_draw_text_run_styled`]'s `D2dSurface`
//! implementation (`DWrite::draw_text_styled`). `italic` / `underline` /
//! `scale` are **not yet** applied: `DWrite` has no italic text format,
//! underline attribute, or per-run font-scale wired up today (GTK applies
//! these through Pango's `AttrList`, TUI through ratatui `Modifier`s —
//! neither has a Direct2D equivalent yet). A future issue can add them
//! once a consumer needs rich message-list rows on Windows; until then a
//! styled row still renders correctly coloured, bold-aware text, which
//! is what distinguishes it from the flat path.
//!
//! When `spans` is **empty** the rasteriser falls back to the flat
//! `row.text` + `row.fg` path — output matches the pre-styled-row
//! behaviour every other backend preserves.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod message_list;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::message_list::MessageList;

/// Draw a [`MessageList`] into a rectangular region.
///
/// `(x, y)` is the top-left of the message area in DIPs; `max_y` is the
/// bottom edge — rows whose top would land at or past `max_y` are
/// skipped. `line_height` is the per-row DIP height.
pub fn draw_message_list(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    list: &MessageList,
    x: f32,
    y: f32,
    max_y: f32,
    line_height: f32,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    // No real width is known at this call site (see the shared `paint`'s
    // doc, "Zero-size guard") — `rect.width` is unused by `paint`, so
    // `0.0` is an honest placeholder rather than a magic sentinel.
    let rect = Rect::new(x, y, 0.0, max_y - y);
    crate::primitives::message_list::native_surface_paint::paint(
        list,
        &mut surface,
        rect,
        line_height,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::message_list::MessageRow;
    use crate::types::{Color, StyledSpan, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 240;
    const H: u32 = 160;
    const LINE_HEIGHT: f32 = 16.0;
    const PANEL_BG: Color = Color::rgb(13, 13, 13);

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0, None)
            .expect("create DWrite")
            .0
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

    fn paint(list: &MessageList) -> HeadlessSurface {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        surface
            .fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), PANEL_BG)
            .expect("fill panel bg");
        let dwrite = dwrite();
        surface
            .paint(|target| {
                draw_message_list(target, &dwrite, list, 0.0, 0.0, H as f32, LINE_HEIGHT);
            })
            .expect("paint");
        surface
    }

    fn pixel_differs_from(s: &HeadlessSurface, x: u32, y: u32, base: Color) -> bool {
        let px = s.pixel_at(x, y);
        (px.r, px.g, px.b) != (base.r, base.g, base.b)
    }

    #[test]
    fn rows_paint_glyphs_above_panel_bg() {
        let list = sample_list();
        let s = paint(&list);
        let mut found = [false; 3];
        for (band, slot) in found.iter_mut().enumerate() {
            let band_u = band as u32;
            let y_top = band_u * LINE_HEIGHT as u32;
            let y_bot = (band_u + 1) * LINE_HEIGHT as u32;
            'scan: for y in y_top..y_bot.min(H) {
                for x in 0..40u32.min(W) {
                    if pixel_differs_from(&s, x, y, PANEL_BG) {
                        *slot = true;
                        break 'scan;
                    }
                }
            }
        }
        assert!(
            found.iter().all(|f| *f),
            "expected non-panel-bg pixels in rows 0..3, found = {:?}",
            found
        );
    }

    #[test]
    fn scroll_top_skips_leading_rows() {
        let mut list = sample_list();
        list.scroll_top = 2;
        let s = paint(&list);
        let mut has_paint = false;
        'outer: for y in 0..LINE_HEIGHT as u32 {
            for x in 0..30u32.min(W) {
                if pixel_differs_from(&s, x, y, PANEL_BG) {
                    has_paint = true;
                    break 'outer;
                }
            }
        }
        assert!(has_paint, "scrolled top band should have row paint");
    }

    #[test]
    fn rows_past_max_y_are_clipped_without_crashing() {
        let mut list = sample_list();
        for i in 0..200 {
            list.rows.push(MessageRow::new(
                format!("row {i}"),
                Color::rgb(200, 200, 200),
                0.0,
            ));
        }
        let s = paint(&list);
        let px = s.pixel_at(W - 1, H - 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (PANEL_BG.r, PANEL_BG.g, PANEL_BG.b),
            "far bottom-right corner should remain untouched panel bg"
        );
    }

    /// A styled row (non-empty `spans`) must paint without error and
    /// leave visible glyph paint in its band — the "renders styled
    /// rows" acceptance criterion (#30).
    #[test]
    fn styled_row_paints_per_span_colour() {
        let mut list = sample_list();
        list.rows = vec![MessageRow {
            text: "bold text".into(),
            fg: Color::rgb(220, 220, 220),
            indent: 0.0,
            spans: vec![
                StyledSpan {
                    text: "bold".into(),
                    fg: Some(Color::rgb(255, 0, 0)),
                    bg: None,
                    bold: true,
                    italic: false,
                    underline: false,
                },
                StyledSpan::plain(" text"),
            ],
            scale: 1.0,
        }];
        let s = paint(&list);
        let mut has_paint = false;
        'outer: for y in 0..LINE_HEIGHT as u32 {
            for x in 0..60u32.min(W) {
                if pixel_differs_from(&s, x, y, PANEL_BG) {
                    has_paint = true;
                    break 'outer;
                }
            }
        }
        assert!(has_paint, "styled row should paint glyph pixels");
    }
}
