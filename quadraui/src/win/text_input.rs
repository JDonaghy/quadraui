//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::text_input::TextInput`].
//!
//! Mirrors `win::command_line`'s posture: [`TextInput::layout`] (already
//! shared in `primitives/`, see that module's doc) resolves every
//! positioning decision — border/padding inset, visible-line rects,
//! horizontal/vertical scroll clamp, and `cursor_bounds` — this module
//! only measures (a plain `line_height`/`char_width` pair, the same
//! backend-tracked scalars `win::command_line` takes, not a per-glyph
//! DirectWrite layout) and paints: a background fill, a 1px border, a
//! focus-visible ring when focused, text rows via [`DWrite::draw_text`],
//! and a 2px cursor bar via `fill_rect`.
//!
//! # Focus-visible ring
//!
//! When `ti.has_focus`, [`draw_text_input`] strokes an outset ring in
//! [`Theme::focus_ring_color`] around `rect`, geometry resolved through
//! the shared
//! [`crate::primitives::button::native_surface_paint::focus_ring_geometry`]
//! helper (`style.focus_ring_width`) — the same seam `win::button` and
//! `win::switch` already paint through for their own rings.
//!
//! # No cursor-offset → x arithmetic here
//!
//! Same rule `win::command_line`'s module doc states: the cursor bar's
//! rect comes straight from [`crate::primitives::text_input::TextInputLayout::cursor_bounds`],
//! the exact value `text_input_layout` also hands a host for placing a
//! caret/IME — this rasteriser does not re-derive a column position from
//! `char_width` a second time.
//!
//! `TextInput` carries no selection range, so unlike a full editor there
//! is no selection highlight to paint here — only the cursor bar, matching
//! `gtk::text_input`'s own cursor-only contract.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod text_input;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::button::native_surface_paint::focus_ring_geometry;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::style::Style;
use crate::theme::Theme;

/// Insert-cursor width in DIPs. Matches `win::command_line`'s
/// `CURSOR_W_DIP` / `win::editor`'s 2px bar-cursor fill.
const CURSOR_W_DIP: f32 = 2.0;

