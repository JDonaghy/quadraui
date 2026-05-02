//! GTK rasteriser for [`crate::Split`].
//!
//! Paints only the divider as a filled rectangle — pane content is the
//! app's responsibility. The divider thickness is derived from
//! `line_height` (4px default at typical font sizes).

use gtk4::cairo::Context;

use super::set_source;
use crate::event::Rect;
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::theme::Theme;

const GTK_DIVIDER_PX: f32 = 4.0;

/// Compute the GTK pixel-unit layout for a [`Split`] without painting.
pub fn gtk_split_layout(split: &Split, x: f64, y: f64, w: f64, h: f64) -> SplitLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    split.layout(bounds, SplitMeasure::new(GTK_DIVIDER_PX))
}

/// Draw a [`Split`] divider onto `cr`. Returns the layout for host
/// click/drag dispatch. Pane content is NOT painted.
#[allow(clippy::too_many_arguments)]
pub fn draw_split(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    split: &Split,
    theme: &Theme,
) -> SplitLayout {
    let layout = gtk_split_layout(split, x, y, w, h);

    let div = &layout.divider_bounds;
    set_source(cr, theme.separator);
    cr.rectangle(
        div.x as f64,
        div.y as f64,
        div.width as f64,
        div.height as f64,
    );
    cr.fill().ok();

    layout
}
