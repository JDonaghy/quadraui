//! TUI rasteriser for [`crate::Split`].
//!
//! Paints only the divider — pane content is the app's responsibility.
//! Horizontal splits draw a `│` column; vertical splits draw a `─` row.
//! Where a divider's ends (or an interior crossing) meet an already
//! painted perpendicular divider, the run is upgraded to the matching
//! box-drawing junction glyph (`┼ ├ ┤ ┬ ┴`) — see
//! [`super::split_junction`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::split_junction::upgrade_junctions;
use super::{ratatui_color, set_cell};
use crate::primitives::split::{Split, SplitDirection, SplitHit, SplitLayout, SplitMeasure};
use crate::theme::Theme;
use crate::types::WidgetId;

const TUI_DIVIDER_THICKNESS: f32 = 1.0;

/// Compute the TUI cell-unit layout for a [`Split`] without painting.
pub fn tui_split_layout(split: &Split, area: Rect) -> SplitLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let layout = split.layout(bounds, SplitMeasure::new(TUI_DIVIDER_THICKNESS));
    snap_divider_hit_region_to_painted_cell(layout, split.direction, split.id)
}

/// Rebuilds the divider + following-pane entries in `layout.hit_regions`
/// so a TUI mouse click (always reported at an integer cell coordinate)
/// lands on the exact cell `draw_split` paints the divider at —
/// `[trunc(position), trunc(position) + thickness)` — instead of the
/// continuous `[position, position + thickness)` band `Split::layout`
/// computes. A fractional ratio (e.g. `0.5` over an odd `available`,
/// landing on `39.5`) used to paint at the truncated cell `39` while the
/// click region still started at `39.5`, so no integer cell ever hit it
/// — dragging the divider silently did nothing. Only the *click*
/// targets move here; `divider_bounds`/`first_bounds`/`second_bounds`
/// (and `resolved_ratio`) keep `Split::layout`'s precise float values,
/// so hosts positioning pane content see no change.
fn snap_divider_hit_region_to_painted_cell(
    mut layout: SplitLayout,
    direction: SplitDirection,
    id: WidgetId,
) -> SplitLayout {
    match direction {
        SplitDirection::Horizontal => {
            let trunc_x = layout.divider_bounds.x.max(0.0).trunc();
            let thickness = layout.divider_bounds.width;
            let divider_hit = crate::event::Rect::new(
                trunc_x,
                layout.divider_bounds.y,
                thickness,
                layout.divider_bounds.height,
            );
            let second_right = layout.second_bounds.x + layout.second_bounds.width;
            let second_hit = crate::event::Rect::new(
                trunc_x + thickness,
                layout.second_bounds.y,
                (second_right - (trunc_x + thickness)).max(0.0),
                layout.second_bounds.height,
            );
            layout.hit_regions = vec![
                (divider_hit, SplitHit::Divider(id)),
                (layout.first_bounds, SplitHit::FirstPane(id)),
                (second_hit, SplitHit::SecondPane(id)),
            ];
        }
        SplitDirection::Vertical => {
            let trunc_y = layout.divider_bounds.y.max(0.0).trunc();
            let thickness = layout.divider_bounds.height;
            let divider_hit = crate::event::Rect::new(
                layout.divider_bounds.x,
                trunc_y,
                layout.divider_bounds.width,
                thickness,
            );
            let second_bottom = layout.second_bounds.y + layout.second_bounds.height;
            let second_hit = crate::event::Rect::new(
                layout.second_bounds.x,
                trunc_y + thickness,
                layout.second_bounds.width,
                (second_bottom - (trunc_y + thickness)).max(0.0),
            );
            layout.hit_regions = vec![
                (divider_hit, SplitHit::Divider(id)),
                (layout.first_bounds, SplitHit::FirstPane(id)),
                (second_hit, SplitHit::SecondPane(id)),
            ];
        }
    }
    layout
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
            // Truncate (not round) the divider's own axis coordinate —
            // the same float->cell convention as
            // `SplitTreeDivider::cell_position`, so a fractional-ratio
            // `Split` divider paints at the same cell a `SplitTree`
            // divider at the same position would.
            let x = div.x.max(0.0) as u16;
            let start_y = div.y.round() as u16;
            let h = div.height.round() as u16;
            let cells: Vec<(u16, u16)> = (0..h).map(|dy| (x, start_y + dy)).collect();
            for &(cx, cy) in &cells {
                set_cell(buf, cx, cy, '│', fg, bg);
            }
            upgrade_junctions(buf, &cells, true, fg, bg);
        }
        SplitDirection::Vertical => {
            // See the `Horizontal` arm above for why this truncates.
            let y = div.y.max(0.0) as u16;
            let start_x = div.x.round() as u16;
            let w = div.width.round() as u16;
            let cells: Vec<(u16, u16)> = (0..w).map(|dx| (start_x + dx, y)).collect();
            for &(cx, cy) in &cells {
                set_cell(buf, cx, cy, '─', fg, bg);
            }
            upgrade_junctions(buf, &cells, false, fg, bg);
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
    // must return coords in the same frame across backends".
    // `tui_split_layout` bakes `area.x`/`area.y`
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

    /// Regression guard: non-zero-origin cases must be handled correctly.
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

    /// Regression guard: non-zero-origin cases must be handled correctly.
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

    /// An app that manually nests two independent
    /// `draw_split` calls (rather than a `SplitTree`) still gets a
    /// junction glyph where the second, full-extent divider crosses the
    /// first, shorter one already sitting in the buffer — this is the
    /// "read the four neighbours back out of the buffer" trick the
    /// issue describes, exercised end-to-end rather than as a synthetic
    /// buffer in `split_junction`'s own unit tests.
    ///
    /// Order matters for a *single*-pass-per-call API like this one: the
    /// divider whose own run spans the crossing (the "long" one) must be
    /// painted *after* the one it crosses, so its own upgrade pass can
    /// read the already-painted perpendicular glyph back. `SplitTree`
    /// (see `split_tree.rs`) sidesteps the ordering requirement with a
    /// two-phase paint; plain `Split` has no such batching, so this is
    /// the contract host code nesting `Split`s by hand must follow.
    #[test]
    fn manually_nested_splits_form_a_junction_when_the_spanning_one_paints_last() {
        let area = Rect::new(0, 0, 21, 11);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();

        // Inner: a Vertical (stacked) split confined to the left third
        // of the area — its '─' row lands at y=5, x=0..9.
        let inner_area = Rect::new(0, 0, 10, 11);
        let inner = vsplit(0.5);
        let inner_layout = draw_split(&mut buf, inner_area, &inner, &theme);
        let inner_row = inner_layout.divider_bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, 4, inner_row), '─');

        // Outer: a Horizontal (side-by-side) split over the FULL area,
        // ratio chosen so its '│' column (x=5) crosses the inner row —
        // painted *last*, so it can see the inner run already there.
        let outer = hsplit(0.25);
        let outer_layout = draw_split(&mut buf, area, &outer, &theme);
        let outer_col = outer_layout.divider_bounds.x.round() as u16;
        assert_eq!(outer_col, 5);

        assert_eq!(cell_char(&buf, outer_col, inner_row), '┼');
        // Away from the crossing row, the outer column is still plain.
        assert_eq!(cell_char(&buf, outer_col, inner_row + 2), '│');
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

    /// Pins the TUI divider float->cell conversion to truncation, not
    /// rounding — the same rule `tui::split_tree`'s
    /// `fractional_ratio_round_trip_at` pins for `SplitTree`. A ratio
    /// chosen so the divider position is NOT cell-aligned (20.7, not
    /// 20.0 or 21.0) makes a round-vs-truncate regression visible: with
    /// `.round()` this would paint at column 21, not 20.
    #[test]
    fn divider_position_truncates_for_a_fractional_ratio() {
        let area = Rect::new(0, 0, 101, 10); // available = 100
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.207); // position = 20.7
        let layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert!((layout.divider_bounds.x - 20.7).abs() < 0.01);

        assert_eq!(cell_char(&buf, 20, 0), '│');
        assert_eq!(cell_char(&buf, 21, 0), ' ');
    }

    /// A fractional divider position that lands exactly on a half-cell
    /// boundary (`0.5` ratio over an odd `available`, e.g. 79) must still
    /// be clickable at the cell it actually paints at — the integer
    /// mouse coordinate a real TUI backend reports. Before
    /// `snap_divider_hit_region_to_painted_cell`, the painted cell
    /// (`39`, from truncating `39.5`) fell just outside the continuous
    /// `[39.5, 40.5)` hit band, so this exact click silently missed.
    #[test]
    fn divider_is_clickable_at_the_cell_it_paints_at_for_a_half_cell_ratio() {
        let area = Rect::new(0, 0, 80, 10); // available = 79, position = 39.5
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert!((layout.divider_bounds.x - 39.5).abs() < 0.01);

        let painted_cell = 39u16;
        assert_eq!(cell_char(&buf, painted_cell, 5), '│');
        assert_eq!(
            layout.hit_test(painted_cell as f32, 5.0),
            SplitHit::Divider(split.id),
            "clicking the exact painted cell must register as the divider"
        );
        assert_eq!(
            layout.hit_test(40.0, 5.0),
            SplitHit::SecondPane(split.id),
            "the cell just past the divider is the second pane, not a dead zone"
        );
    }
}
