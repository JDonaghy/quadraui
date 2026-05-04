//! TUI rasteriser for [`crate::CommandCenter`].
//!
//! Renders `◀ ▶ [🔍 title]` as cell characters, centered in the area.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
use crate::theme::Theme;

const TUI_ARROW_WIDTH: f32 = 2.0;
const TUI_GAP: f32 = 1.0;

/// Compute TUI cell-unit layout for a [`CommandCenter`] without painting.
pub fn tui_command_center_layout(cc: &CommandCenter, area: Rect) -> CommandCenterLayout {
    let search_w = if cc.search_label.is_empty() {
        0.0
    } else {
        (cc.search_label.chars().count() + 4) as f32
    };
    cc.layout(
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        CommandCenterMeasure {
            arrow_width: TUI_ARROW_WIDTH,
            gap: TUI_GAP,
            search_box_width: search_w,
            height: area.height.min(1) as f32,
        },
    )
}

/// Draw a [`CommandCenter`] into `area` on `buf`. Returns the layout
/// for host click dispatch.
pub fn draw_command_center(
    buf: &mut Buffer,
    area: Rect,
    cc: &CommandCenter,
    theme: &Theme,
) -> CommandCenterLayout {
    let layout = tui_command_center_layout(cc, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let bg = ratatui_color(theme.tab_bar_bg);
    let enabled_fg = ratatui_color(theme.tab_inactive_fg);
    let disabled_fg = ratatui_color(theme.muted_fg);
    let border_fg = ratatui_color(theme.muted_fg);
    let text_fg = ratatui_color(theme.tab_inactive_fg);

    // Fill background.
    let y = area.y;
    for x in area.x..area.x + area.width {
        set_cell(buf, x, y, ' ', enabled_fg, bg);
    }

    // Back arrow.
    if let Some(bb) = layout.back_bounds {
        let bx = bb.x.round() as u16;
        let fg = if cc.back_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        set_cell(buf, bx, y, '◀', fg, bg);
    }

    // Forward arrow.
    if let Some(fb) = layout.forward_bounds {
        let fx = fb.x.round() as u16;
        let fg = if cc.forward_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        set_cell(buf, fx, y, '▶', fg, bg);
    }

    // Search box.
    if let Some(sb) = layout.search_bounds {
        let sx = sb.x.round() as u16;
        let sw = sb.width.round() as u16;
        set_cell(buf, sx, y, '[', border_fg, bg);
        let mut col = sx + 1;
        for ch in cc.search_label.chars() {
            if col >= sx + sw - 1 {
                break;
            }
            set_cell(buf, col, y, ch, text_fg, bg);
            col += 1;
        }
        if sw > 1 {
            set_cell(buf, sx + sw - 1, y, ']', border_fg, bg);
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::command_center::CommandCenterHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_cc(search: &str) -> CommandCenter {
        CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: search.into(),
        }
    }

    #[test]
    fn arrows_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("Search");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        let bb = layout.back_bounds.unwrap();
        let bx = bb.x.round() as u16;
        assert_eq!(cell_char(&buf, bx, 0), '◀');

        let hit = layout.hit_test(bb.x + 0.5, 0.5);
        assert_eq!(hit, CommandCenterHit::Back);

        let fb = layout.forward_bounds.unwrap();
        let fx = fb.x.round() as u16;
        assert_eq!(cell_char(&buf, fx, 0), '▶');

        let hit = layout.hit_test(fb.x + 0.5, 0.5);
        assert_eq!(hit, CommandCenterHit::Forward);
    }

    #[test]
    fn search_box_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("Test");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        let sb = layout.search_bounds.unwrap();
        let sx = sb.x.round() as u16;
        assert_eq!(cell_char(&buf, sx, 0), '[');
        assert_eq!(cell_char(&buf, sx + 1, 0), 'T');

        let hit = layout.hit_test(sb.x + 2.0, 0.5);
        assert_eq!(hit, CommandCenterHit::SearchBox);
    }

    #[test]
    fn no_search_box_when_label_empty() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        assert!(layout.search_bounds.is_none());
    }

    #[test]
    fn outside_hit() {
        let area = Rect::new(10, 0, 20, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let cc = mk_cc("X");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        assert_eq!(layout.hit_test(0.0, 0.5), CommandCenterHit::Outside);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let cc = mk_cc("X");
        let _layout = draw_command_center(&mut buf, Rect::new(0, 0, 0, 0), &cc, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
