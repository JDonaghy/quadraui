//! GTK rasteriser for [`crate::Switch`].
//!
//! Paints a pill-shaped track with a circular knob on the right when
//! `value` is `true`, on the left when `false`. The whole rect is one
//! hit target, same as [`crate::Switch::layout`].

use gtk4::cairo::Context;

use super::{rounded_rect_path, set_source};
use crate::primitives::switch::{Switch, SwitchLayout};
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`Switch`] without painting.
/// The control has no internal sub-regions, so `(x, y, w, h)` maps
/// straight through to [`Switch::layout`]'s bounds.
pub fn gtk_switch_layout(switch: &Switch, x: f64, y: f64, w: f64, h: f64) -> SwitchLayout {
    switch.layout(crate::event::Rect::new(
        x as f32, y as f32, w as f32, h as f32,
    ))
}

/// Draw a [`Switch`] at `(x, y, w, h)` onto `cr`. Returns the layout for
/// host click dispatch.
pub fn draw_switch(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    switch: &Switch,
    theme: &Theme,
) -> SwitchLayout {
    let layout = gtk_switch_layout(switch, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    let track_bg = if switch.disabled {
        theme.surface_bg
    } else if switch.value {
        theme.accent_bg
    } else {
        theme.surface_bg
    };
    let border = if switch.disabled {
        theme.muted_fg
    } else if switch.focused {
        theme.accent_fg
    } else {
        theme.border_fg
    };
    let knob_fg = if switch.disabled {
        theme.muted_fg
    } else {
        theme.foreground
    };

    let radius = (h / 2.0).max(0.0);
    set_source(cr, track_bg);
    rounded_rect_path(cr, x, y, w, h, radius);
    cr.fill().ok();

    set_source(cr, border);
    cr.set_line_width(1.0);
    rounded_rect_path(
        cr,
        x + 0.5,
        y + 0.5,
        (w - 1.0).max(0.0),
        (h - 1.0).max(0.0),
        radius,
    );
    cr.stroke().ok();

    let knob_r = (radius - 2.0).max(0.0);
    let knob_cy = y + h / 2.0;
    let knob_cx = if switch.value {
        x + w - radius
    } else {
        x + radius
    };
    set_source(cr, knob_fg);
    cr.arc(knob_cx, knob_cy, knob_r, 0.0, std::f64::consts::TAU);
    cr.fill().ok();

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::switch::SwitchHit;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_surface() -> ImageSurface {
        ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface")
    }

    fn mk_switch(value: bool) -> Switch {
        let mut s = Switch::new(WidgetId::new("settings:wrap"));
        s.value = value;
        s
    }

    /// `switch_layout` is documented **ABSOLUTE**: `bounds` must start
    /// at the switch's own origin, not (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let switch = mk_switch(false);
        let layout = gtk_switch_layout(&switch, x, y, 24.0, 12.0);

        assert_eq!(layout.bounds.x as f64, x);
        assert_eq!(layout.bounds.y as f64, y);

        let hit = layout.hit_test(crate::event::Point::new(x as f32 + 1.0, y as f32 + 1.0));
        assert_eq!(hit, SwitchHit::Body);
        let outside = layout.hit_test(crate::event::Point::new(x as f32 + 500.0, y as f32));
        assert_eq!(outside, SwitchHit::Outside);

        let surface = headless_surface();
        let cr = CairoContext::new(&surface).expect("Context::new");
        draw_switch(&cr, x, y, 24.0, 12.0, &switch, &Theme::default());
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
    fn on_state_still_paints_without_panicking() {
        let surface = headless_surface();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let switch = mk_switch(true);
        draw_switch(&cr, 0.0, 0.0, 24.0, 12.0, &switch, &Theme::default());
    }

    #[test]
    fn disabled_switch_still_paints_without_panicking() {
        let surface = headless_surface();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let mut switch = mk_switch(false);
        switch.disabled = true;
        draw_switch(&cr, 0.0, 0.0, 24.0, 12.0, &switch, &Theme::default());
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let surface = headless_surface();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let switch = mk_switch(false);
        let layout = draw_switch(&cr, 0.0, 0.0, 0.0, 0.0, &switch, &Theme::default());
        assert_eq!(layout.bounds.width, 0.0);
    }
}
