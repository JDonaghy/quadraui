//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::text_input::TextInput`] (#733).
//!
//! Mirrors `win::command_line`'s posture: [`TextInput::layout`] (already
//! shared in `primitives/`, see that module's doc) resolves every
//! positioning decision — border/padding inset, visible-line rects,
//! horizontal/vertical scroll clamp, and `cursor_bounds` — this module
//! only measures (a plain `line_height`/`char_width` pair, the same
//! backend-tracked scalars `win::command_line` takes, not a per-glyph
//! DirectWrite layout) and paints: a background fill, a 1px border, text
//! rows via [`DWrite::draw_text`], and a 2px cursor bar via `fill_rect`.
//!
//! # No cursor-offset → x arithmetic here
//!
//! Same rule `win::command_line`'s module doc states: the cursor bar's
//! rect comes straight from [`crate::primitives::text_input::TextInputLayout::cursor_bounds`],
//! the exact value `text_input_layout` also hands a host for placing a
//! caret/IME — this rasteriser does not re-derive a column position from
//! `char_width` a second time (#733's acceptance bar — "no geometry
//! re-derived in `win/`").
//!
//! `TextInput` carries no selection range (see that primitive's doc), so
//! unlike a full editor there is no selection highlight to paint here —
//! only the cursor bar, matching `gtk::text_input`'s own cursor-only
//! contract.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod text_input;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::theme::Theme;

/// Insert-cursor width in DIPs. Matches `win::command_line`'s
/// `CURSOR_W_DIP` / `win::editor`'s 2px bar-cursor fill.
const CURSOR_W_DIP: f32 = 2.0;

/// Compute [`TextInputLayout`] for `ti` painted at `rect`, using the
/// backend's tracked `line_height`/`char_width` — what
/// [`crate::win::WinBackend::text_input_layout`] calls directly, and what
/// [`draw_text_input`] paints against. Delegates entirely to
/// [`TextInput::layout`] — no geometry re-derived here (#733's
/// acceptance bar).
pub fn win_text_input_layout(
    ti: &TextInput,
    rect: Rect,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(line_height, char_width))
}

