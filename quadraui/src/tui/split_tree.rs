//! TUI rasteriser for [`crate::SplitTree`].
//!
//! Paints only the dividers — leaf content is the app's responsibility,
//! painted into the rects `SplitTreeLayout::leaves` returns. Mirrors
//! [`super::split::draw_split`]'s divider glyphs: `│` for `Horizontal`
//! (side-by-side) splits, `─` for `Vertical` (stacked) splits.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::split_tree::{SplitDirection, SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::theme::Theme;

const TUI_DIVIDER_THICKNESS: f32 = 1.0;

/// Compute the TUI cell-unit layout for a [`SplitTree`] without
/// painting. Hosts call this in drag/click handlers so hit-testing
/// consumes the exact same geometry `draw_split_tree` paints from —
/// never re-derive it with a hand-rolled measurer (Primitive Rule 2).
pub fn tui_split_tree_layout(tree: &SplitTree, area: Rect) -> SplitTreeLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    tree.layout(bounds, SplitTreeMeasure::new(TUI_DIVIDER_THICKNESS))
}

/// Draw a [`SplitTree`]'s dividers into `area` on `buf`. Returns the
/// layout for host click/drag dispatch. Leaf content is NOT painted —
/// the app draws into each `layout.leaves[i].1` rect.
pub fn draw_split_tree(
    buf: &mut Buffer,
    area: Rect,
    tree: &SplitTree,
    theme: &Theme,
) -> SplitTreeLayout {
    let layout = tui_split_tree_layout(tree, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let fg = ratatui_color(theme.separator);
    let bg = ratatui_color(theme.background);

    for div in &layout.dividers {
        // #452-class fix: paint at the exact same truncated cell
        // `SplitTreeLayout::hit_test_divider_cell` compares against —
        // one conversion, called from both paint and hit-test.
        let axis_cell = div.cell_position();
        let cross_start = div.cross_start.round() as u16;
        let cross_len = div.cross_size.round() as u16;
        match div.direction {
            SplitDirection::Horizontal => {
                for dy in 0..cross_len {
                    set_cell(buf, axis_cell, cross_start + dy, '│', fg, bg);
                }
            }
            SplitDirection::Vertical => {
                for dx in 0..cross_len {
                    set_cell(buf, cross_start + dx, axis_cell, '─', fg, bg);
                }
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn two_pane(direction: SplitDirection, ratio: f32) -> SplitTree {
        SplitTree::split(
            direction,
            ratio,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        )
    }

    #[test]
    fn horizontal_paint_and_click_round_trip() {
        // 41 cols so available=40, 0.5*40=20 -> divider at cell x=20.
        let area = Rect::new(0, 0, 41, 10);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Horizontal, 0.5);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let d = &layout.dividers[0];
        let cell = d.cell_position();
        assert_eq!(cell_char(&buf, cell, 0), '│');

        // hit_test_divider_cell at the exact painted cell resolves the
        // same split_index the paint loop used.
        assert_eq!(layout.hit_test_divider_cell(cell, 5), Some(d.split_index));

        // Leaf hit-test resolves panes on either side of the divider.
        assert_eq!(
            layout.hit_test_leaf(Point { x: 1.0, y: 5.0 }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: cell as f32 + 2.0,
                y: 5.0
            }),
            Some(&wid("b"))
        );
    }

    #[test]
    fn vertical_paint_and_click_round_trip() {
        // 21 rows so available=20, 0.5*20=10 -> divider at cell y=10.
        let area = Rect::new(0, 0, 40, 21);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Vertical, 0.5);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let d = &layout.dividers[0];
        let cell = d.cell_position();
        assert_eq!(cell_char(&buf, 0, cell), '─');
        assert_eq!(layout.hit_test_divider_cell(cell, 5), Some(d.split_index));

        assert_eq!(
            layout.hit_test_leaf(Point { x: 5.0, y: 1.0 }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: 5.0,
                y: cell as f32 + 2.0
            }),
            Some(&wid("b"))
        );
    }

    #[test]
    fn nested_tree_paints_all_dividers_and_round_trips() {
        // Split(H, Split(V, a, c), b) inside a 61x21 area.
        let area = Rect::new(0, 0, 61, 21);
        let mut buf = Buffer::empty(area);
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(layout.dividers.len(), 2);

        for div in &layout.dividers {
            let axis_cell = div.cell_position();
            let cross_mid = (div.cross_start + div.cross_size / 2.0).round() as u16;
            let expected = match div.direction {
                SplitDirection::Horizontal => cell_char(&buf, axis_cell, cross_mid),
                SplitDirection::Vertical => cell_char(&buf, cross_mid, axis_cell),
            };
            let want = match div.direction {
                SplitDirection::Horizontal => '│',
                SplitDirection::Vertical => '─',
            };
            assert_eq!(expected, want, "divider {} paint mismatch", div.split_index);

            // hit_test_divider_cell must resolve every painted divider
            // back to its own split_index — the round-trip guarantee.
            let cross_cell = match div.direction {
                SplitDirection::Horizontal => cross_mid,
                SplitDirection::Vertical => cross_mid,
            };
            assert_eq!(
                layout.hit_test_divider_cell(axis_cell, cross_cell),
                Some(div.split_index)
            );
        }

        assert_eq!(layout.leaves.len(), 3);
    }

    #[test]
    fn hit_test_agrees_with_actually_painted_cell_for_fractional_ratio() {
        // A ratio chosen so the divider position is NOT cell-aligned —
        // this is the vimcode #452 regression case: paint must truncate
        // (`as u16`) the same way `SplitTreeDivider::cell_position()`
        // does, not round — otherwise a click on the visually-painted
        // divider misses.
        let area = Rect::new(0, 0, 101, 10); // available = 100
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Horizontal, 0.207); // position = 20.7
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert!((layout.dividers[0].position - 20.7).abs() < 0.01);

        // Find the column the divider glyph actually painted at —
        // don't assume a formula, read the buffer.
        let painted_col = (0..area.width)
            .find(|&x| cell_char(&buf, x, 5) == '│')
            .expect("divider glyph should have painted somewhere");

        // hit_test_divider_cell at the ACTUAL painted column must
        // resolve to this divider — proving paint and hit-test agree
        // on the same float -> cell conversion.
        assert_eq!(
            layout.hit_test_divider_cell(painted_col, 5),
            Some(layout.dividers[0].split_index),
            "painted at column {painted_col} but hit_test_divider_cell disagrees \
             (paint and click used different float->cell conversions)"
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let tree = two_pane(SplitDirection::Horizontal, 0.5);
        let _layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn divider_position_matches_ratio() {
        let area = Rect::new(0, 0, 41, 10);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Horizontal, 0.3);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let cell = layout.dividers[0].cell_position();
        // 41 cols, 1-cell divider -> 40 available. 0.3 * 40 = 12.
        assert_eq!(cell, 12);
        assert_eq!(cell_char(&buf, cell, 0), '│');
        assert_eq!(cell_char(&buf, cell - 1, 0), ' ');
        assert_eq!(cell_char(&buf, cell + 1, 0), ' ');
    }
}
