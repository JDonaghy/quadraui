//! Layout + the shared painter's entry point for
//! [`crate::primitives::text_input::TextInput`] on Windows.
//!
//! Mirrors `win::command_line`'s posture: [`TextInput::layout`] (already
//! shared in `primitives/`, see that module's doc) resolves every
//! positioning decision — border/padding inset, visible-line rects,
//! horizontal/vertical scroll clamp, and `cursor_bounds`. Painting itself
//! is delegated to the shared
//! [`crate::primitives::text_input::native_surface_paint::paint`]
//! function — see that fn's doc for details. The `Backend`-trait entry
//! point [`super::backend::WinBackend::draw_text_input`] calls the shared
//! `paint` directly through `self`'s own `PaintSurface` impl, mirroring
//! `MacBackend::draw_text_input`'s shape. This module carries only
//! [`win_text_input_layout`], which provides backend-specific geometry
//! without performing painting.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod text_input;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use crate::event::Rect;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};

/// Compute [`TextInputLayout`] for `ti` painted at `rect`, using the
/// backend's tracked `line_height`/`char_width` — what
/// [`crate::win::WinBackend::text_input_layout`] and
/// [`crate::win::WinBackend::draw_text_input`] both call. Delegates
/// entirely to [`TextInput::layout`].
pub fn win_text_input_layout(
    ti: &TextInput,
    rect: Rect,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(line_height, char_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::text_input::TextInputHit;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: f32 = 200.0;
    const H: f32 = 60.0;
    /// Matches `primitives::text_input::native_surface_paint::CURSOR_W`.
    const CURSOR_W: f32 = 2.0;

    fn sample(lines: Vec<&str>, cursor_line: usize, cursor_col: usize) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.cursor_line = cursor_line;
        ti.cursor_col = cursor_col;
        ti.has_focus = true;
        ti
    }

    /// Paint `ti` via the shared
    /// [`crate::primitives::text_input::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter `WinBackend::draw_text_input`
    /// uses, exercised here directly — and return the resolved layout.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        ti: &TextInput,
        theme: &Theme,
        style: &Style,
        line_height: f32,
        char_width: f32,
    ) -> TextInputLayout {
        let layout = win_text_input_layout(ti, rect, line_height, char_width);
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::text_input::paint(ti, &layout, &mut raw, theme, style);
            })
            .expect("paint text input");
        layout
    }

    /// Text paints, and multibyte characters do not panic: a `cursor_col`
    /// landing mid-UTF-8 sequence (e.g. mid-`é`) snaps via
    /// [`TextInput::layout`] rather than slicing a line at a
    /// non-char-boundary.
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

        let layout = paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &theme,
            &Style::default(),
            16.0,
            char_width,
        );

        // "text_ok" — some non-background pixel actually painted inside
        // the content area (proves the glyph run ran, not just the
        // border).
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

        let layout = paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &theme,
            &Style::default(),
            16.0,
            char_width,
        );

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
    /// hand-measured column offset.
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

        paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &theme,
            &Style::default(),
            16.0,
            char_width,
        );

        let px = (cb.x + CURSOR_W / 2.0) as u32;
        let py = (cb.y + cb.height / 2.0) as u32;
        let sample_px = surface.pixel_at(px, py);
        assert_eq!(
            (sample_px.r, sample_px.g, sample_px.b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "insert cursor should paint at cursor_bounds",
        );
    }

    /// Placeholder text paints when `ti.lines` is empty and
    /// `ti.placeholder` is set — the shared painter's
    /// `layout.placeholder_active` branch, exercised through the Win
    /// surface adapter rather than assumed from reading the primitive.
    #[test]
    fn placeholder_paints_when_lines_are_empty() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            muted_fg: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.placeholder = Some("search…".to_string());
        ti.has_focus = false;
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &theme,
            &Style::default(),
            16.0,
            char_width,
        );
        assert!(
            layout.placeholder_active,
            "expected placeholder to be active"
        );

        let mut painted_any = false;
        for x in (layout.content_bounds.x as u32)..(layout.content_bounds.x + 40.0) as u32 {
            for y in (layout.content_bounds.y as u32)..(layout.content_bounds.y + 16.0) as u32 {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected placeholder glyphs to paint");
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
            paint(
                &surface, &dwrite, rect, &ti, &theme, &style, 16.0, char_width,
            );

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

        paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &theme,
            &Style::default(),
            16.0,
            char_width,
        );

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width text input should paint nothing at all",
        );
    }

    /// No-paint layout must agree byte-for-byte with what the shared
    /// painter paints against — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::command_line`, `win::sidebar_panel`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let ti = sample(vec!["hello", "world"], 1, 2);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = paint(
            &surface,
            &dwrite,
            rect,
            &ti,
            &Theme::default(),
            &Style::default(),
            16.0,
            char_width,
        );
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
