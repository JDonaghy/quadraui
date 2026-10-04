//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::command_line::CommandLine`] (issue #725).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod command_line;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! Painting moved to the shared
//! [`crate::primitives::command_line::native_surface_paint::paint`]
//! (#1083, `PaintSurface` Phase 4 6/8) — see that fn's doc for the named
//! divergences this closed on the other two backends (this module's own
//! `char_bounds`-based cursor and lack of any selection paint were
//! already what the port converged everyone onto, so this file's own
//! behaviour is unchanged by the move — only its implementation is now
//! shared). [`draw_command_line`]/[`draw_command_line_selection`] below
//! are thin wrappers over it, using [`crate::win::surface::D2dSurface`]
//! as the `PaintSurface` adapter.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
use crate::theme::Theme;

/// Compute [`CommandLineLayout`] for `cmd` painted at `rect`, using the
/// backend's monospace `char_width` (issue #705) — what
/// [`crate::win::WinBackend::command_line_layout`] calls directly, and
/// what [`draw_command_line`] paints against.
pub fn win_command_line_layout(
    cmd: &CommandLine,
    rect: Rect,
    char_width: f32,
) -> CommandLineLayout {
    cmd.layout(rect, CommandLineMeasure::new(char_width))
}

/// Paint `cmd` into `rect` (DIPs, target-relative) on `target` and return
/// the resolved [`CommandLineLayout`] — same contract as the GTK/macOS/
/// TUI twins' `draw_command_line`: callers (and tests) read the layout
/// back instead of re-deriving it, so paint and hit-test can't drift
/// apart (#705 review).
pub fn draw_command_line(
    target: &ID2D1RenderTarget,
    dwrite: &super::text::DWrite,
    rect: Rect,
    cmd: &CommandLine,
    theme: &Theme,
    char_width: f32,
) -> CommandLineLayout {
    draw_command_line_selection(target, dwrite, rect, cmd, theme, char_width, None)
}