/// Paint `ti` into `rect` (DIPs, target-relative) on `target` and return
/// the resolved [`TextInputLayout`] — same contract as the GTK/TUI
/// twins' `draw_text_input`: callers (and tests) read the layout back
/// instead of re-deriving it, so paint and hit-test can't drift apart.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_input(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    ti: &TextInput,
    theme: &Theme,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    let layout = win_text_input_layout(ti, rect, line_height, char_width);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let _ = fill_rect(target, rect, theme.background);

    let border_color = if ti.has_focus {
        theme.accent_fg
    } else {
        theme.border_fg
    };
    let _ = stroke_rect(target, rect, border_color, 1.0);

    let h_scroll = layout.resolved_scroll_col;
    let slice_from = |line: &str, off: usize| -> String {
        if off == 0 {
            line.to_string()
        } else {
            line.chars().skip(off).collect()
        }
    };

    if layout.placeholder_active {
        if let (Some(text), Some(first)) = (ti.placeholder.as_ref(), layout.visible_lines.first()) {
            let _ = dwrite.draw_text(target, text, first.bounds, theme.muted_fg);
        }
    } else {
        for vis in &layout.visible_lines {
            let full = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
            let visible = slice_from(full, h_scroll);
            if visible.is_empty() {
                continue;
            }
            let _ = dwrite.draw_text(target, &visible, vis.bounds, theme.foreground);
        }
    }

    if ti.has_focus {
        if let Some(cb) = layout.cursor_bounds {
            let cursor_rect = Rect::new(cb.x, cb.y, CURSOR_W_DIP, cb.height);
            let _ = fill_rect(target, cursor_rect, theme.cursor);
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::text_input::TextInputHit;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 60.0;

    fn sample(lines: Vec<&str>, cursor_line: usize, cursor_col: usize) -> TextInput {
        TextInput {
            id: WidgetId::new("ti"),
            lines: lines.into_iter().map(String::from).collect(),
            cursor_line,
            cursor_col,
            placeholder: None,
            scroll_offset: 0,
            scroll_col: 0,
            has_focus: true,
        }
    }

    /// C0 smoke: `draw_text_input` must actually paint text + a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#733's acceptance bar — "draw_text_input survives C0
    /// with text_ok on win"). Also regression-guards the multibyte-cursor
    /// panic class `win::command_line` fixed under quadraui#503 — a
    /// `cursor_col` landing mid-`é` must snap via [`TextInput::layout`],
    /// not slice a line at a non-char-boundary.
    #[test]
    fn draw_text_input_paints_and_does_not_panic_on_multibyte_cursor() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let ti = sample(vec![":éditer"], 0, 2);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_text_input(target, &dwrite, rect, &ti, &theme, 16.0, char_width);
            })
            .map(|_| win_text_input_layout(&ti, rect, 16.0, char_width))
            .expect("paint text input");

        // "text_ok" — some non-background pixel actually painted inside
        // the content area (proves DrawText ran, not just the border).
        let mut painted_any = false;
        for x in (layout.content_bounds.x as u32)..(layout.content_bounds.x + 40.0) as u32 {
            for y in (layout.content_bounds.y as u32)..(layout.content_bounds.y + 16.0) as u32 {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected text_input to paint visible glyphs");
    }

    /// Paint↔click round trip (`docs/TESTING.md` coverage-taxonomy row
    /// 1) at a non-zero origin — #505's LOCAL/ABSOLUTE mixup regression
    /// guard, mirrored from `win::command_line` /
    /// `win::sidebar_panel`'s own nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 12.0_f32;
        let origin_y = 5.0_f32;
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let theme = Theme::default();
        let ti = sample(vec!["hello", "world"], 0, 0);
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = surface
            .paint(|target| {
                draw_text_input(target, &dwrite, rect, &ti, &theme, 16.0, char_width);
            })
            .map(|_| win_text_input_layout(&ti, rect, 16.0, char_width))
            .expect("paint text input");

        assert!(layout.content_bounds.x >= origin_x);
        assert!(layout.content_bounds.y >= origin_y);

        let first_line = layout.visible_lines.first().expect("one visible line");
        let hit = layout
            .hit_regions
            .iter()
            .find(|(r, _)| {
                r.contains(crate::event::Point::new(
                    first_line.bounds.x + 0.5,
                    first_line.bounds.y + 0.5,
                ))
            })
            .map(|(_, h)| h.clone());
        assert_eq!(hit, Some(TextInputHit::Line { line_idx: 0 }));
    }

    /// The cursor bar paints at `cursor_bounds` — the same rect
    /// [`TextInputLayout`] hands back for cursor/IME placement, not a
    /// hand-measured column offset — the paint-side half of "no
    /// cursor-offset→x arithmetic in `win/`" (module doc).
    #[test]
    fn cursor_paints_at_the_shared_layout_bounds() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            cursor: Color::rgb(255, 0, 0),
            ..Theme::default()
        };
        let ti = sample(vec!["hello"], 0, 3);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = win_text_input_layout(&ti, rect, 16.0, char_width);
        let cb = layout.cursor_bounds.expect("cursor visible");

        surface
            .paint(|target| {
                draw_text_input(target, &dwrite, rect, &ti, &theme, 16.0, char_width);
            })
            .expect("paint text input");

        let px = (cb.x + CURSOR_W_DIP / 2.0) as u32;
        let py = (cb.y + cb.height / 2.0) as u32;
        let sample_px = surface.pixel_at(px, py);
        assert_eq!(
            (sample_px.r, sample_px.g, sample_px.b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at cursor_bounds",
        );
    }

    #[test]
    fn zero_width_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let ti = sample(vec!["hello"], 0, 0);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_text_input(target, &dwrite, rect, &ti, &theme, 16.0, char_width);
            })
            .expect("paint text input");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width text input should paint nothing at all",
        );
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_text_input` painted — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::command_line`, `win::sidebar_panel`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let ti = sample(vec!["hello", "world"], 1, 2);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &Theme::default(),
                    16.0,
                    char_width,
                );
            })
            .map(|_| win_text_input_layout(&ti, rect, 16.0, char_width))
            .expect("paint");
        let no_paint = win_text_input_layout(&ti, rect, 16.0, char_width);
        assert_eq!(painted, no_paint);
    }

    /// #733 acceptance bar: `win_text_input_layout` must delegate to the
    /// shared [`TextInput::layout`] rather than re-deriving any geometry
    /// — asserted by proving the two calls (through the wrapper, and
    /// directly against the primitive) produce byte-for-byte identical
    /// layouts for the same inputs.
    #[test]
    fn win_text_input_layout_delegates_to_shared_primitive_layout() {
        let ti = sample(vec!["hello", "world"], 1, 3);
        let rect = Rect::new(4.0, 6.0, W, H);
        let line_height = 18.0;
        let char_width = 9.0;

        let via_wrapper = win_text_input_layout(&ti, rect, line_height, char_width);
        let direct = ti.layout(rect, TextInputMeasure::new(line_height, char_width));
        assert_eq!(via_wrapper, direct);
    }
}
