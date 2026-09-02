//! GTK rasteriser for [`crate::Spinner`].
//!
//! Paints a Unicode animation glyph + label using Pango text layout.
//! Same braille frame table as TUI for visual consistency across
//! backends.

use gtk4::cairo::Context;
use gtk4::pango;

use super::set_source;
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
use crate::theme::Theme;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Compute the GTK pixel-unit layout for a [`Spinner`] without painting.
pub fn gtk_spinner_layout(
    spinner: &Spinner,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
) -> SpinnerLayout {
    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    let text = if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    };
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
    let layout = gtk_spinner_layout(spinner, pango_layout, x, y);

    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    let text = if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    };
    pango_layout.set_text(&text);
    pango_layout.set_attributes(None);

    let fg = spinner.accent.unwrap_or(theme.foreground);
    set_source(cr, fg);
    cr.move_to(x, y);
    super::painted_text::show_layout(cr, pango_layout);

    layout
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

    /// `spinner_layout` is documented **ABSOLUTE** (issue #505):
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

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
