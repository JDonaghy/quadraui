//! GTK rasteriser for [`crate::Panel`].
//!
//! Paints panel chrome: title bar (background rect + Pango title text
//! + action glyphs) and leaves `content_bounds` for the app.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::set_source;
use crate::event::Rect;
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;

const GTK_ACTION_BUTTON_PX: f32 = 24.0;

/// Compute the GTK pixel-unit layout for a [`Panel`] without painting.
pub fn gtk_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height as f32
        } else {
            0.0
        },
        action_button_width: GTK_ACTION_BUTTON_PX,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

/// Draw a [`Panel`] chrome onto `cr`. Returns the layout for host
/// click dispatch. Content is NOT painted.
#[allow(clippy::too_many_arguments)]
pub fn draw_panel(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &Panel,
    theme: &Theme,
    line_height: f64,
) -> PanelLayout {
    let layout = gtk_panel_layout(panel, x, y, w, h, line_height);

    // Paint title bar.
    if let Some(tb) = layout.title_bar_bounds {
        let title_bg = panel.accent.unwrap_or(theme.separator);
        set_source(cr, title_bg);
        cr.rectangle(tb.x as f64, tb.y as f64, tb.width as f64, tb.height as f64);
        cr.fill().ok();

        // Title text.
        if let Some(ref title) = panel.title {
            let text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            pango_layout.set_text(&text);
            pango_layout.set_attributes(None);
            set_source(cr, theme.foreground);
            cr.move_to(tb.x as f64 + 4.0, tb.y as f64);
            pcfn::show_layout(cr, pango_layout);
        }

        // Action buttons.
        for va in &layout.visible_actions {
            let action = &panel.actions[va.action_idx];
            let action_bg = if action.is_active {
                theme.accent_bg
            } else {
                title_bg
            };
            set_source(cr, action_bg);
            cr.rectangle(
                va.bounds.x as f64,
                va.bounds.y as f64,
                va.bounds.width as f64,
                va.bounds.height as f64,
            );
            cr.fill().ok();

            pango_layout.set_text(&action.icon);
            pango_layout.set_attributes(None);
            set_source(cr, theme.foreground);
            let text_w = pango_layout.pixel_size().0 as f64;
            let glyph_x = va.bounds.x as f64 + (va.bounds.width as f64 - text_w) / 2.0;
            cr.move_to(glyph_x, va.bounds.y as f64);
            pcfn::show_layout(cr, pango_layout);
        }
    }

    layout
}
