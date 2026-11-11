//! GTK rasteriser for [`crate::TextInput`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::text_input::paint`] — the same painter
//! `MacBackend::draw_text_input` uses — via the shared
//! [`super::surface::CairoSurface`] adapter, which lets this module's
//! free [`draw_text_input`] (kept for its existing public signature;
//! `GtkBackend::draw_text_input` itself calls the shared painter
//! directly with `self` as the surface, same shape as
//! `Self::draw_number_input`/`Self::draw_combo_box`) reach that painter
//! without a live `GtkBackend`. See that function's doc for the
//! geometry (border, placeholder/text lines, focus-visible ring,
//! 2-unit cursor bar) — nothing is re-derived here.

use gtk4::cairo::Context;
use gtk4::pango;

use super::surface::CairoSurface;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::style::Style;
use crate::theme::Theme;

pub fn gtk_text_input_layout(
    ti: &TextInput,
    rect: crate::event::Rect,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(line_height, char_width))
}

#[allow(clippy::too_many_arguments)]
pub fn draw_text_input(
    cr: &Context,
    layout: &pango::Layout,
    rect: crate::event::Rect,
    ti: &TextInput,
    theme: &Theme,
    style: &Style,
    line_height: f64,
    char_width: f64,
) -> TextInputLayout {
    let li = gtk_text_input_layout(ti, rect, line_height as f32, char_width as f32);
    // `translucent_fill: false` matches this painter's own colours
    // (always opaque — `TextInput` carries no translucent fills), same
    // as `gtk::surface`'s `Form`/`Split`/`SplitTree` call sites.
    let mut surface = CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: false,
    };
    crate::primitives::text_input::paint(ti, &li, &mut surface, theme, style);
    li
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::text_input::TextInputHit;
    use crate::types::WidgetId;

    /// `text_input_layout` is documented **ABSOLUTE**:
    /// `content_bounds` / hit regions must be shifted by `rect.x`/
    /// `rect.y`, not left at (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f32, y: f32) {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = vec!["hello".into()];
        ti.has_focus = true;
        let rect = crate::event::Rect::new(x, y, 40.0, 20.0);
        let layout = gtk_text_input_layout(&ti, rect, 10.0, 8.0);

        // Content sits inset by a fixed border + padding from `rect`'s
        // own origin (1px border + 1 char-width horizontal padding, 1px
        // vertical) — ABSOLUTE means that inset tracks `rect.x`/`rect.y`
        // exactly, not a fixed (0, 0)-relative offset.
        assert_eq!(layout.content_bounds.x, x + 1.0 + 8.0);
        assert_eq!(layout.content_bounds.y, y + 1.0);

        let p = Point::new(layout.content_bounds.x + 0.5, layout.content_bounds.y + 0.5);
        let hit = layout
            .hit_regions
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h.clone());
        assert_eq!(hit, Some(TextInputHit::Line { line_idx: 0 }));
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Tests that painting and clicking work correctly with non-zero origins.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::event::Rect;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 60;

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        // Cairo ARGB32 byte order on little-endian is BGRA.
        (data[off + 2], data[off + 1], data[off])
    }

    fn sample(ti: &TextInput, rect: Rect, style: &Style) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            // Baseline fill distinct from every colour `draw_text_input`
            // paints, so any pixel still showing it proves nothing was
            // painted there.
            cr.set_source_rgb(10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_text_input(
                &cr,
                &pango_layout,
                rect,
                ti,
                &Theme::default(),
                style,
                16.0,
                8.0,
            );
        }
        surface
    }

    /// Focus-visible ring: present (outset of the border, in
    /// `theme.focus_ring_color()`) when `ti.has_focus`, absent when not
    /// — the gtk-rasteriser half of the seam's "ring present after Tab
    /// focus, absent after a click" acceptance. Samples a point on the
    /// ring's *outset* band (outside `rect` entirely, in the
    /// `focus_ring_geometry` gap), so unlike a sample inside `rect` this
    /// cannot pass merely because the border happens to share
    /// `theme.accent_fg` with `Theme::focus_ring_color()`; only the ring
    /// stroke itself reaches this pixel.
    #[test]
    fn focus_visible_ring_paints_in_the_outset_gap_only_when_focused() {
        let rect = Rect::new(20.0, 15.0, 100.0, 30.0);
        let style = Style::default();
        let theme = Theme::default();

        // Centre of the ring's stroke band on the left edge, well clear
        // of the top/bottom corners: `focus_ring_geometry`'s outset gap
        // is 2px, so `rect.x - 2.0` sits on the ring's own outset rect
        // edge, inside the 2px-wide stroke centred on it.
        let ring_x = (rect.x - 2.0) as i32;
        let ring_y = (rect.y + rect.height / 2.0) as i32;

        for has_focus in [true, false] {
            let mut ti = TextInput::new(WidgetId::new("ti"));
            ti.lines = vec!["hello".into()];
            ti.has_focus = has_focus;

            let mut surface = sample(&ti, rect, &style);
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");
            let (r, g, b) = pixel(&data, stride, ring_x, ring_y);

            if has_focus {
                assert_eq!(
                    (r, g, b),
                    (
                        theme.focus_ring_color().r,
                        theme.focus_ring_color().g,
                        theme.focus_ring_color().b
                    ),
                    "focused TextInput should paint the focus-visible ring in the outset gap",
                );
            } else {
                assert_eq!(
                    (r, g, b),
                    (10, 20, 30),
                    "unfocused TextInput should not paint a focus-visible ring",
                );
            }
        }
    }

    /// Caret width is 2.0 surface units — the shared painter's
    /// `CURSOR_W` (see `crate::primitives::text_input::
    /// native_surface_paint`'s own doc). Samples the pixel column one
    /// unit in from `cursor_bounds.x`: at width 2.0 that column sits
    /// fully inside the filled cursor rect `[cb.x, cb.x + 2.0)`, so it
    /// paints pure `theme.cursor`; at width 1.5 the same column is only
    /// half-covered, so cairo's edge antialiasing blends it with
    /// whatever is underneath instead.
    #[test]
    fn caret_is_two_units_wide() {
        let rect = Rect::new(20.0, 15.0, 100.0, 30.0);
        let style = Style::default();
        let theme = Theme::default();

        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = vec!["hello".into()];
        ti.has_focus = true;
        ti.cursor_col = 2;

        let layout = gtk_text_input_layout(&ti, rect, 16.0, 8.0);
        let cb = layout.cursor_bounds.expect("cursor visible");

        let mut surface = sample(&ti, rect, &style);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let sample_x = (cb.x + 1.0) as i32;
        let sample_y = (cb.y + cb.height / 2.0) as i32;
        let (r, g, b) = pixel(&data, stride, sample_x, sample_y);

        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "expected the caret to fully cover the pixel column 1 unit in from \
             cursor_bounds.x, proving it is at least 2 units wide",
        );
    }

    /// A line long enough that its painted tail runs well past
    /// `rect`'s right edge must still stop at `layout.bounds` —
    /// `draw_text_input` clips text painting to `content_bounds`, so
    /// nothing lands outside the widget regardless of how long the
    /// unclamped Pango run is. Samples a pixel clear of `rect` on the
    /// right (still inside the `W`x`H` canvas) for the baseline fill,
    /// which only a clip failure would overwrite.
    #[test]
    fn long_line_does_not_paint_past_the_widget_bounds() {
        let rect = Rect::new(20.0, 15.0, 100.0, 30.0);
        let style = Style::default();
        let line_height = 16.0;
        let char_width = 8.0;

        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = vec!["x".repeat(500)];
        ti.has_focus = true;
        ti.cursor_col = 0;

        let layout = gtk_text_input_layout(&ti, rect, line_height as f32, char_width as f32);

        let mut surface = sample(&ti, rect, &style);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // Just right of `rect` (which ends at x = 120), well inside the
        // W=200 canvas. Scan the whole painted row's y-band rather than a
        // single y, since a row-top-aligned layout only puts glyph ink in
        // part of that band — a single fixed y can land below the ink and
        // pass even with the clip removed.
        let outside_x = (rect.x + rect.width + 20.0) as i32;
        let row_top = layout.content_bounds.y as i32;
        let row_bottom = (layout.content_bounds.y + line_height as f32) as i32;
        for y in row_top..row_bottom {
            let (r, g, b) = pixel(&data, stride, outside_x, y);
            assert_eq!(
                (r, g, b),
                (10, 20, 30),
                "a long unscrolled line must not paint past content_bounds / rect at y={y}",
            );
        }
    }
}
