//! TUI rasteriser for [`crate::CommandCenter`].
//!
//! Renders `◀ ▶ [🔍 title]` as cell characters, centered in the area.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::command_center::{
    CommandCenter, CommandCenterHit, CommandCenterLayout, CommandCenterMeasure,
};
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
    if area.width == 0 || area.height == 0 {
        return CommandCenterLayout::empty(crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
    }

    let mut layout = tui_command_center_layout(cc, area);

    // The paint loop below only draws an element that fits entirely
    // within `area`; drop bounds/hit-regions for anything that would
    // spill past the edge so the returned layout never describes cells
    // it didn't actually paint (quadraui#649). At `area` widths used in
    // practice this never trips — it only matters when the command
    // center is squeezed narrower than its own content.
    let area_right = area.x as f32 + area.width as f32;
    let fits = |r: crate::event::Rect| r.x >= area.x as f32 && r.x + r.width <= area_right;

    let back_fits = layout.back_bounds.is_some_and(fits);
    let forward_fits = layout.forward_bounds.is_some_and(fits);
    let search_fits = layout.search_bounds.is_some_and(fits);

    if !back_fits {
        layout.back_bounds = None;
    }
    if !forward_fits {
        layout.forward_bounds = None;
    }
    if !search_fits {
        layout.search_bounds = None;
    }
    layout.hit_regions.retain(|(_, hit)| match hit {
        CommandCenterHit::Back => back_fits,
        CommandCenterHit::Forward => forward_fits,
        CommandCenterHit::SearchBox => search_fits,
        CommandCenterHit::Bar | CommandCenterHit::Outside => true,
    });

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
        for (col, ch) in (sx + 1..).zip(cc.search_label.chars()) {
            if col >= sx + sw - 1 {
                break;
            }
            set_cell(buf, col, y, ch, text_fg, bg);
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

    /// Shared body for `arrows_paint_and_click_round_trip[_at_nonzero_origin]`:
    /// `tui_command_center_layout` bakes `area.x`/`area.y` into
    /// `CommandCenter::layout`'s `bounds` (absolute frame), so paint + click
    /// must agree at a non-zero origin too, per LESSONS.md's "layout
    /// helpers must return coords in the same frame across backends"
    /// (quadraui#494).
    fn arrows_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("Search");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        let bb = layout.back_bounds.unwrap();
        let bx = bb.x.round() as u16;
        assert_eq!(cell_char(&buf, bx, origin_y), '◀');

        let hit = layout.hit_test(bb.x + 0.5, origin_y as f32 + 0.5);
        assert_eq!(hit, CommandCenterHit::Back);

        let fb = layout.forward_bounds.unwrap();
        let fx = fb.x.round() as u16;
        assert_eq!(cell_char(&buf, fx, origin_y), '▶');

        let hit = layout.hit_test(fb.x + 0.5, origin_y as f32 + 0.5);
        assert_eq!(hit, CommandCenterHit::Forward);
    }

    #[test]
    fn arrows_paint_and_click_round_trip() {
        arrows_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn arrows_paint_and_click_round_trip_at_nonzero_origin() {
        arrows_paint_and_click_round_trip_at(7, 13);
    }

    /// Shared body for `search_box_paint_and_click_round_trip[_at_nonzero_origin]`
    /// — see `arrows_paint_and_click_round_trip_at` for the non-zero-origin
    /// rationale (quadraui#494).
    fn search_box_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("Test");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        let sb = layout.search_bounds.unwrap();
        let sx = sb.x.round() as u16;
        assert_eq!(cell_char(&buf, sx, origin_y), '[');
        assert_eq!(cell_char(&buf, sx + 1, origin_y), 'T');

        let hit = layout.hit_test(sb.x + 2.0, origin_y as f32 + 0.5);
        assert_eq!(hit, CommandCenterHit::SearchBox);
    }

    #[test]
    fn search_box_paint_and_click_round_trip() {
        search_box_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn search_box_paint_and_click_round_trip_at_nonzero_origin() {
        search_box_paint_and_click_round_trip_at(7, 13);
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
        let layout = draw_command_center(&mut buf, Rect::new(0, 0, 0, 0), &cc, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert!(layout.back_bounds.is_none());
        assert!(layout.forward_bounds.is_none());
        assert!(layout.search_bounds.is_none());
        assert_eq!(layout.hit_test(0.0, 0.0), CommandCenterHit::Outside);
    }

    /// Issue #649: a `width == 0` area used to still get a fully
    /// populated layout back from `tui_command_center_layout` — nothing
    /// painted, but every hit region present, so a host caching that
    /// layout would treat empty cells as clickable.
    #[test]
    fn zero_width_area_is_hit_testably_empty() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let cc = mk_cc("X");
        let layout = draw_command_center(&mut buf, Rect::new(0, 0, 0, 1), &cc, &Theme::default());

        assert!(layout.back_bounds.is_none());
        assert!(layout.forward_bounds.is_none());
        assert!(layout.search_bounds.is_none());
        for x in 0..10 {
            assert_eq!(
                layout.hit_test(x as f32 + 0.5, 0.5),
                CommandCenterHit::Outside
            );
        }
    }

    /// Same as `zero_width_area_is_hit_testably_empty` but for
    /// `height == 0` — the other half of the degenerate-area guard.
    #[test]
    fn zero_height_area_is_hit_testably_empty() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let cc = mk_cc("X");
        let layout = draw_command_center(&mut buf, Rect::new(0, 0, 10, 0), &cc, &Theme::default());

        assert!(layout.back_bounds.is_none());
        assert!(layout.forward_bounds.is_none());
        assert!(layout.search_bounds.is_none());
        assert_eq!(layout.hit_test(0.0, 0.0), CommandCenterHit::Outside);
    }

    /// Issue #649: when the command center is squeezed narrower than its
    /// own content, the search box doesn't fit inside `area` — the paint
    /// loop must skip it (not spill past the edge into a wider host
    /// buffer), and the returned layout must not report `search_bounds`
    /// or hit-test it, matching what was actually painted.
    #[test]
    fn clipped_search_box_does_not_report_search_bounds() {
        // content_width for "Search" is 16 cells (2 arrows * 2 + gap 1 +
        // gap 1 + search box 10); an 8-wide area can fit the arrows but
        // not the search box.
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        let cc = mk_cc("Search");
        let layout = draw_command_center(&mut buf, area, &cc, &Theme::default());

        assert!(
            layout.search_bounds.is_none(),
            "search box doesn't fit in an 8-wide area; its bounds must not be reported"
        );
        // Paint and layout must agree: nothing was drawn at the search
        // box's would-be position (column 6), so it stays background.
        assert_eq!(cell_char(&buf, 6, 0), ' ');
        assert_eq!(layout.hit_test(6.5, 0.5), CommandCenterHit::Bar);
    }
}
