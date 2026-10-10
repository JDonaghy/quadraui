//! GTK rasteriser for [`crate::StatusBar`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::status_bar::native_surface_paint::paint`] — see that fn's doc
//! for implementation details. This module carries [`draw_status_bar`] over the
//! shared [`super::surface::CairoSurface`] adapter.
//!
//! `draw_status_bar` remains public and un-deprecated: [`super::surface::CairoSurface`]
//! is `pub(crate)`, so `kubeui-gtk/src/main.rs`'s `draw` closure (a bare `cr`/`layout`
//! with no `GtkBackend` in scope) has no other way to reach the shared paint.
//! `MIN_GAP_PX` stays put — it is still `GtkBackend::status_bar_layout`'s own
//! no-paint measurer constant.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::status_bar::{StatusBar, StatusBarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// The sanctioned entry point for a caller holding a bare `cr`/`layout`
/// with no [`crate::gtk::backend::GtkBackend`] in scope —
/// [`super::surface::CairoSurface`] is `pub(crate)`, so this free
/// function is how `kubeui-gtk/src/main.rs` (and any future such
/// caller) reaches the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`].
/// Every in-tree `Backend::draw_status_bar_interactive` call site goes
/// through [`crate::gtk::backend::GtkBackend`] instead, which owns its
/// own `CairoSurface`.
#[allow(clippy::too_many_arguments)]
pub fn draw_status_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    bar: &StatusBar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    crate::primitives::status_bar::native_surface_paint::paint(
        bar,
        &mut surface,
        theme,
        x as f32,
        y as f32,
        width as f32,
        line_height as f32,
        hovered_id,
        pressed_id,
    )
}

/// 16-pixel minimum gap between left and right segment groups, matching
/// the existing vimcode GTK behaviour. Still used by
/// `GtkBackend::status_bar_layout`'s own no-paint measurer — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub const MIN_GAP_PX: f32 = 16.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBarHit, StatusBarSegment};
    use crate::types::Color;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_cairo_and_pango() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    fn test_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("sb"),
            left_segments: vec![StatusBarSegment {
                text: "Ready".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("sb:action")),
            }],
            right_segments: vec![],
        }
    }

    /// `status_bar_layout` is documented **LOCAL**:
    /// `StatusBar::layout` (called by `paint` internally) only ever
    /// receives `width`/`line_height` — the `x`/`y` this function paints
    /// at are used purely to offset the drawing calls, never folded into
    /// the returned `StatusBarLayout`. Painting at a non-zero origin must
    /// therefore produce byte-identical segment bounds / hit regions to
    /// painting at the origin — the same "ignores origin" shape
    /// `data_table_layout`/`form_layout` already guard on GTK
    /// (`gtk::backend` tests), which `status_bar_layout` had no
    /// equivalent for.
    ///
    /// Exercises the shared paint through [`super::super::surface::CairoSurface`]
    /// directly rather than [`draw_status_bar`] — both reach the same
    /// `native_surface_paint::paint` body, so this is just the more
    /// direct of the two equally-valid paths (mirrors `gtk::panel`'s
    /// identical test shape).
    fn round_trip_at(x: f64, y: f64) {
        let (surface, pango_layout) = headless_cairo_and_pango();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let theme = Theme::default();
        let bar = test_bar();

        let mut raw = crate::gtk::surface::CairoSurface {
            cr: &cr,
            layout: Some(&pango_layout),
            translucent_fill: true,
        };
        let at_origin = crate::primitives::status_bar::native_surface_paint::paint(
            &bar, &mut raw, &theme, 0.0, 0.0, 100.0, 20.0, None, None,
        );
        let mut raw = crate::gtk::surface::CairoSurface {
            cr: &cr,
            layout: Some(&pango_layout),
            translucent_fill: true,
        };
        let shifted = crate::primitives::status_bar::native_surface_paint::paint(
            &bar, &mut raw, &theme, x as f32, y as f32, 100.0, 20.0, None, None,
        );

        assert_eq!(
            at_origin, shifted,
            "LOCAL status_bar_layout must not shift segment bounds by x/y"
        );

        let seg = shifted.visible_segments[0];
        let cx = seg.bounds.x + 1.0;
        let cy = seg.bounds.y + 1.0;
        assert_eq!(
            shifted.hit_test(cx, cy),
            StatusBarHit::Segment(bar.left_segments[0].action_id.clone().unwrap())
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
