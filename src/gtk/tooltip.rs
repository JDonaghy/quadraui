//! GTK rasteriser for [`crate::Tooltip`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_tooltip`. Paints a
//! rectangle (background + 1 px border) at the resolved bounds, then
//! draws either the plain `text` or per-row `styled_lines`.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout position.
///
/// `padding_x` is the horizontal padding (in pixels) from the left
/// border to the start of text — consumers typically pass the same
/// `char_width` they used when computing the tooltip's measured width.
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
#[allow(clippy::too_many_arguments)]
pub fn draw_tooltip(
    cr: &Context,
    layout: &pango::Layout,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    let bounds = tooltip_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }

    let bg = tooltip
        .bg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_bg));
    let fg = tooltip
        .fg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_fg));
    let border = cairo_rgb(theme.hover_border);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
    );
    cr.fill().ok();

    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.rectangle(
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
    );
    cr.stroke().ok();

    let text_x = bounds.x as f64 + padding_x;
    let text_top = bounds.y as f64 + 2.0;

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate() {
            let row_y = text_top + i as f64 * line_height;
            if row_y + line_height > bounds.y as f64 + bounds.height as f64 {
                break;
            }
            let mut x_off = text_x;
            for span in &styled.spans {
                let span_fg = span.fg.map(cairo_rgb).unwrap_or(fg);
                cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
                layout.set_text(&span.text);
                layout.set_attributes(None);
                cr.move_to(x_off, row_y);
                pcfn::show_layout(cr, layout);
                let (text_w, _) = layout.pixel_size();
                x_off += text_w as f64;
            }
        }
        return;
    }

    cr.set_source_rgb(fg.0, fg.1, fg.2);
    for (i, text_line) in tooltip.text.lines().enumerate() {
        let row_y = text_top + i as f64 * line_height;
        if row_y + line_height > bounds.y as f64 + bounds.height as f64 {
            break;
        }
        layout.set_text(text_line);
        layout.set_attributes(None);
        cr.move_to(text_x, row_y);
        pcfn::show_layout(cr, layout);
    }
}
