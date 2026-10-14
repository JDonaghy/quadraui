//! GTK rasteriser for [`crate::Button`].
//!
//! Paints a filled/outlined chrome rect per [`ButtonVariant`] and
//! disabled/focused state, with the label centred via Pango text
//! layout. The whole rect is one hit target, same as
//! [`crate::Button::layout`].

use gtk4::cairo::Context;
use gtk4::pango;

use super::set_source;
use crate::primitives::button::{Button, ButtonLayout, ButtonVariant};
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`Button`] without painting.
/// The control has no internal sub-regions, so `(x, y, w, h)` maps
/// straight through to [`Button::layout`]'s bounds.
pub fn gtk_button_layout(button: &Button, x: f64, y: f64, w: f64, h: f64) -> ButtonLayout {
    button.layout(crate::event::Rect::new(
        x as f32, y as f32, w as f32, h as f32,
    ))
}

/// Draw a [`Button`] at `(x, y, w, h)` onto `cr`. Returns the layout for
/// host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_button(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    button: &Button,
    theme: &Theme,
) -> ButtonLayout {
    let layout = gtk_button_layout(button, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    let bg = if button.disabled {
        theme.surface_bg
    } else {
        match button.variant {
            ButtonVariant::Primary => theme.accent_bg,
            ButtonVariant::Secondary => theme.surface_bg,
            ButtonVariant::Icon => theme.background,
        }
    };
    let border = if button.disabled {
        theme.muted_fg
    } else if button.focused {
        theme.accent_fg
    } else {
        theme.border_fg
    };
    let fg = if button.disabled {
        theme.muted_fg
    } else {
        theme.foreground
    };

    set_source(cr, bg);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    if !matches!(button.variant, ButtonVariant::Icon) {
        set_source(cr, border);
        cr.set_line_width(1.0);
        cr.rectangle(x + 0.5, y + 0.5, (w - 1.0).max(0.0), (h - 1.0).max(0.0));
        cr.stroke().ok();
    }

    pango_layout.set_text(&button.label);
    pango_layout.set_attributes(None);
    let (pw, ph) = pango_layout.pixel_size();
    let text_x = x + ((w - pw as f64) / 2.0).max(0.0);
    let text_y = y + ((h - ph as f64) / 2.0).max(0.0);
    set_source(cr, fg);
    cr.move_to(text_x, text_y);
    super::painted_text::show_layout(cr, pango_layout);

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::button::ButtonHit;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_pango_layout() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    fn mk_button() -> Button {
        Button::new(WidgetId::new("dialog:ok"), "OK")
    }

    /// `button_layout` is documented **ABSOLUTE**: `bounds` must start
    /// at the button's own origin, not (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let (_surface, pango_layout) = headless_pango_layout();
        let button = mk_button();
        let layout = gtk_button_layout(&button, x, y, 60.0, 20.0);

        assert_eq!(layout.bounds.x as f64, x);
        assert_eq!(layout.bounds.y as f64, y);

        let hit = layout.hit_test(crate::event::Point::new(x as f32 + 1.0, y as f32 + 1.0));
        assert_eq!(hit, ButtonHit::Body);
        let outside = layout.hit_test(crate::event::Point::new(x as f32 + 500.0, y as f32));
        assert_eq!(outside, ButtonHit::Outside);

        let (_surface2, pango_layout2) = headless_pango_layout();
        let cr = CairoContext::new(&_surface2).expect("Context::new");
        draw_button(
            &cr,
            &pango_layout2,
            x,
            y,
            60.0,
            20.0,
            &button,
            &Theme::default(),
        );
        let _ = pango_layout;
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }

    #[test]
    fn disabled_button_still_paints_without_panicking() {
        let (_surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&_surface).expect("Context::new");
        let mut button = mk_button();
        button.disabled = true;
        draw_button(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            60.0,
            20.0,
            &button,
            &Theme::default(),
        );
    }

    #[test]
    fn icon_variant_paints_without_border() {
        let (_surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&_surface).expect("Context::new");
        let mut button = mk_button();
        button.variant = ButtonVariant::Icon;
        draw_button(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            20.0,
            20.0,
            &button,
            &Theme::default(),
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let (_surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&_surface).expect("Context::new");
        let button = mk_button();
        let layout = draw_button(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            0.0,
            0.0,
            &button,
            &Theme::default(),
        );
        assert_eq!(layout.bounds.width, 0.0);
    }
}
