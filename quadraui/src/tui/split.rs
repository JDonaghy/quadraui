//! TUI rasteriser for [`crate::Split`].
//!
//! Paints only the divider — pane content is the app's responsibility.
//! Horizontal splits draw a `│` column; vertical splits draw a `─` row.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::split::{Split, SplitDirection, SplitLayout, SplitMeasure};
use crate::theme::Theme;

const TUI_DIVIDER_THICKNESS: f32 = 1.0;

/// Compute the TUI cell-unit layout for a [`Split`] without painting.
pub fn tui_split_layout(split: &Split, area: Rect) -> SplitLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    split.layout(bounds, SplitMeasure::new(TUI_DIVIDER_THICKNESS))
}

/// Draw a [`Split`] divider into `area` on `buf`. Returns the layout
/// for host click/drag dispatch. Pane content is NOT painted — the
/// app draws into `layout.first_bounds` / `layout.second_bounds`.
pub fn draw_split(buf: &mut Buffer, area: Rect, split: &Split, theme: &Theme) -> SplitLayout {
    let layout = tui_split_layout(split, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let div = &layout.divider_bounds;
    let fg = ratatui_color(theme.separator);
    let bg = ratatui_color(theme.background);

    match split.direction {
        SplitDirection::Horizontal => {
            let x = div.x.round() as u16;
            let start_y = div.y.round() as u16;
            let h = div.height.round() as u16;
            for dy in 0..h {
                set_cell(buf, x, start_y + dy, '│', fg, bg);
            }
        }
        SplitDirection::Vertical => {
            let y = div.y.round() as u16;
            let start_x = div.x.round() as u16;
            let w = div.width.round() as u16;
            for dx in 0..w {
                set_cell(buf, start_x + dx, y, '─', fg, bg);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{Split, SplitDirection, SplitHit};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn hsplit(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Horizontal,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn vsplit(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Vertical,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends"
    // (quadraui#494). `tui_split_layout` bakes `area.x`/`area.y`
    // straight into `split.layout`'s returned bounds (absolute frame),
    // so the regression guard is a full paint+hit_test round trip
    // re-run at a non-zero origin.

    fn horizontal_round_trip_at(origin_x: u16, origin_y: u16) {
        // Use 41 cols so available=40, 0.5*40=20 → divider at x=20, integer.
        let area = Rect::new(origin_x, origin_y, 41, 10);
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_x = layout.divider_bounds.x.round() as u16;
        assert_eq!(cell_char(&buf, div_x, origin_y), '│');

        let mid_y = origin_y as f32 + 5.0;
        let hit = layout.hit_test(layout.divider_bounds.x + 0.5, mid_y);
        assert_eq!(hit, SplitHit::Divider(WidgetId::new("split")));

        let hit_first = layout.hit_test(origin_x as f32 + 1.0, mid_y);
        assert_eq!(hit_first, SplitHit::FirstPane(WidgetId::new("split")));

        let hit_second = layout.hit_test(layout.divider_bounds.x + 1.5, mid_y);
        assert_eq!(hit_second, SplitHit::SecondPane(WidgetId::new("split")));
    }

    #[test]
    fn horizontal_paint_and_click_round_trip() {
        horizontal_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md).
    #[test]
    fn horizontal_paint_and_click_round_trip_at_nonzero_origin() {
        horizontal_round_trip_at(7, 13);
    }

    fn vertical_round_trip_at(origin_x: u16, origin_y: u16) {
        // Use 21 rows so available=20, 0.5*20=10 → divider at y=10, integer.
        let area = Rect::new(origin_x, origin_y, 40, 21);
        let mut buf = Buffer::empty(area);
        let split = vsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_y = layout.divider_bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, origin_x, div_y), '─');

        let mid_x = origin_x as f32 + 20.0;
        let hit = layout.hit_test(mid_x, layout.divider_bounds.y + 0.5);
        assert_eq!(hit, SplitHit::Divider(WidgetId::new("split")));

        let hit_first = layout.hit_test(mid_x, origin_y as f32 + 1.0);
        assert_eq!(hit_first, SplitHit::FirstPane(WidgetId::new("split")));

        let hit_second = layout.hit_test(mid_x, layout.divider_bounds.y + 1.5);
        assert_eq!(hit_second, SplitHit::SecondPane(WidgetId::new("split")));
    }

    #[test]
    fn vertical_paint_and_click_round_trip() {
        vertical_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md).
    #[test]
    fn vertical_paint_and_click_round_trip_at_nonzero_origin() {
        vertical_round_trip_at(7, 13);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let split = hsplit(0.5);
        let _layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn divider_position_matches_ratio() {
        let area = Rect::new(0, 0, 41, 10);
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.3);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_x = layout.divider_bounds.x.round() as u16;
        // 41 cols, 1-cell divider → 40 available. 0.3 * 40 = 12.
        assert_eq!(div_x, 12);
        assert_eq!(cell_char(&buf, div_x, 0), '│');
        assert_eq!(cell_char(&buf, div_x - 1, 0), ' ');
        assert_eq!(cell_char(&buf, div_x + 1, 0), ' ');
    }
}
