//! Direct2D / DirectWrite rasteriser for [`crate::TextDisplay`] (issue
//! #30).
//!
//! Mirrors [`crate::macos::text_display::draw_text_display`]: optional
//! title row at the top, body rows painted from
//! `resolved_scroll_offset` (auto-scroll pin-to-bottom is handled
//! entirely inside [`TextDisplay::layout`]/[`TextDisplay::layout_with_scrollbar`]
//! — this rasteriser doesn't special-case `auto_scroll` itself, it just
//! calls through), per-line spans and timestamps, optional scrollbar
//! gutter + thumb at the trailing edge.
//!
//! [`win_text_display_layout`] exposes the same layout function the
//! rasteriser uses so hosts can drive hit-testing for scrollbar drag
//! interaction without re-deriving metrics — one layout per frame,
//! source-of-truth contract (matches `gtk_text_display_layout` /
//! `mac_text_display_layout`).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod text_display;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::text_display::{TextDisplay, TextDisplayLayout, TextDisplayLineMeasure};
use crate::theme::Theme;
use crate::types::Decoration;

/// Scrollbar gutter width in DIPs. Matches the GTK/macOS rasterisers'
/// 12-DIP gutter so the body width — and therefore the resolved layout
/// — stays parity-equivalent across every pixel backend.
const SCROLLBAR_GUTTER_DIP: f32 = 12.0;

/// Minimum scrollbar thumb length in DIPs.
const SCROLLBAR_MIN_THUMB_DIP: f32 = 8.0;

/// Compute the layout the Win-GUI rasteriser would produce for
/// `display` at `rect` with the supplied `line_height`. Hosts call this
/// to drive hit-testing for scrollbar drag interaction without
/// re-deriving metrics.
///
/// The returned layout's coordinates are **body-local** (y=0 at the top
/// of the body region). Title-bar painting consumes one `line_height`
/// strip above; the body height passed to the primitive shrinks by that
/// strip when `title` is present — same contract as
/// `gtk_text_display_layout` / `mac_text_display_layout`.
pub fn win_text_display_layout(
    display: &TextDisplay,
    rect: Rect,
    line_height: f32,
) -> TextDisplayLayout {
    let body_h = if display.title.is_some() {
        (rect.height - line_height).max(0.0)
    } else {
        rect.height
    };
    if body_h <= 0.0 {
        return display.layout(0.0, 0.0, |_| TextDisplayLineMeasure::new(line_height));
    }
    if display.show_scrollbar {
        display.layout_with_scrollbar(
            rect.width,
            body_h,
            SCROLLBAR_GUTTER_DIP,
            SCROLLBAR_MIN_THUMB_DIP,
            |_| TextDisplayLineMeasure::new(line_height),
        )
    } else {
        display.layout(rect.width, body_h, |_| {
            TextDisplayLineMeasure::new(line_height)
        })
    }
}

