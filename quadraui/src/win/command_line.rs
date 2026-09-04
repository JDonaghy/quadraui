//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::command_line::CommandLine`] (issue #725).
//!
//! Mirrors `win::status_bar`'s structure: [`CommandLine::layout`] (the
//! shared #705 [`CommandLineLayout`], see that primitive's module doc)
//! resolves every column position; this module only measures the current
//! font's monospace `char_width` (already tracked on `WinBackend`, the
//! same role as GTK's/macOS's `current_char_width`) and paints — a
//! background fill via `fill_rect` and text via [`DWrite::draw_text`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod command_line;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # No cursor-offset → x arithmetic here
//!
//! The GTK/macOS/TUI twins each re-derive the cursor's x-position by
//! measuring `text_util::safe_prefix(&cmd.text, offset)` against the
//! paint font — three private copies of the same column arithmetic
//! [`CommandLineLayout`] (#705) now exists specifically to centralise.
//! This rasteriser does not add a fourth: the cursor bar's rect comes
//! straight back from [`CommandLineLayout::char_bounds`], the exact value
//! `command_line_layout` also hands a host for hit-testing, so paint and
//! layout share one source of truth and can't drift apart (#725 scope
//! note; `PRIMITIVE_RULES.md`'s primitive-first rule, #713).

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
use crate::theme::Theme;

/// Insert-cursor width in DIPs. Matches the GTK/macOS rasterisers' 2px
/// bar (`gtk::command_line`, `macos::command_line::CURSOR_W`).
const CURSOR_W_DIP: f32 = 2.0;

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
    dwrite: &DWrite,
    rect: Rect,
    cmd: &CommandLine,
    theme: &Theme,
    char_width: f32,
) -> CommandLineLayout {
    let layout = win_command_line_layout(cmd, rect, char_width);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let _ = fill_rect(target, rect, theme.command_line_bg);

    if cmd.text.is_empty() {
        return layout;
    }

    // `layout.text_origin_x` already carries `rect.x` and any
    // right-align shift (issue #505 convention: absolute, not
    // rect-local) — paint the text starting there, clipped to the
    // remainder of the bar so an over-long or right-aligned string
    // can't paint past `rect`'s edges.
    let text_rect = Rect::new(
        layout.text_origin_x,
        rect.y,
        (rect.x + rect.width - layout.text_origin_x).max(0.0),
        rect.height,
    );
    let _ = dwrite.draw_text(target, &cmd.text, text_rect, theme.command_line_fg);

    if let Some(offset) = cmd.cursor_offset {
        let col_rect = layout.char_bounds(offset);
        let cursor_rect = Rect::new(col_rect.x, col_rect.y, CURSOR_W_DIP, col_rect.height);
        let _ = fill_rect(target, cursor_rect, theme.cursor);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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

        let px = (cursor_col_rect.x + CURSOR_W_DIP / 2.0) as u32;
        let py = (cursor_col_rect.y + cursor_col_rect.height / 2.0) as u32;
        let sample_px = surface.pixel_at(px, py);
        assert_eq!(
            (sample_px.r, sample_px.g, sample_px.b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at char_bounds(1)'s column",
        );
    }

    #[test]
    fn zero_width_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
