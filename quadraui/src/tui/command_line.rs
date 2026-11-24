//! TUI entry points for [`crate::primitives::command_line::CommandLine`]
//! — thin delegates onto the widget's own cell painter,
//! `crate::primitives::command_line::cell_paint`, which documents the
//! bar fill, the right-align shift, the selection highlight and the
//! block cursor. This module only adapts a raw ratatui [`Buffer`] into
//! the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::command_line::{cell_layout, cell_paint, CommandLine, CommandLineLayout};
use crate::theme::Theme;

/// Compute [`CommandLineLayout`] for `cmd` in `area` — one cell per
/// character column.
///
/// Delegates to [`crate::primitives::command_line::cell_layout`], the
/// one cell-units layout every cell-grid caller shares, so paint and
/// hit-test agree on every column.
pub fn tui_command_line_layout(cmd: &CommandLine, area: Rect) -> CommandLineLayout {
    cell_layout(
        cmd,
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
    )
}

/// Paint `cmd` into `buf` at `area` and return the
/// [`CommandLineLayout`] used to place its glyphs — the same value
/// `tui_command_line_layout` would compute, handed back so callers never
/// have to re-derive it and risk it drifting from what was actually
/// painted.
pub fn draw_command_line(
    buf: &mut Buffer,
    area: Rect,
    cmd: &CommandLine,
    theme: &Theme,
) -> CommandLineLayout {
    draw_command_line_selection(buf, area, cmd, theme, None)
}

/// Paint `cmd` exactly like [`draw_command_line`], additionally
/// highlighting the cell columns [`CommandLineLayout::selection_bounds`]
/// resolves for `selection` with `theme.selection` as the cell
/// background.
///
/// Both entry points reach the same painter, which takes the selection
/// as a parameter — this is the `Backend::draw_command_line_selection`
/// signature's own shape, not a second rasteriser. Also what
/// [`crate::primitive::Primitive::paint_cells`] reaches the painter
/// through, since the `Buffer` → [`crate::CellSurface`] adapter is
/// private to this module's parent.
pub fn draw_command_line_selection(
    buf: &mut Buffer,
    area: Rect,
    cmd: &CommandLine,
    theme: &Theme,
    selection: Option<(usize, usize)>,
) -> CommandLineLayout {
    let layout = tui_command_line_layout(cmd, area);
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(cmd, &layout, &mut surface, theme, selection);
    layout
}
