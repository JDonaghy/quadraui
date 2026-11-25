//! TUI entry points for [`crate::RadioGroup`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::radio_group::cell_paint`,
//! which documents the per-row glyph rules. This module only adapts a
//! raw ratatui [`Buffer`] into the [`crate::CellSurface`] that painter
//! takes, and converts a ratatui cell [`Rect`] into the quadraui one the
//! widget's own cell layout takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitives::radio_group::{
    cell_paint, cell_radio_group_layout, RadioGroup, RadioGroupLayout,
};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`RadioGroup`] without
/// painting. Every option gets one full-width row, one cell tall —
/// [`RadioGroup::layout`]'s `measure` closure is fed a constant size
/// rather than any real text metrics (the TUI backend has no font to
/// measure against).
pub fn tui_radio_group_layout(group: &RadioGroup, area: Rect) -> RadioGroupLayout {
    cell_radio_group_layout(
        group,
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
    )
}

/// Draw a [`RadioGroup`] into `area` on `buf`, one row per option.
/// Returns the layout for host click dispatch.
pub fn draw_radio_group(
    buf: &mut Buffer,
    area: Rect,
    group: &RadioGroup,
    theme: &Theme,
) -> RadioGroupLayout {
    let layout = tui_radio_group_layout(group, area);
    draw_radio_group_layout(buf, group, &layout, theme);
    layout
}

/// Draw a [`RadioGroup`] into `layout.items` on `buf`.
///
/// `layout` must be the same [`RadioGroupLayout`] the host hit-tests
/// against (typically [`tui_radio_group_layout`]'s return value) so
/// paint and click-routing can never disagree.
pub(crate) fn draw_radio_group_layout(
    buf: &mut Buffer,
    group: &RadioGroup,
    layout: &RadioGroupLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(group, layout, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::radio_group::{RadioGroupHit, RadioOption};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_group() -> RadioGroup {
        RadioGroup::new(
            WidgetId::new("settings:theme"),
            vec![
                RadioOption::new(WidgetId::new("settings:theme:light"), "Light"),
                RadioOption::new(WidgetId::new("settings:theme:dark"), "Dark"),
            ],
        )
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_radio_group_layout` bakes `area.x`/`area.y` straight into
    // `RadioGroup::layout`'s returned bounds (absolute frame), so the
    // regression guard is a full paint+hit_test round trip re-run at a
    // non-zero origin.
    fn round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 10, 2);
        let mut buf = Buffer::empty(area);
        let mut group = mk_group();
        group.selected = Some(1);
        group.focused = true;
        let layout = draw_radio_group(&mut buf, area, &group, &Theme::default());

        assert_eq!(cell_char(&buf, origin_x, origin_y), '(');
        assert_eq!(cell_char(&buf, origin_x + 1, origin_y), ' ');
        assert_eq!(cell_char(&buf, origin_x + 4, origin_y), 'L');

        assert_eq!(cell_char(&buf, origin_x, origin_y + 1), '(');
        assert_eq!(cell_char(&buf, origin_x + 1, origin_y + 1), '•');
        assert_eq!(cell_char(&buf, origin_x + 4, origin_y + 1), 'D');

        let hit = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32));
        assert_eq!(hit, RadioGroupHit::Option(0));
        let hit2 = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32 + 1.0));
        assert_eq!(hit2, RadioGroupHit::Option(1));
        let outside = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32 + 50.0));
        assert_eq!(outside, RadioGroupHit::Outside);
    }

    #[test]
    fn paint_and_hit_round_trip() {
        round_trip_at(0, 0);
    }

    #[test]
    fn paint_and_hit_round_trip_at_nonzero_origin() {
        round_trip_at(7, 13);
    }

    #[test]
    fn disabled_option_still_paints_but_dims() {
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let mut group = mk_group();
        group.options[1].disabled = true;
        draw_radio_group(&mut buf, area, &group, &Theme::default());
        assert_eq!(cell_char(&buf, 4, 1), 'D');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(buf_area);
        let group = mk_group();
        let _layout = draw_radio_group(&mut buf, Rect::new(0, 0, 0, 0), &group, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