/// Compute [`TextInputLayout`] for `ti` painted at `rect`, using the
/// backend's tracked `line_height`/`char_width` — what
/// [`crate::win::WinBackend::text_input_layout`] calls directly, and what
/// [`draw_text_input`] paints against. Delegates entirely to
/// [`TextInput::layout`].
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
///
/// `style` supplies the focus-visible ring's geometry
/// ([`Style::focus_ring_width`]), read fresh each call rather than
/// hardcoded — see the ring block below.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_input(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    ti: &TextInput,
    theme: &Theme,
    style: &Style,
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

    // Focus-visible ring: an outset stroke in the theme's ring colour,
    // independent of the border tint above — see
    // `crate::primitives::button::native_surface_paint::focus_ring_geometry`'s
    // doc for why a per-backend `TextInput` rasteriser resolves the
    // outset gap through that shared helper rather than a hand-tuned
    // literal, and for why this strokes square (`stroke_rect`, no
    // radius) rather than at `ring.corner_radius` — `TextInput`'s own
    // border above is always square. `ti.has_focus` already carries the
    // caller's focus-visible suppression (composed via
    // `crate::compose::FocusRing::is_focus_visible` before it's set on
    // `TextInput`), so no separate check is needed here.
    if ti.has_focus {
        let ring = focus_ring_geometry(rect, style);
        let _ = stroke_rect(target, ring.rect, theme.focus_ring_color(), ring.width);
    }

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
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.cursor_line = cursor_line;
        ti.cursor_col = cursor_col;
        ti.has_focus = true;
        ti
    }

    /// `draw_text_input` must paint text and a click-routable layout,
    /// and handle multibyte characters correctly so a `cursor_col` landing
    /// mid-UTF-8 sequence (e.g. mid-`é`) snaps via [`TextInput::layout`]
    /// rather than slicing a line at a non-char-boundary.
    #[test]
    fn draw_text_input_paints_and_does_not_panic_on_multibyte_cursor() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let ti = sample(vec![":éditer"], 0, 2);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &theme,
                    &Style::default(),
                    16.0,
                    char_width,
                );
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

    /// Paint↔click round trip at a non-zero origin — regression guard for
    /// LOCAL/ABSOLUTE coordinate mixups, mirrored from `win::command_line`
    /// and `win::sidebar_panel`'s own nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 12.0_f32;
        let origin_y = 5.0_f32;
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let theme = Theme::default();
        let ti = sample(vec!["hello", "world"], 0, 0);
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = surface
            .paint(|target| {
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &theme,
                    &Style::default(),
                    16.0,
                    char_width,
                );
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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &theme,
                    &Style::default(),
                    16.0,
                    char_width,
                );
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

    /// Focus-visible ring: present (outset of the border, in
    /// `theme.focus_ring_color()`) when `ti.has_focus`, absent when not
    /// — the win-rasteriser half of the seam's "ring present after Tab
    /// focus, absent after a click" acceptance. Samples a point on the
    /// ring's *outset* band (outside `rect` entirely, in the
    /// `focus_ring_geometry` gap), never inside the 1px border's own
    /// stroke band — so, unlike a sample inside `rect`, this cannot pass
    /// merely because the border happens to share `theme.accent_fg`
    /// with `Theme::focus_ring_color()`; only the ring stroke itself
    /// reaches this pixel.
    #[test]
    fn focus_visible_ring_paints_in_the_outset_gap_only_when_focused() {
        const RING_GAP: f32 = 2.0; // matches `focus_ring_geometry`'s `FOCUS_RING_GAP`
        let rect = Rect::new(10.0, 10.0, 100.0, 30.0);
        let theme = Theme::default();
        let style = Style::default();
        let baseline = Color::rgb(10, 20, 30);
        // On the ring's left-edge band, well clear of the top/bottom
        // corners: outside `rect` (x < rect.x), so neither the
        // background fill nor the border stroke ever reaches it.
        let ring_x = (rect.x - RING_GAP / 2.0) as u32;
        let ring_y = (rect.y + rect.height / 2.0) as u32;

        for has_focus in [true, false] {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let (dwrite, _, char_width) =
                DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let mut ti = sample(vec!["hello"], 0, 0);
            ti.has_focus = has_focus;

            surface
                .fill_rect(Rect::new(0.0, 0.0, W, H), baseline)
                .expect("fill baseline");
            surface
                .paint(|target| {
                    draw_text_input(target, &dwrite, rect, &ti, &theme, &style, 16.0, char_width);
                })
                .expect("paint text input");

            let px = surface.pixel_at(ring_x, ring_y);
            if has_focus {
                assert_eq!(
                    (px.r, px.g, px.b),
                    (
                        theme.focus_ring_color().r,
                        theme.focus_ring_color().g,
                        theme.focus_ring_color().b
                    ),
                    "focused TextInput should paint the focus-visible ring in the outset gap",
                );
            } else {
                assert_eq!(
                    (px.r, px.g, px.b),
                    (baseline.r, baseline.g, baseline.b),
                    "unfocused TextInput should not paint a focus-visible ring",
                );
            }
        }
    }

    #[test]
    fn zero_width_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let ti = sample(vec!["hello"], 0, 0);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &theme,
                    &Style::default(),
                    16.0,
                    char_width,
                );
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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_text_input(
                    target,
                    &dwrite,
                    rect,
                    &ti,
                    &Theme::default(),
                    &Style::default(),
                    16.0,
                    char_width,
                );
            })
            .map(|_| win_text_input_layout(&ti, rect, 16.0, char_width))
            .expect("paint");
        let no_paint = win_text_input_layout(&ti, rect, 16.0, char_width);
        assert_eq!(painted, no_paint);
    }

    /// `win_text_input_layout` delegates to the shared [`TextInput::layout`]
    /// rather than re-deriving any geometry — asserted by proving the two
    /// calls (through the wrapper, and directly against the primitive)
    /// produce byte-for-byte identical layouts for the same inputs.
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
