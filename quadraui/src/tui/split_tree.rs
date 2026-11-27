//! TUI entry points for [`crate::SplitTree`]: thin delegates onto the
//! widget's own cell layout and painter (`crate::primitives::split`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::theme::Theme;

/// The cell layout of `tree` over `area` — [`SplitTree::cell_layout`],
/// the one source of leaf rects for both paint and hit-test.
pub fn tui_split_tree_layout(tree: &SplitTree, area: Rect) -> SplitTreeLayout {
    tree.cell_layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Paint `tree`'s dividers into `area` on `buf` and return its layout.
/// Leaf content is the app's: it paints into each `layout.leaves[i].1`.
pub fn draw_split_tree(
    buf: &mut Buffer,
    area: Rect,
    tree: &SplitTree,
    theme: &Theme,
) -> SplitTreeLayout {
    let layout = tui_split_tree_layout(tree, area);
    crate::primitives::split::paint_tree_cells(
        &layout,
        &mut super::BufferCellSurface::new(buf),
        theme,
    );
    layout
}
