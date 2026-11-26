//! TUI entry points for [`Minimap`] — thin delegates onto the widget's
//! own braille cell painter, `crate::primitives::minimap::paint_cells`,
//! which documents the dot and colour rules. These functions only adapt a
//! raw ratatui [`Buffer`] and [`Rect`] to it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::BufferCellSurface;
use crate::primitives::minimap::{
    paint_cells, Minimap, MinimapLayout, MinimapSizing, BRAILLE_COLS_PER_CELL,
    BRAILLE_LINES_PER_ROW,
};
use crate::theme::Theme;

pub use crate::primitives::minimap::resolve_cols_per_cell;

/// Buffer lines packed into one terminal row's braille dots.
pub const LINES_PER_ROW: usize = BRAILLE_LINES_PER_ROW;
/// The narrowest buffer-columns-per-cell scale: one buffer column per dot.
/// A host aggregating its own [`crate::MinimapSpan`]s builds its
/// [`crate::MinimapGrid::cols_per_cell`] from [`MinimapLayout::cols_per_cell`],
/// not from this constant, since the painted scale adapts to the buffer.
pub const COLS_PER_CELL: usize = BRAILLE_COLS_PER_CELL;

fn to_rect(area: Rect) -> crate::Rect {
    crate::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}

/// The cell layout of `minimap` in `area` without painting, at one cell
/// row per minimap row ([`MinimapSizing::FixedPitch`]`(1.0)`).
pub fn tui_minimap_layout(minimap: &Minimap, area: Rect) -> MinimapLayout {
    tui_minimap_layout_with_sizing(minimap, area, MinimapSizing::FixedPitch(1.0))
}

/// [`tui_minimap_layout`] under an explicit row-pitch strategy — see
/// [`Minimap::cell_layout`].
pub fn tui_minimap_layout_with_sizing(
    minimap: &Minimap,
    area: Rect,
    sizing: MinimapSizing,
) -> MinimapLayout {
    minimap.cell_layout(to_rect(area), sizing)
}

/// Draw `minimap` into `area` on `buf` at [`resolve_cols_per_cell`]'s
/// scale. Returns the layout for host click dispatch
/// (`layout.hit_test(x, y)`); its `cols_per_cell` is the scale painted.
pub fn draw_minimap(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let cols_per_cell = resolve_cols_per_cell(minimap, area.width as usize);
    draw_minimap_with_scale(buf, area, minimap, theme, cols_per_cell)
}

/// [`draw_minimap`] at an explicit horizontal scale, clamped to at least
/// `1`. A host aggregating its own spans must build its
/// [`crate::MinimapGrid`] at the same scale.
pub fn draw_minimap_with_scale(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
    cols_per_cell: usize,
) -> MinimapLayout {
    draw_minimap_with_sizing(
        buf,
        area,
        minimap,
        theme,
        cols_per_cell,
        MinimapSizing::FixedPitch(1.0),
    )
}

/// [`draw_minimap_with_scale`] under an explicit row-pitch strategy; under
/// [`MinimapSizing::RepeatToFill`] each row repeats across every cell row
/// its pitch spans.
pub fn draw_minimap_with_sizing(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
    cols_per_cell: usize,
    sizing: MinimapSizing,
) -> MinimapLayout {
    let mut layout = tui_minimap_layout_with_sizing(minimap, area, sizing);
    layout.cols_per_cell = cols_per_cell.max(1);
    paint_cells::paint(minimap, &layout, &mut BufferCellSurface::new(buf), theme);
    layout
}
