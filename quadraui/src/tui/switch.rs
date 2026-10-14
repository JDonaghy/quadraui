//! TUI rasteriser for [`crate::Switch`].
//!
//! Paints a 4-cell `[● ]`/`[ ●]` track with the knob on the right when
//! `value` is `true`, on the left when `false`. The whole cell range is
//! one hit target, same as [`crate::Switch::layout`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::switch::{Switch, SwitchLayout};
use crate::theme::Theme;

const TUI_SWITCH_WIDTH: u16 = 4;

/// Compute the TUI cell-unit layout for a [`Switch`] without painting.
/// The control has no internal sub-regions, so `area` maps straight
/// through to [`Switch::layout`]'s bounds.
pub fn tui_switch_layout(switch: &Switch, area: Rect) -> SwitchLayout {
    switch.layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Draw a [`Switch`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
pub fn draw_switch(buf: &mut Buffer, area: Rect, switch: &Switch, theme: &Theme) -> SwitchLayout {
    let layout = tui_switch_layout(switch, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let y = layout.bounds.y.round() as u16;
    let x0 = layout.bounds.x.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let end = x0.saturating_add(w);

    let track_bg = if switch.disabled {
        ratatui_color(theme.surface_bg)
    } else if switch.value {
        ratatui_color(theme.accent_bg)
    } else {
        ratatui_color(theme.surface_bg)
    };
    let brk_fg = if switch.disabled {
        ratatui_color(theme.muted_fg)
    } else if switch.focused {
        ratatui_color(theme.accent_fg)
    } else {
        ratatui_color(theme.border_fg)
    };
    let knob_fg = if switch.disabled {
        ratatui_color(theme.muted_fg)
    } else {
        ratatui_color(theme.foreground)
    };

    let track_w = TUI_SWITCH_WIDTH.min(w);
    let knob_col = if track_w <= 2 {
        x0
    } else if switch.value {
        x0 + track_w - 2
    } else {
        x0 + 1
    };

    let mut col = x0;
    if col < end {
        set_cell(buf, col, y, '[', brk_fg, track_bg);
        col += 1;
    }
    while col < x0 + track_w.saturating_sub(1) && col < end {
        let ch = if col == knob_col { '●' } else { ' ' };
        let fg = if col == knob_col { knob_fg } else { track_bg };
        set_cell(buf, col, y, ch, fg, track_bg);
        col += 1;
    }
    if col < end {
        set_cell(buf, col, y, ']', brk_fg, track_bg);
        col += 1;
    }
    while col < end {
        set_cell(buf, col, y, ' ', knob_fg, ratatui_color(theme.background));
        col += 1;
    }

    layout
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
