//! TUI entry points for [`crate::Split`]: thin delegates onto the
//! widget's own cell layout and painter (`crate::primitives::split`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::primitives::split::{Split, SplitLayout};
use crate::theme::Theme;

/// The cell layout of `split` over `area` — [`Split::cell_layout`].
pub fn tui_split_layout(split: &Split, area: Rect) -> SplitLayout {
    split.cell_layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Paint `split`'s divider into `area` on `buf` and return its layout.
/// The panes are the app's: it paints into `layout.first_bounds` /
/// `layout.second_bounds`.
pub fn draw_split(buf: &mut Buffer, area: Rect, split: &Split, theme: &Theme) -> SplitLayout {
    let layout = tui_split_layout(split, area);
    crate::primitives::split::paint_tree_cells(
        &split.as_tree().cell_layout(layout.bounds),
        &mut super::BufferCellSurface::new(buf),
        theme,
    );
    layout
}
