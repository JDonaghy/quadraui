//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::command_line::CommandLine`].
//!
//! Painting moved to the shared
//! [`crate::primitives::command_line::native_surface_paint::paint`]
//! (#1083, `PaintSurface` Phase 4 6/8) — see that fn's doc for the named
//! divergences this closed (this module used to paint no selection
//! highlight at all, and anchored its insert cursor at `text_x +
//! prefix_width` via a real Core Text glyph-prefix measurement rather
//! than the shared [`crate::primitives::command_line::CommandLineLayout::char_bounds`]
//! fixed-advance column — the latter is what the unified `paint` uses
//! now). [`draw_command_line`]/[`draw_command_line_selection`] below are
//! thin wrappers over it, using [`crate::macos::surface::CgSurface`] as
//! the `PaintSurface` adapter.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::command_line::CommandLine;
use crate::theme::Theme;

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
    char_width: f32,
) {
    draw_command_line_selection(
        ctx,
        font,
        cmd,
        theme,
        x,
        y,
        width,
        line_height,
        char_width,
        None,
    );
}

/// Paint `cmd` exactly like [`draw_command_line`], additionally painting
/// a selection highlight behind the text for `selection` — the macOS
/// side of closing #1083's "selection silently dropped" divergence (see
/// this module's doc). Sibling function, not a new parameter, mirroring
/// [`crate::gtk::command_line::draw_command_line_selection`]'s shape.
///
/// # Safety
///
/// Same contract as [`draw_command_line`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_command_line_selection(
    ctx: CGContextRef,
    font: &CTFont,
    cmd: &CommandLine,
    theme: &Theme,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    char_width: f32,
    selection: Option<(usize, usize)>,
) -> crate::primitives::command_line::CommandLineLayout {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let rect = crate::event::Rect::new(x as f32, y as f32, width as f32, line_height as f32);
    crate::primitives::command_line::native_surface_paint::paint(
        cmd,
        &mut surface,
        theme,
        rect,
        char_width,
        selection,
    )
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

    /// #1083: the cursor bar paints at the shared
    /// [`crate::primitives::command_line::CommandLineLayout::char_bounds`]
    /// column — the same rect [`Backend::command_line_layout`] would hand
    /// back for hit-testing that column — not a hand-measured Core Text
    /// prefix width (this test's pre-#1083 shape). Mirrors
    /// `win::command_line`'s `cursor_paints_at_the_shared_layout_column`.
    #[test]
    fn cursor_paints_cursor_colour_after_the_prefix() {
        let cmd = sample(":wq", Some(1), false);
        let rect = QRect::new(0.0, 0.0, W as f32, 20.0);
        let (surface, lh) = paint_via_backend(&cmd, rect);
        let theme = Theme::default();

        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        let layout = backend.command_line_layout(rect, &cmd);
        let cursor_rect = layout.char_bounds(1);

        let px = (cursor_rect.x + 1.0) as u32;
        let py = (lh / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at char_bounds(1)'s column",
        );
    }

    /// #1083: closes the divergence this module's doc names — before the
    /// port, `MacBackend::draw_command_line_selection` silently ignored
    /// `selection` and forwarded to the plain (unhighlighted) paint (see
    /// issue #1001's "no visual highlight yet" note on that method), so
    /// this assertion was RED: no pixel anywhere in the bar ever carried
    /// `theme.selection`. After the port, it paints through the shared
    /// `native_surface_paint::paint`, same as `gtk::command_line`'s
    /// `gtk_command_line_selection_paints_highlight_behind_text` twin.
    #[test]
    fn selection_paints_highlight_behind_text() {
        // A space character deliberately, so the probed pixel carries no
        // glyph ink and the highlight colour (alpha 1.0, an opaque
        // replace over the white background) can be asserted exactly.
        let cmd = sample("  x", None, false);
        let rect = QRect::new(0.0, 0.0, W as f32, 20.0);
        let theme = Theme {
            // `paint()` fills the *entire* bar rect with `command_line_bg`
            // before painting the selection highlight, so this must match
            // the white the surface was primed with below — otherwise the
            // "unselected column stays plain background" assertion checks
            // against the wrong colour. Mirrors the GTK twin
            // (`gtk_command_line_selection_paints_highlight_behind_text`),
            // which sets this explicitly for the same reason.
            command_line_bg: crate::types::Color::rgb(255, 255, 255),
            selection: crate::types::Color::rgb(0, 0, 255),
            selection_alpha: 1.0,
            ..Theme::default()
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.set_theme(theme);
        let line_height = backend.line_height() as f64;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_command_line_selection(rect, &cmd, Some((0, 1)));
        });
        backend.end_frame();

        let layout = backend.command_line_layout(rect, &cmd);
        let sel_rect = layout
            .selection_bounds((0, 1))
            .expect("non-empty selection should produce a rect");

        let px = (sel_rect.x + sel_rect.width / 2.0) as u32;
        let py = (line_height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (0, 0, 255),
            "selected column should paint the highlight colour",
        );

        // Unselected column (second space) stays plain background.
        let unsel_px = (sel_rect.x + sel_rect.width + sel_rect.width / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(unsel_px, py);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "unselected column should not paint the highlight",
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
