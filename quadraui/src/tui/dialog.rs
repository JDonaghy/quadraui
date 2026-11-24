//! TUI entry points for [`crate::Dialog`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::dialog::cell_paint`,
//! which documents the border chrome, table/input slots and button
//! rendering. This module only adapts a raw ratatui [`Buffer`] into the
//! [`crate::CellSurface`] that painter takes, and supplies the
//! toolbar-item measurer the cell-grid layout needs.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::dialog::{cell_layout, cell_paint, Dialog, DialogLayout};
use crate::theme::Theme;

/// Compute the TUI dialog layout using cell metrics.
///
/// Delegates to [`crate::primitives::dialog::cell_layout`], the one
/// cell-units layout every cell-grid caller shares: the box's own cell
/// measurements come from `cell_measure`, and a body toolbar is measured
/// by the same `cell_item_width` the toolbar rasteriser uses, so toolbar
/// button positions agree between paint and hit-test.
///
/// This function is TUI's own internal default measurer for dialogs painted
/// without a caller-supplied `DialogMeasure`. `Dialog` is an overlay-with-caller-anchor
/// primitive (like `Tooltip`, `ContextMenu`, `Completions`): callers build their own
/// `DialogMeasure` from portable `Backend::line_height()` and call
/// `dialog.layout(...)` directly. See `quadraui/docs/decisions/DECISIONS.md` D-007.
pub fn tui_dialog_layout(dialog: &Dialog, viewport: crate::event::Rect) -> DialogLayout {
    cell_layout(dialog, viewport)
}

/// Draw a [`Dialog`] at its resolved layout.
///
/// Equivalent to [`draw_dialog_with_nerd_fonts`] with `nerd_fonts_enabled:
/// false` — a separate function (rather than a grown argument list on
/// this one) so a caller of this re-exported `pub fn` with the original
/// 4-argument signature keeps compiling untouched. See `CLAUDE.md`'s
/// *Downstream consumers* rule 2.
pub fn draw_dialog(buf: &mut Buffer, dialog: &Dialog, layout: &DialogLayout, theme: &Theme) {
    draw_dialog_with_nerd_fonts(buf, dialog, layout, theme, false);
}

/// [`draw_dialog`], plus `nerd_fonts_enabled`: resolves the Nerd-Font half
/// of a [`crate::ButtonSpec::icon`] on a [`crate::DialogButton`] that
/// carries one — see [`draw_dialog`] for a caller that doesn't need this.
pub fn draw_dialog_with_nerd_fonts(
    buf: &mut Buffer,
    dialog: &Dialog,
    layout: &DialogLayout,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(dialog, layout, &mut surface, theme, nerd_fonts_enabled);
}
