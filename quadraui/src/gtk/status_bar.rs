//! GTK rasteriser for [`crate::StatusBar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::status_bar::native_surface_paint::paint`] (#860,
//! `NativeSurface` Phase 2d slice 3/9) — see that fn's doc for the named
//! divergences (bold-aware measurement: GTK/Win measured a segment's own
//! `bold` weight, macOS ignored it; GTK's missing zero-size guard, now
//! applying the already-fixed quadraui#791 shape uniformly) found while
//! unifying `gtk::draw_status_bar`, `macos::status_bar::draw_status_bar`
//! and `win::status_bar::draw_status_bar` into one implementation. This
//! module now only carries the deprecated [`draw_status_bar`]
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter (#1072 — consolidated from this module's own private
//! `RawGtkStatusBarSurface`). `MIN_GAP_PX` stays put — it's still
//! `GtkBackend::status_bar_layout`'s own no-paint measurer constant,
//! untouched by this migration.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::status_bar::{StatusBar, StatusBarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// 16-pixel minimum gap between left and right segment groups, matching
/// the existing vimcode GTK behaviour. Still used by
/// `GtkBackend::status_bar_layout`'s own no-paint measurer — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub const MIN_GAP_PX: f32 = 16.0;

/// Deprecated free-function shim (#860, CLAUDE.md rule 8): reproduces
/// the pre-#860 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_status_bar` reference rather than going
/// through [`crate::Backend::draw_status_bar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_status_bar` instead — this free function is a compatibility shim over the shared #860 implementation"
)]
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

    /// `status_bar_layout` is documented **LOCAL** (issue #505):
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
    /// directly rather than the deprecated [`draw_status_bar`] shim, so
    /// this test doesn't trip the `-D warnings`-denied `deprecated` lint
    /// (CLAUDE.md rule 3; mirrors `gtk::panel`'s identical test-migration
    /// note).
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

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
