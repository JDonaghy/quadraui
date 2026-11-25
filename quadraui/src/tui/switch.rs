//! TUI entry points for [`crate::Switch`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::switch::cell_paint`,
//! which documents the track/knob rules. This module only adapts a raw
//! ratatui [`Buffer`] into the [`crate::CellSurface`] that painter takes,
//! and converts a ratatui cell [`Rect`] into the quadraui one the
//! widget's own cell layout takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitives::switch::{cell_paint, cell_switch_layout, Switch, SwitchLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`Switch`] without painting.
/// The control has no internal sub-regions, so `area` maps straight
/// through to [`Switch::layout`]'s bounds.
pub fn tui_switch_layout(switch: &Switch, area: Rect) -> SwitchLayout {
    cell_switch_layout(
        switch,
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
    )
}

/// Draw a [`Switch`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
pub fn draw_switch(buf: &mut Buffer, area: Rect, switch: &Switch, theme: &Theme) -> SwitchLayout {
    let layout = tui_switch_layout(switch, area);
    draw_switch_layout(buf, switch, &layout, theme);
    layout
}

/// Draw a [`Switch`] into `layout.bounds` on `buf`.
///
/// `layout` must be the same [`SwitchLayout`] the host hit-tests against
/// (typically [`tui_switch_layout`]'s return value) so paint and
/// click-routing can never disagree.
pub(crate) fn draw_switch_layout(
    buf: &mut Buffer,
    switch: &Switch,
    layout: &SwitchLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(switch, layout, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::switch::SwitchHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_switch(value: bool) -> Switch {
        let mut s = Switch::new(WidgetId::new("settings:wrap"));
        s.value = value;
        s
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_switch_layout` bakes `area.x`/`area.y` straight into
    // `Switch::layout`'s returned bounds (absolute frame), so the
    // regression guard is a full paint+hit_test round trip re-run at a
    // non-zero origin.
    fn off_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 4, 1);
        let mut buf = Buffer::empty(area);
        let switch = mk_switch(false);
        let layout = draw_switch(&mut buf, area, &switch, &Theme::default());

        assert_eq!(cell_char(&buf, origin_x, origin_y), '[');
        assert_eq!(cell_char(&buf, origin_x + 1, origin_y), '●');
        assert_eq!(cell_char(&buf, origin_x + 2, origin_y), ' ');
        assert_eq!(cell_char(&buf, origin_x + 3, origin_y), ']');

        let hit = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32));
        assert_eq!(hit, SwitchHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 50.0, origin_y as f32));
        assert_eq!(outside, SwitchHit::Outside);
    }

    #[test]
    fn off_paint_and_hit_round_trip() {
        off_round_trip_at(0, 0);
    }

    #[test]
    fn off_paint_and_hit_round_trip_at_nonzero_origin() {
        off_round_trip_at(7, 13);
    }

    #[test]
    fn on_moves_knob_to_the_right() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        let switch = mk_switch(true);
        draw_switch(&mut buf, area, &switch, &Theme::default());
        assert_eq!(cell_char(&buf, 1, 0), ' ');
        assert_eq!(cell_char(&buf, 2, 0), '●');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let switch = mk_switch(false);
        let _layout = draw_switch(&mut buf, Rect::new(0, 0, 0, 0), &switch, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
