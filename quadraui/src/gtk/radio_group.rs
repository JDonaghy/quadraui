//! GTK rasteriser for [`crate::RadioGroup`].
//!
//! Stacks each [`crate::RadioOption`] one per row, painting a filled
//! dot for the selected option and an outlined circle for the rest,
//! followed by the label via Pango text layout. Each row is its own
//! hit target, mirroring
//! [`crate::primitives::radio_group::RadioGroupLayout::hit_test`]'s
//! per-option resolution.

use gtk4::cairo::Context;
use gtk4::pango;

use super::set_source;
use crate::primitives::radio_group::{
    RadioGroup, RadioGroupLayout, RadioOption, RadioOptionMeasure,
};
use crate::theme::Theme;

const DOT_COLUMN_WIDTH: f64 = 18.0;

/// Compute the GTK pixel-unit layout for a [`RadioGroup`] without
/// painting. Every option gets a row `row_height` tall — the same
/// constant-height convention [`crate::tui::radio_group`] uses rather
/// than measuring real text metrics, since a layout-only call has no
/// Pango context to measure against.
pub fn gtk_radio_group_layout(
    group: &RadioGroup,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    row_height: f64,
) -> RadioGroupLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    group.layout(bounds, |_option: &RadioOption| RadioOptionMeasure {
        width: bounds.width,
        height: row_height as f32,
    })
}

/// Draw a [`RadioGroup`] at `(x, y, w, h)` onto `cr`, one row per
/// option, measuring each row's height from the label's real Pango
/// metrics. Returns the layout for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_radio_group(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    group: &RadioGroup,
    theme: &Theme,
) -> RadioGroupLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let layout = group.layout(bounds, |option: &RadioOption| {
        pango_layout.set_text(&option.label);
        pango_layout.set_attributes(None);
        let (_pw, ph) = pango_layout.pixel_size();
        RadioOptionMeasure {
            width: bounds.width,
            height: (ph.max(0) as f32).max(1.0),
        }
    });

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    for (index, option) in group.options.iter().enumerate() {
        let Some(row) = layout.items.get(index) else {
            continue;
        };
        if row.height <= 0.0 || row.width <= 0.0 {
            continue;
        }
        let is_selected = group.selected == Some(index);

        let row_bg = if is_selected {
            theme.selected_bg
        } else {
            theme.background
        };
        let ring = if option.disabled {
            theme.muted_fg
        } else if is_selected && group.focused {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        let label_fg = if option.disabled {
            theme.muted_fg
        } else {
            theme.foreground
        };

        set_source(cr, row_bg);
        cr.rectangle(
            row.x as f64,
            row.y as f64,
            row.width as f64,
            row.height as f64,
        );
        cr.fill().ok();

        let cy = row.y as f64 + row.height as f64 / 2.0;
        let cx = row.x as f64 + DOT_COLUMN_WIDTH / 2.0;
        let r = (row.height as f64 / 2.0 - 2.0).max(2.0);
        set_source(cr, ring);
        cr.set_line_width(1.0);
        cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
        cr.stroke().ok();
        if is_selected {
            set_source(cr, ring);
            cr.arc(cx, cy, (r - 2.0).max(0.0), 0.0, std::f64::consts::TAU);
            cr.fill().ok();
        }

        pango_layout.set_text(&option.label);
        pango_layout.set_attributes(None);
        let (_pw, ph) = pango_layout.pixel_size();
        let text_x = row.x as f64 + DOT_COLUMN_WIDTH;
        let text_y = row.y as f64 + (row.height as f64 - ph as f64) / 2.0;
        set_source(cr, label_fg);
        cr.move_to(text_x, text_y.max(row.y as f64));
        super::painted_text::show_layout(cr, pango_layout);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::radio_group::{RadioGroupHit, RadioOption};
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_pango_layout() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    fn mk_group() -> RadioGroup {
        RadioGroup::new(
            WidgetId::new("settings:theme"),
            vec![
                RadioOption::new(WidgetId::new("settings:theme:light"), "Light"),
                RadioOption::new(WidgetId::new("settings:theme:dark"), "Dark"),
            ],
        )
    }

    /// `radio_group_layout` is documented **ABSOLUTE**: `bounds` must
    /// start at the group's own origin, not (0, 0) — the case that
    /// hides a LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let group = mk_group();
        let layout = gtk_radio_group_layout(&group, x, y, 100.0, 40.0, 16.0);

        assert_eq!(layout.bounds.x as f64, x);
        assert_eq!(layout.bounds.y as f64, y);
        assert_eq!(layout.items.len(), 2);

        let hit = layout.hit_test(crate::event::Point::new(x as f32 + 1.0, y as f32 + 1.0));
        assert_eq!(hit, RadioGroupHit::Option(0));
        let outside = layout.hit_test(crate::event::Point::new(x as f32 + 1.0, y as f32 + 500.0));
        assert_eq!(outside, RadioGroupHit::Outside);

        let (surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&surface).expect("Context::new");
        draw_radio_group(
            &cr,
            &pango_layout,
            x,
            y,
            100.0,
            40.0,
            &group,
            &Theme::default(),
        );
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
    fn disabled_option_still_paints_without_panicking() {
        let (surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let mut group = mk_group();
        group.options[1].disabled = true;
        draw_radio_group(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            100.0,
            40.0,
            &group,
            &Theme::default(),
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let (surface, pango_layout) = headless_pango_layout();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let group = mk_group();
        let layout = draw_radio_group(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            0.0,
            0.0,
            &group,
            &Theme::default(),
        );
        assert_eq!(layout.bounds.width, 0.0);
    }
}
