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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{Split, SplitDirection, SplitHit};
    use crate::types::WidgetId;

    fn round_trip_at(x: f64, y: f64) {
        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let layout = gtk_split_layout(&split, x, y, 100.0, 40.0);

        // `split_layout` is documented **ABSOLUTE** (issue #505):
        // `first_bounds` must start exactly at the origin the split was
        // laid out at, not at (0, 0).
        assert_eq!(layout.first_bounds.x as f64, x);
        assert_eq!(layout.first_bounds.y as f64, y);

        // A click inside the first pane's own (absolute) bounds must
        // resolve back to it without any further coordinate shift.
        let cx = layout.first_bounds.x + layout.first_bounds.width / 2.0;
        let cy = layout.first_bounds.y + layout.first_bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(cx, cy),
            SplitHit::FirstPane(split.id.clone())
        );

        let dx = layout.divider_bounds.x + layout.divider_bounds.width / 2.0;
        let dy = layout.divider_bounds.y + layout.divider_bounds.height / 2.0;
        assert_eq!(layout.hit_test(dx, dy), SplitHit::Divider(split.id));
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_split_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
