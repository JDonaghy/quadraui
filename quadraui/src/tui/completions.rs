//! TUI entry point for [`crate::Completions`] — a thin delegate onto the
//! widget's own cell painter,
//! `crate::primitives::completions::cell_paint`, which documents the row
//! chrome and content rules. This module only adapts a raw ratatui
//! [`Buffer`] into the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::completions::{cell_paint, Completions, CompletionsLayout};
use crate::theme::Theme;

/// Draw a [`Completions`] popup at its resolved layout. `layout` must be
/// the same [`CompletionsLayout`] the host hit-tests against, so paint
/// and click-routing can never disagree.
pub fn draw_completions(
    buf: &mut Buffer,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(completions, layout, &mut surface, theme);
}
