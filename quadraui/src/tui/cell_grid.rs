//! TUI entry points for [`crate::CellGrid`]: the cell layout and the
//! cell painter both live with the widget (`primitives::cell_grid`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::primitives::cell_grid::{paint_cells, CellGrid, CellGridLayout};
use crate::theme::Theme;

fn quad(area: Rect) -> crate::Rect {
    crate::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}

/// Resolve the layout [`draw_cell_grid`] paints `grid` at, without
/// touching a buffer. Coordinate frame: **LOCAL** to `area`.
pub fn tui_cell_grid_layout(area: Rect, grid: &CellGrid) -> CellGridLayout {
    paint_cells::layout(grid, quad(area))
}

/// Draw `grid` into `area` and return its layout for hit-testing.
/// Coordinate frame: **LOCAL** — `hit_test` on the returned layout takes
/// `area`-relative coordinates, matching [`tui_cell_grid_layout`].
pub fn draw_cell_grid(
    buf: &mut Buffer,
    area: Rect,
    grid: &CellGrid,
    theme: &Theme,
) -> CellGridLayout {
    let layout = tui_cell_grid_layout(area, grid);
    paint_cells::paint(
        grid,
        quad(area),
        &layout,
        &mut super::BufferCellSurface::new(buf),
        theme,
    );
    layout
}
