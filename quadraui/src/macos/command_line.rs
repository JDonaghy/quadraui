//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::command_line::CommandLine`].
//!
//! Straight port of [`crate::gtk::command_line::draw_command_line`]:
//! fill the bar rect with `theme.command_line_bg`, draw the text in
//! `theme.command_line_fg` (left- or right-aligned), and optionally
//! paint a 2-point insert cursor at `cursor_offset`.
//!
//! ## Divergence from the GTK twin (deliberate)
//!
//! GTK anchors the cursor at `x + prefix_width`, i.e. it ignores the
//! right-alignment shift. That is only ever visible for a right-aligned
//! command line that *also* carries a cursor — a combination the vim-style
//! count/match displays never produce — so the bug has never bitten. This
//! rasteriser anchors at `text_x + prefix_width` instead, which agrees with
//! GTK for every left-aligned case and is simply correct for the other.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::command_line::CommandLine;
use crate::theme::Theme;
use crate::types::Color;

/// Insert-cursor width in points. Matches the GTK rasteriser's 2px bar.
const CURSOR_W: f64 = 2.0;

/// Paint `cmd` into the rect `(x, y, width, line_height)` on `ctx`.
///
/// The command line carries no hit regions — it is display-only, with
/// keystroke handling owned by the app's editor engine — so nothing is
/// returned.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_command_line(
    ctx: CGContextRef,
    font: &CTFont,
    cmd: &CommandLine,
    theme: &Theme,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
) {
    if width <= 0.0 || line_height <= 0.0 {
        return;
    }

    CGContextSaveGState(ctx);
    // Clip so an over-long command (or a right-aligned string wider than
    // the bar) truncates at the bar edges instead of painting past them.
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, width, line_height));

    fill_rect(ctx, x, y, width, line_height, theme.command_line_bg);

    if cmd.text.is_empty() {
        CGContextRestoreGState(ctx);
        return;
    }

    let text_x = if cmd.right_align {
        let (text_w, _) = measure_text(font, &cmd.text);
        x + width - text_w
    } else {
        x
    };
    draw_text(
        ctx,
        font,
        &cmd.text,
        text_x,
        y,
        color_to_cg(theme.command_line_fg),
    );

    if let Some(offset) = cmd.cursor_offset {
        // `safe_prefix` snaps to a char boundary — a byte offset landing
        // mid-`é` must not panic (quadraui#503, the GTK twin's regression).
        let anchor = crate::text_util::safe_prefix(&cmd.text, offset);
        let (anchor_w, _) = measure_text(font, anchor);
        fill_rect(
            ctx,
            text_x + anchor_w,
            y,
            CURSOR_W,
            line_height,
            theme.cursor,
        );
    }

    CGContextRestoreGState(ctx);
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 60;

    fn sample(text: &str, cursor: Option<usize>, right_align: bool) -> CommandLine {
        CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: cursor,
            right_align,
        }
    }

    /// Paint through the real `Backend::draw_command_line` path — the
    /// same call chain the live `drawRect:` runner uses.
    fn paint_via_backend(cmd: &CommandLine, rect: QRect) -> (BitmapSurface, f64) {
        let surface = BitmapSurface::new(W, H);
        // Known non-theme background so "did the bar fill happen here?"
        // is answerable per pixel.
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        let line_height = backend.line_height() as f64;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_command_line(rect, cmd);
        });
        backend.end_frame();
        (surface, line_height)
    }

    #[test]
    fn fills_the_bar_background_at_origin() {
        let cmd = sample(":wq", None, false);
        let (surface, lh) = paint_via_backend(&cmd, QRect::new(0.0, 0.0, W as f32, 20.0));
        let theme = Theme::default();
        // Probe well right of the ":wq" glyphs but inside the bar.
        let probe_y = (lh / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(W - 4, probe_y);
        assert_eq!(
            (r, g, b),
            (
                theme.command_line_bg.r,
                theme.command_line_bg.g,
                theme.command_line_bg.b
            ),
            "command line background should cover the full bar width",
        );
    }

    /// Non-zero-origin regression guard (LESSONS.md:159-181): the bar
    /// must paint where it was asked to, and must leave the rows above
    /// it untouched.
    #[test]
    fn fills_the_bar_background_at_nonzero_origin() {
        let origin_x = 24.0_f32;
        let origin_y = 30.0_f32;
        let cmd = sample(":wq", None, false);
        let (surface, _lh) = paint_via_backend(
            &cmd,
            QRect::new(origin_x, origin_y, W as f32 - origin_x, 20.0),
        );
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W - 4, origin_y as u32 + 4);
        assert_eq!(
            (r, g, b),
            (
                theme.command_line_bg.r,
                theme.command_line_bg.g,
                theme.command_line_bg.b
            ),
            "bar should paint at the requested origin",
        );

        // Left of `origin_x` and above `origin_y`: untouched white.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(4, origin_y as u32 + 4);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint left of the bar origin",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(W - 4, origin_y as u32 - 4);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint above the bar origin",
        );
    }

    #[test]
    fn cursor_paints_cursor_colour_after_the_prefix() {
        let cmd = sample(":wq", Some(1), false);
        let (surface, lh) = paint_via_backend(&cmd, QRect::new(0.0, 0.0, W as f32, 20.0));
        let theme = Theme::default();
        let font = make_font("Menlo", 14.0).expect("Menlo installed");
        let (prefix_w, _) = measure_text(&font, ":");

        let px = (prefix_w + CURSOR_W / 2.0) as u32;
        let py = (lh / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at the prefix width",
        );
    }

    /// Regression for the multibyte panic the GTK twin fixed under
    /// quadraui#503: a `cursor_offset` landing mid-character must snap,
    /// not slice a `str` at a non-boundary.
    #[test]
    fn multibyte_cursor_offset_does_not_panic() {
        let text = ":éditer";
        assert!(!text.is_char_boundary(2));
        let cmd = sample(text, Some(2), false);
        let _ = paint_via_backend(&cmd, QRect::new(0.0, 0.0, W as f32, 20.0));
    }

    #[test]
    fn right_aligned_text_is_pushed_to_the_right_edge() {
        // The right-aligned string's glyphs must land in the right half.
        // Probe: with the bar filled, at least one pixel in the right
        // quarter differs from the bar background (a glyph), while the
        // left quarter is pure background.
        let cmd = sample("3/17", None, true);
        let (surface, lh) = paint_via_backend(&cmd, QRect::new(0.0, 0.0, W as f32, 20.0));
        let theme = Theme::default();
        let bg = (
            theme.command_line_bg.r,
            theme.command_line_bg.g,
            theme.command_line_bg.b,
        );
        let row = (lh / 2.0) as u32;

        let left_quarter_all_bg = (0..W / 4).all(|x| {
            let (r, g, b, _) = surface.pixel(x, row);
            (r, g, b) == bg
        });
        assert!(
            left_quarter_all_bg,
            "right-aligned text must not paint into the left quarter",
        );

        let right_quarter_has_glyph = (W - W / 4..W).any(|x| {
            (0..(lh as u32).min(H)).any(|dy| {
                let (r, g, b, _) = surface.pixel(x, dy);
                (r, g, b) != bg
            })
        });
        assert!(
            right_quarter_has_glyph,
            "right-aligned text should paint glyphs in the right quarter",
        );
    }

    #[test]
    fn zero_width_rect_is_a_no_op() {
        let cmd = sample(":wq", Some(1), false);
        let (surface, _lh) = paint_via_backend(&cmd, QRect::new(0.0, 0.0, 0.0, 20.0));
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(1, 1);
                (r, g, b)
            },
            (255, 255, 255),
            "a zero-width command line should paint nothing at all",
        );
    }

    /// Paint/click round-trip (`docs/TESTING.md` coverage-taxonomy row 1,
    /// #705 review): paint through the real `Backend::draw_command_line`
    /// path (same infra as `paint_via_backend` above), find the actual
    /// painted (non-background) pixel for two different characters, then
    /// `hit_test` those exact pixels via `Backend::command_line_layout`
    /// and assert they resolve to the right byte offsets. `MacBackend`
    /// derives `current_char_width` from the same Menlo font metrics
    /// `draw_text` paints with (`set_current_font` -> `font_metrics` ->
    /// `measure_text(font, "M")`), so — unlike the GTK twin's
    /// fixed-advance approximation — paint and layout share one
    /// ground-truth measurement here, and no separate font-width probe
    /// is needed.
    ///
    /// Non-zero-origin per LESSONS.md:159-181 (a LOCAL/ABSOLUTE mixup is
    /// invisible at `rect.x == 0`).
    #[test]
    fn command_line_layout_hit_test_matches_painted_glyph_at_nonzero_origin() {
        let origin_x = 24.0_f32;
        let origin_y = 30.0_f32;
        let rect = QRect::new(origin_x, origin_y, W as f32 - origin_x, 20.0);
        let cmd = sample(":wq", None, false);

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_command_line(rect, &cmd);
        });
        backend.end_frame();

        let layout = backend.command_line_layout(rect, &cmd);
        let char_width = backend.char_width();
        assert!(char_width > 1.0, "Menlo char_width should be several px");

        let y0 = origin_y as u32;
        let y1 = (origin_y + rect.height).min(H as f32) as u32;

        let find_painted = |x0: u32, x1: u32| -> Option<u32> {
            for x in x0..x1.min(W) {
                for y in y0..y1 {
                    let (r, g, b, _) = surface.pixel(x, y);
                    if (r, g, b) != (255, 255, 255) {
                        return Some(x);
                    }
                }
            }
            None
        };

        // Column 0 (':') interior — inset 1px from the left cell edge to
        // dodge antialiasing at the boundary (mirrors the GTK twin's
        // round-trip test).
        let col0_x0 = origin_x as u32 + 1;
        let col0_x1 = (origin_x + char_width).floor() as u32;
        let px0 = find_painted(col0_x0, col0_x1)
            .unwrap_or_else(|| panic!("column 0 (':') painted no pixel in {col0_x0}..{col0_x1}"));
        assert_eq!(layout.hit_test(px0 as f32), 0);

        // Column 1 ('w').
        let col1_x0 = (origin_x + char_width).ceil() as u32 + 1;
        let col1_x1 = (origin_x + 2.0 * char_width).floor() as u32;
        let px1 = find_painted(col1_x0, col1_x1)
            .unwrap_or_else(|| panic!("column 1 ('w') painted no pixel in {col1_x0}..{col1_x1}"));
        assert_eq!(layout.hit_test(px1 as f32), 1);

        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        assert_eq!(layout.hit_test(origin_x), 0);
    }
}
