//! TUI entry points for the [`Editor`] primitive — thin delegates onto
//! the widget's own cell painter, `crate::primitives::editor::paint_cells`,
//! which documents every paint category. This module only adapts a raw
//! ratatui [`Buffer`] into the [`crate::CellSurface`] that painter takes,
//! and reads the caret the painter reported back out of it.
//!
//! The per-window status line is not painted here: a host reserves a row
//! below `area` for it.

use crate::primitives::editor::{paint_cells, Editor, EditorPaintOptions};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::BufferCellSurface;

/// Output of [`draw_editor`] — carries side-effects the host needs
/// to apply *after* the buffer paint completes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorPaintResult {
    /// Screen-space `(x, y)` for `Frame::set_cursor_position` when the
    /// cursor shape is `Bar` or `Underline`. `None` when the cursor is
    /// missing or in `Block` mode (Block is painted directly into the
    /// buffer via fg/bg inversion).
    pub cursor_position: Option<(u16, u16)>,
}

/// Paint an [`Editor`] into `buf`.
///
/// Equivalent to [`draw_editor_with_reserve`] with `right_reserve_w: 0`.
pub fn draw_editor(
    buf: &mut Buffer,
    area: Rect,
    editor: &Editor,
    theme: &Theme,
) -> EditorPaintResult {
    draw_editor_with_reserve(buf, area, editor, theme, 0)
}

/// [`draw_editor`], plus a strip `right_reserve_w` cells wide left
/// unpainted between the buffer text and the vertical scrollbar — the
/// paint-side half of [`Editor::layout_with_options_and_reserve`]'s
/// `right_reserve_w`. The vertical scrollbar keeps hugging
/// `area.x + area.width - 1`; everything else stops short of the strip,
/// so a host can paint a minimap there without it being overwritten.
/// `right_reserve_w: 0` reproduces [`draw_editor`] exactly.
pub fn draw_editor_with_reserve(
    buf: &mut Buffer,
    area: Rect,
    editor: &Editor,
    theme: &Theme,
    right_reserve_w: u16,
) -> EditorPaintResult {
    let layout = editor.layout_with_options_and_reserve(
        crate::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        1.0,
        1.0,
        EditorPaintOptions::default(),
        None,
        right_reserve_w as f32,
    );
    let mut surface = BufferCellSurface::new(buf);
    paint_cells::paint(editor, &layout, right_reserve_w, &mut surface, theme);
    EditorPaintResult {
        cursor_position: surface.cursor().map(|(x, y, _)| (x, y)),
    }
}