/// Draw a [`TextDisplay`] into `rect` (DIPs) on `target`.
///
/// Background is filled with [`Theme::background`]. Each visible line's
/// spans are painted with their own `fg` (falling back to the per-line
/// decoration colour or [`Theme::foreground`]) and `bold` weight.
/// Optional timestamp prefix renders in [`Theme::muted_fg`].
#[allow(clippy::too_many_arguments)]
pub fn draw_text_display(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    display: &TextDisplay,
    theme: &Theme,
    line_height: f32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }

    let _ = fill_rect(target, rect, theme.background);

    // Optional title row at the top. Body shrinks by `line_height` when
    // present.
    let (body_y, body_h) = if let Some(ref title) = display.title {
        let mut cursor_x = rect.x;
        for span in &title.spans {
            let span_fg = span.fg.unwrap_or(theme.foreground);
            let (sw, _) = dwrite.measure_text(&span.text).unwrap_or((0.0, 0.0));
            let _ = dwrite.draw_text(
                target,
                &span.text,
                Rect::new(cursor_x, rect.y, sw.max(1.0), line_height),
                span_fg,
            );
            cursor_x += sw;
        }
        (rect.y + line_height, (rect.height - line_height).max(0.0))
    } else {
        (rect.y, rect.height)
    };
    if body_h <= 0.0 {
        return;
    }

    let layout = win_text_display_layout(
        display,
        Rect::new(rect.x, body_y, rect.width, body_h),
        line_height,
    );

    for vis in &layout.visible_lines {
        let line = &display.lines[vis.line_idx];
        let row_y = body_y + vis.bounds.y;
        if row_y + line_height > body_y + body_h {
            break;
        }

        let line_fg = match line.decoration {
            Decoration::Error => theme.error_fg,
            Decoration::Warning => theme.warning_fg,
            Decoration::Muted => theme.muted_fg,
            _ => theme.foreground,
        };

        let mut cursor_x = rect.x;

        if let Some(ref ts) = line.timestamp {
            let (tw, _) = dwrite.measure_text(ts).unwrap_or((0.0, 0.0));
            let _ = dwrite.draw_text(
                target,
                ts,
                Rect::new(cursor_x, row_y, tw.max(1.0), line_height),
                theme.muted_fg,
            );
            cursor_x += tw + 6.0;
        }

        for span in &line.spans {
            let span_fg = span.fg.unwrap_or(line_fg);
            let (sw, _) = dwrite
                .measure_text_styled(&span.text, span.bold)
                .unwrap_or((0.0, 0.0));
            if let Some(span_bg) = span.bg {
                let _ = fill_rect(target, Rect::new(cursor_x, row_y, sw, line_height), span_bg);
            }
            let _ = dwrite.draw_text_styled(
                target,
                &span.text,
                Rect::new(cursor_x, row_y, sw.max(1.0), line_height),
                span_fg,
                span.bold,
            );
            cursor_x += sw;
        }
    }

    // Scrollbar gutter.
    if display.show_scrollbar {
        if let Some(gutter) = layout.scrollbar_bounds {
            let _ = fill_rect(
                target,
                Rect::new(
                    rect.x + gutter.x,
                    body_y + gutter.y,
                    gutter.width,
                    gutter.height,
                ),
                theme.scrollbar_track,
            );
        }
        if let Some(thumb) = layout.thumb_bounds {
            let inset = 2.0;
            let _ = fill_rect(
                target,
                Rect::new(
                    rect.x + thumb.x + inset,
                    body_y + thumb.y,
                    (thumb.width - inset * 2.0).max(2.0),
                    thumb.height,
                ),
                theme.scrollbar_thumb,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::text_display::{TextDisplayHit, TextDisplayLine};
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 240;
    const H: u32 = 160;
    const LINE_HEIGHT: f32 = 16.0;

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0).expect("create DWrite").0
    }

    fn line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(lines: usize, show_scrollbar: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..lines).map(|i| line(&format!("ln{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    fn paint(td: &TextDisplay) -> (HeadlessSurface, TextDisplayLayout) {
        paint_at(td, Rect::new(0.0, 0.0, W as f32, H as f32))
    }

    fn paint_at(td: &TextDisplay, rect: Rect) -> (HeadlessSurface, TextDisplayLayout) {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_text_display(target, &dwrite, rect, td, &theme, LINE_HEIGHT);
            })
            .expect("paint");
        let layout = win_text_display_layout(td, rect, LINE_HEIGHT);
        (surface, layout)
    }

    #[test]
    fn background_fills_theme_background() {
        let td = make_td(0, false);
        let (s, _) = paint(&td);
        let theme = Theme::default();
        let px = s.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b)
        );
    }

    #[test]
    fn scrollbar_gutter_paints_track_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint(&td);
        let gutter = layout.scrollbar_bounds.expect("gutter present");
        let probe_x = (gutter.x + gutter.width / 2.0) as u32;
        let probe_y = (gutter.y + gutter.height - 2.0) as u32;
        let px = s.pixel_at(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b
            )
        );
    }

    #[test]
    fn scrollbar_thumb_paints_thumb_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint(&td);
        let thumb = layout.thumb_bounds.expect("thumb present");
        let probe_x = (thumb.x + thumb.width / 2.0) as u32;
        let probe_y = (thumb.y + thumb.height / 2.0) as u32;
        let px = s.pixel_at(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b
            )
        );
    }

    #[test]
    fn layout_hit_test_resolves_lines() {
        let td = make_td(20, false);
        let (_, layout) = paint(&td);
        let vis = &layout.visible_lines[0];
        let cx = vis.bounds.x + vis.bounds.width / 2.0;
        let cy = vis.bounds.y + vis.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    /// Regression guard for quadraui#494 ("layout helpers must return
    /// coords in the same frame across backends"): paint at a non-zero
    /// rect origin with a title row present, then round-trip an
    /// absolute click the way a real host does.
    #[test]
    fn layout_hit_test_resolves_lines_at_nonzero_origin() {
        let mut td = make_td(20, false);
        td.title = Some(StyledText::plain("Logs"));

        let rect_x = 9.0_f32;
        let rect_y = 17.0_f32;
        let rect = Rect::new(rect_x, rect_y, W as f32 - rect_x, H as f32 - rect_y);
        let (_surface, layout) = paint_at(&td, rect);
        let body_y = rect_y + LINE_HEIGHT;

        let vis = &layout.visible_lines[0];
        assert_eq!(
            vis.bounds.y, 0.0,
            "visible_lines bounds.y must be body-local"
        );

        let abs_x = rect_x + vis.bounds.x + vis.bounds.width * 0.5;
        let abs_y = body_y + vis.bounds.y + vis.bounds.height * 0.5;
        let local_x = abs_x - rect_x;
        let local_y = abs_y - body_y;
        match layout.hit_test(local_x, local_y) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    /// `auto_scroll == true` must pin the resolved layout to the bottom
    /// (newest lines visible) regardless of `scroll_offset` — the
    /// primitive's `layout()` handles this; this test just confirms the
    /// Win-GUI rasteriser routes through it rather than special-casing
    /// `scroll_offset` itself.
    #[test]
    fn auto_scroll_pins_to_the_bottom() {
        let mut td = make_td(200, false);
        td.auto_scroll = true;
        td.scroll_offset = 0;

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = win_text_display_layout(&td, rect, LINE_HEIGHT);

        let last_idx = td.lines.len() - 1;
        assert_eq!(
            layout.visible_lines.last().map(|v| v.line_idx),
            Some(last_idx),
            "auto_scroll should keep the newest line visible at the bottom"
        );
        assert!(
            layout.resolved_scroll_offset > 0,
            "resolved_scroll_offset should have advanced past 0 to reach the bottom"
        );
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_text_display` painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let td = make_td(30, true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let (_surface, painted) = paint_at(&td, rect);
        let no_paint = win_text_display_layout(&td, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }
}
