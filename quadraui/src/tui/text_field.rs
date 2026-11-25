//! TUI entry points for [`crate::TextField`] — thin delegates onto the
//! widget's own cell painter,
//! `crate::primitives::text_field::cell_paint`, which documents the
//! bracketed row, the icon/clear columns and the block caret. This
//! module only adapts a raw ratatui [`Buffer`] into the
//! [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::text_field::{cell_layout, cell_paint, TextField, TextFieldLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Compute the TUI text-field layout using cell metrics.
///
/// Delegates to [`crate::primitives::text_field::cell_layout`], the one
/// cell-units layout every cell-grid caller shares, so paint and
/// hit-test agree on the icon, text and clear columns.
pub fn tui_text_field_layout(tf: &TextField, rect: Rect) -> TextFieldLayout {
    cell_layout(
        tf,
        crate::event::Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
    )
}

/// Draw a [`TextField`] into `rect` on `buf` and return the layout its
/// glyphs were placed with, so callers never re-derive it and risk it
/// drifting from what was painted.
///
/// Also what [`crate::primitive::Primitive::paint_cells`] reaches the
/// cell painter through: the `Buffer` → [`crate::CellSurface`] adapter is
/// private to this module's parent.
pub fn draw_text_field(
    buf: &mut Buffer,
    rect: Rect,
    tf: &TextField,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) -> TextFieldLayout {
    let layout = tui_text_field_layout(tf, rect);
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(tf, &layout, &mut surface, theme, nerd_fonts_enabled);
    layout
}

/// The ratatui colour a `crate::tui` caller already holds, recovered as
/// the quadraui [`Color`] a [`crate::CellSurface`] verb takes.
///
/// Total for every colour this module's callers pass: each one comes from
/// `crate::tui`'s own `qc`, which always lands
/// [`RatatuiColor::Rgb`]. Anything else has no literal RGB to recover and
/// maps to "the terminal's own default" — the same meaning
/// [`crate::cell_surface::CellStyle`]'s `None` carries.
fn from_ratatui(c: RatatuiColor) -> Option<Color> {
    match c {
        RatatuiColor::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
        _ => None,
    }
}

/// Paint the inside-the-brackets text content shared by `TextField`'s
/// own cell painter and the `Form` cell painter's
/// `FieldKind::TextInput` row — a thin delegate onto
/// [`crate::primitives::text_field::cell_paint::put_text_row`], which
/// documents the selection highlight and caret placement.
///
/// Kept here, taking the ratatui colours `draw_form` already holds, so
/// that caller is unchanged by the painter's move into the widget
/// module.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_text_row_cells(
    buf: &mut Buffer,
    y: u16,
    text_col: u16,
    right_bound: u16,
    max_cols: usize,
    skip_chars: usize,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    selection_anchor: Option<usize>,
    field_fg: RatatuiColor,
    dim_fg: RatatuiColor,
    row_bg: RatatuiColor,
) {
    let fallback = Color::rgb(0, 0, 0);
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::put_text_row(
        &mut surface,
        y,
        text_col,
        right_bound,
        max_cols,
        skip_chars,
        value,
        placeholder,
        cursor,
        selection_anchor,
        from_ratatui(field_fg).unwrap_or(fallback),
        from_ratatui(dim_fg).unwrap_or(fallback),
        from_ratatui(row_bg).unwrap_or(fallback),
    );
}
