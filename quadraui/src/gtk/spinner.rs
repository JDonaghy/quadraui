//! GTK rasteriser for [`crate::Spinner`].
//!
//! Layout-only queries go through [`gtk_spinner_layout`]; painting
//! goes through [`crate::primitives::spinner::native_surface_paint::paint`]
//! via a [`super::surface::CairoSurface`] — the glyph table and the
//! glyph+label text are shared with every other `native_surface_paint`
//! adopter, not duplicated here.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::spinner::{
    native_surface_paint, spinner_text, Spinner, SpinnerLayout, SpinnerMeasure,
};
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`Spinner`] without painting.
pub fn gtk_spinner_layout(
    spinner: &Spinner,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
) -> SpinnerLayout {
    let text = spinner_text(spinner);
    pango_layout.set_text(&text);
    pango_layout.set_attributes(None);
    let (pw, ph) = pango_layout.pixel_size();
    spinner.layout(
        x as f32,
        y as f32,
        SpinnerMeasure::new(pw.max(0) as f32, ph.max(0) as f32),
    )
}

/// Draw a [`Spinner`] onto `cr`. Returns the layout for host
/// hit-testing.
pub fn draw_spinner(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    spinner: &Spinner,
    theme: &Theme,
) -> SpinnerLayout {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        // Inert: `native_surface_paint::paint` only issues a text run for
        // `Spinner`, never a fill.
        translucent_fill: false,
    };
    native_surface_paint::paint(spinner, &mut surface, theme, x as f32, y as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::spinner::SpinnerHit;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_pango_layout() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    /// `spinner_layout` is documented **ABSOLUTE**:
    /// `bounds` must start at the spinner's own origin, not (0, 0) —
    /// the case that hides a LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let (_surface, pango_layout) = headless_pango_layout();
        let spinner = Spinner {
            id: WidgetId::new("spin"),
            label: "Indexing".into(),
            frame_idx: 0,
            accent: None,
        };
        let layout = gtk_spinner_layout(&spinner, &pango_layout, x, y);

        assert_eq!(layout.bounds.x as f64, x);
        assert_eq!(layout.bounds.y as f64, y);

        let cx = layout.bounds.x + 1.0;
        let cy = layout.bounds.y + 1.0;
        assert_eq!(
            layout.hit_test(cx, cy, &spinner.id),
            SpinnerHit::Body(spinner.id.clone())
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// This test verifies round-trip behavior with a non-zero origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }

    /// `draw_spinner` and `gtk_spinner_layout` must agree: the layout
    /// `draw_spinner` returns from actually painting has to match the
    /// layout `gtk_spinner_layout` reports without painting, at a
    /// non-zero origin. This is the no-paint twin contract
    /// `macos::spinner` documents for its own pair.
    #[test]
    fn draw_spinner_agrees_with_gtk_spinner_layout() {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = Theme::default();
        let spinner = Spinner {
            id: WidgetId::new("spin"),
            label: "Indexing".into(),
            frame_idx: 2,
            accent: None,
        };

        let measured = gtk_spinner_layout(&spinner, &pango_layout, 7.0, 13.0);
        let painted = draw_spinner(&cr, &pango_layout, 7.0, 13.0, &spinner, &theme);

        assert_eq!(measured, painted);
    }
}