/// Paint `cmd` exactly like [`draw_command_line`], additionally painting
/// a selection highlight behind the text for `selection` — the Windows
/// side of closing #1083's "selection silently dropped" divergence (see
/// this module's doc). Sibling function, not a new parameter, mirroring
/// [`crate::gtk::command_line::draw_command_line_selection`]'s shape.
#[allow(clippy::too_many_arguments)]
pub fn draw_command_line_selection(
    target: &ID2D1RenderTarget,
    dwrite: &super::text::DWrite,
    rect: Rect,
    cmd: &CommandLine,
    theme: &Theme,
    char_width: f32,
    selection: Option<(usize, usize)>,
) -> CommandLineLayout {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
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
    use super::*;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: f32 = 200.0;
    const H: f32 = 20.0;

    fn sample(text: &str, cursor: Option<usize>, right_align: bool) -> CommandLine {
        CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: cursor,
            right_align,
        }
    }

    /// Regression for the multibyte panic the GTK twin fixed under
    /// quadraui#503 (`text_area_with_multibyte_cursor_does_not_panic`'s
    /// command-line sibling, this issue's acceptance criterion): a
    /// `cursor_offset` landing mid-`é` must snap via
    /// [`CommandLineLayout::char_bounds`], not slice `text` at a
    /// non-boundary.
    #[test]
    fn draw_command_line_with_multibyte_cursor_does_not_panic() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();

        // ":éditer" — byte 2 sits inside the 2-byte 'é' (starts at byte 1).
        let text = ":éditer";
        assert!(!text.is_char_boundary(2));
        let cmd = sample(text, Some(2), false);
        let rect = Rect::new(0.0, 0.0, W, H);

        surface
            .paint(|target| {
                // Must not panic.
                draw_command_line(target, &dwrite, rect, &cmd, &theme, char_width);
            })
            .expect("paint command line");
    }

    /// Paint↔click round trip (`docs/TESTING.md` coverage-taxonomy row
    /// 1): paint via the real `draw_command_line` rasteriser into a
    /// headless Direct2D surface, find the actual painted (non-
    /// background) pixel for two different characters, then `hit_test`
    /// those exact pixels via the returned [`CommandLineLayout`] and
    /// assert they resolve to the correct byte offsets — mirrors
    /// `gtk::command_line`'s and `macos::command_line`'s #705-review
    /// twins.
    ///
    /// #505: a LOCAL/ABSOLUTE mixup is invisible at `rect.x == 0`, so
    /// this is exercised at a nonzero origin too.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 24.0_f32;
        let origin_y = 3.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        assert!(char_width > 1.0, "char_width should be several px");
        let theme = Theme {
            command_line_bg: Color::rgb(255, 255, 255),
            command_line_fg: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let cmd = sample(":wq", None, false);
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = surface
            .paint(|target| {
                draw_command_line(target, &dwrite, rect, &cmd, &theme, char_width);
            })
            .map(|_| win_command_line_layout(&cmd, rect, char_width))
            .expect("paint command line");

        let is_bg = |x: u32, y: u32| {
            let px = surface.pixel_at(x, y);
            (px.r, px.g, px.b) == (255, 255, 255)
        };
        let find_painted = |x0: u32, x1: u32, y0: u32, y1: u32| -> Option<u32> {
            for x in x0..x1.min(W as u32) {
                for y in y0..y1.min(H as u32) {
                    if !is_bg(x, y) {
                        return Some(x);
                    }
                }
            }
            None
        };

        let y0 = origin_y as u32;
        let y1 = H as u32;

        // Column 0 (':') interior — inset 1px from the left cell edge to
        // dodge antialiasing at the boundary.
        let col0_x0 = origin_x as u32 + 1;
        let col0_x1 = (origin_x + char_width).floor() as u32;
        let px0 = find_painted(col0_x0, col0_x1, y0, y1)
            .unwrap_or_else(|| panic!("column 0 (':') painted no pixel in {col0_x0}..{col0_x1}"));
        assert_eq!(layout.hit_test(px0 as f32), 0);

        // Column 1 ('w').
        let col1_x0 = (origin_x + char_width).ceil() as u32 + 1;
        let col1_x1 = (origin_x + 2.0 * char_width).floor() as u32;
        let px1 = find_painted(col1_x0, col1_x1, y0, y1)
            .unwrap_or_else(|| panic!("column 1 ('w') painted no pixel in {col1_x0}..{col1_x1}"));
        assert_eq!(layout.hit_test(px1 as f32), 1);

        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        assert_eq!(layout.hit_test(origin_x), 0);
    }

    /// The cursor bar paints at `char_bounds(offset)` — the same rect
    /// [`CommandLineLayout`] would hand back for hit-testing that column
    /// — not a hand-measured prefix width. Probing the theme's `cursor`
    /// colour at that rect's centre is the paint-side half of "no
    /// cursor-offset→x arithmetic in `win/`".
    #[test]
    fn cursor_paints_at_the_shared_layout_column() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let cmd = sample(":wq", Some(1), false);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = win_command_line_layout(&cmd, rect, char_width);
        let cursor_col_rect = layout.char_bounds(1);

        surface
            .paint(|target| {
                draw_command_line(target, &dwrite, rect, &cmd, &theme, char_width);
            })
            .expect("paint command line");

        let px = (cursor_col_rect.x + 1.0) as u32;
        let py = (cursor_col_rect.y + cursor_col_rect.height / 2.0) as u32;
        let sample_px = surface.pixel_at(px, py);
        assert_eq!(
            (sample_px.r, sample_px.g, sample_px.b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at char_bounds(1)'s column",
        );
    }

    /// #1083: closes the divergence this module's doc names — before the
    /// port, `WinBackend::draw_command_line_selection` silently ignored
    /// `selection` and forwarded to the plain (unhighlighted) paint (see
    /// issue #1001's "no visual highlight yet" note on that method), so
    /// this assertion was RED: no pixel anywhere in the bar ever carried
    /// `theme.selection`. After the port, it paints through the shared
    /// `native_surface_paint::paint`, same as `gtk::command_line`'s
    /// `gtk_command_line_selection_paints_highlight_behind_text` twin.
    ///
    /// # Why the probes never sit in a column that holds a glyph
    ///
    /// The GTK twin pays for its `"  x"` / "column 1 is an ink-free
    /// control pixel" shape with a **monospace** paint font
    /// (`"Monospace 12"`), so Pango's per-glyph advances line up with
    /// [`CommandLineLayout`]'s fixed column pitch and column *n* holds
    /// character *n* and nothing else. This module's paint font is the
    /// proportional `Segoe UI`, and the pitch is `measure_text("0")`
    /// (see `win::text::DWrite::new`): a space advances ~3.5 DIP against
    /// a ~7.3 DIP column, so in `"  x"` DirectWrite paints the `x`
    /// *inside column 1* — the exact pixel a transplanted twin reads as
    /// its unselected control. That is a font-metric coincidence, not
    /// the behaviour under test, so the colour probes here run against
    /// an **all-spaces** string (ink-free on any face, proportional or
    /// not), and the "text still reaches the surface" half asserts over
    /// the whole bar with no column arithmetic at all.
    #[test]
    fn selection_paints_highlight_behind_text() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        assert!(char_width > 1.0, "char_width should be several px");
        let theme = Theme {
            command_line_bg: Color::rgb(255, 255, 255),
            command_line_fg: Color::rgb(0, 0, 0),
            selection: Color::rgb(0, 0, 255),
            selection_alpha: 1.0,
            ..Theme::default()
        };
        let rect = Rect::new(0.0, 0.0, W, H);
        let row = (rect.height / 2.0) as u32;

        // Part 1 — highlight geometry. Three spaces: selecting byte 0..1
        // highlights column 0, and column 1 is an unselected control
        // pixel that no glyph can ever occupy (see this test's doc).
        let cmd = sample("   ", None, false);
        let layout = win_command_line_layout(&cmd, rect, char_width);
        let sel_rect = layout
            .selection_bounds((0, 1))
            .expect("non-empty selection should produce a rect");
        let unsel_rect = layout.char_bounds(1);

        surface
            .paint(|target| {
                draw_command_line_selection(
                    target,
                    &dwrite,
                    rect,
                    &cmd,
                    &theme,
                    char_width,
                    Some((0, 1)),
                );
            })
            .expect("paint command line selection");

        // Both probes come from the shared layout's own column rects, so
        // this never hardcodes a coordinate.
        let sample_px = surface.pixel_at((sel_rect.x + sel_rect.width / 2.0) as u32, row);
        assert_eq!(
            (sample_px.r, sample_px.g, sample_px.b),
            (0, 0, 255),
            "selected space column should paint the highlight colour",
        );

        let unsel_sample = surface.pixel_at((unsel_rect.x + unsel_rect.width / 2.0) as u32, row);
        assert_eq!(
            (unsel_sample.r, unsel_sample.g, unsel_sample.b),
            (255, 255, 255),
            "unselected column should not paint the highlight",
        );

        // Part 2 — the "behind text" half of this test's name: with real
        // text and the whole string selected, the bar must carry *both*
        // the highlight fill and glyph ink on top of it. Whole-bar scan,
        // so it holds for any font's advances.
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let cmd = sample(":wq", None, false);
        surface
            .paint(|target| {
                draw_command_line_selection(
                    target,
                    &dwrite,
                    rect,
                    &cmd,
                    &theme,
                    char_width,
                    Some((0, cmd.text.len())),
                );
            })
            .expect("paint command line selection over text");

        let (mut saw_highlight, mut saw_ink) = (false, false);
        for x in 0..W as u32 {
            for y in 0..H as u32 {
                match surface.pixel_at(x, y) {
                    px if (px.r, px.g, px.b) == (0, 0, 255) => saw_highlight = true,
                    px if (px.r, px.g, px.b) != (255, 255, 255) => saw_ink = true,
                    _ => {}
                }
            }
        }
        assert!(
            saw_highlight,
            "a fully-selected command line should paint the highlight colour somewhere",
        );
        assert!(
            saw_ink,
            "glyph ink should still paint on top of the selection highlight",
        );
    }

    #[test]
    fn zero_width_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let cmd = sample(":wq", Some(1), false);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        // Known non-theme background so "did anything paint here?" is
        // answerable per pixel, rather than relying on the DIB's
        // uninitialised-memory contents (mirrors the macOS twin's
        // `zero_width_rect_is_a_no_op`).
        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_command_line(target, &dwrite, rect, &cmd, &theme, char_width);
            })
            .expect("paint command line");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width command line should paint nothing at all",
        );
    }
}
