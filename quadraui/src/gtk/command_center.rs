//! GTK rasteriser for [`crate::CommandCenter`].
//!
//! Renders back/forward arrows via Pango and a rounded-rect bordered
//! search box, centered within the given area.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::set_source;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
use crate::theme::Theme;

const GTK_ARROW_WIDTH_PX: f32 = 24.0;
const GTK_GAP_PX: f32 = 8.0;
const GTK_SEARCH_PAD_PX: f64 = 8.0;
const GTK_SEARCH_MIN_WIDTH: f32 = 280.0;
const GTK_CORNER_RADIUS: f64 = 4.0;

/// Compute GTK pixel-unit layout for a [`CommandCenter`] without painting.
pub fn gtk_command_center_layout(
    cc: &CommandCenter,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> CommandCenterLayout {
    let search_w = if cc.search_label.is_empty() {
        0.0
    } else {
        pango_layout.set_text(&cc.search_label);
        pango_layout.set_attributes(None);
        let text_w = pango_layout.pixel_size().0 as f32;
        (text_w + GTK_SEARCH_PAD_PX as f32 * 2.0).max(GTK_SEARCH_MIN_WIDTH)
    };
    cc.layout(
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
        CommandCenterMeasure {
            arrow_width: GTK_ARROW_WIDTH_PX,
            gap: GTK_GAP_PX,
            search_box_width: search_w,
            height: h as f32,
        },
    )
}

/// Draw a [`CommandCenter`] onto `cr`. Returns the layout for host
/// click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_command_center(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    cc: &CommandCenter,
    theme: &Theme,
    line_height: f64,
) -> CommandCenterLayout {
    let layout = gtk_command_center_layout(cc, pango_layout, x, y, w, h);

    // Background.
    set_source(cr, theme.tab_bar_bg);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    let enabled_fg = theme.tab_inactive_fg;
    let disabled_fg = theme.muted_fg;
    let text_y = y + (h - line_height) / 2.0;

    // Back arrow.
    if let Some(bb) = layout.back_bounds {
        let fg = if cc.back_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        pango_layout.set_text("◀");
        pango_layout.set_attributes(None);
        let tw = pango_layout.pixel_size().0 as f64;
        set_source(cr, fg);
        cr.move_to(bb.x as f64 + (bb.width as f64 - tw) / 2.0, text_y);
        pcfn::show_layout(cr, pango_layout);
    }

    // Forward arrow.
    if let Some(fb) = layout.forward_bounds {
        let fg = if cc.forward_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        pango_layout.set_text("▶");
        pango_layout.set_attributes(None);
        let tw = pango_layout.pixel_size().0 as f64;
        set_source(cr, fg);
        cr.move_to(fb.x as f64 + (fb.width as f64 - tw) / 2.0, text_y);
        pcfn::show_layout(cr, pango_layout);
    }

    // Search box.
    if let Some(sb) = layout.search_bounds {
        let bx = sb.x as f64;
        let by = sb.y as f64 + 2.0;
        let bw = sb.width as f64;
        let bh = sb.height as f64 - 4.0;
        let r = GTK_CORNER_RADIUS.min(bh / 2.0);

        // Rounded rect border.
        set_source(cr, theme.separator);
        cr.new_path();
        cr.arc(bx + bw - r, by + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(
            bx + bw - r,
            by + bh - r,
            r,
            0.0,
            std::f64::consts::FRAC_PI_2,
        );
        cr.arc(
            bx + r,
            by + bh - r,
            r,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI,
        );
        cr.arc(
            bx + r,
            by + r,
            r,
            std::f64::consts::PI,
            3.0 * std::f64::consts::FRAC_PI_2,
        );
        cr.close_path();
        cr.set_line_width(1.0);
        cr.stroke().ok();

        // Search text.
        pango_layout.set_text(&cc.search_label);
        pango_layout.set_attributes(None);
        set_source(cr, theme.tab_inactive_fg);
        cr.move_to(bx + GTK_SEARCH_PAD_PX, text_y);
        pcfn::show_layout(cr, pango_layout);
    }

    layout
}
