//! TUI entry point for [`crate::Palette`] — a thin delegate onto the
//! widget's own cell painter, `crate::primitives::palette::cell_paint`,
//! which documents the border chrome, title badge, query caret, item rows
//! and preview pane it paints. This module only adapts a raw ratatui
//! [`Buffer`] into the [`crate::CellSurface`] that painter takes, and the
//! geometry entry point onto
//! `crate::primitives::palette::cell_layout`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::palette::{cell_layout, cell_paint, Palette, PaletteLayout};
use crate::style::Style;
use crate::theme::Theme;

/// Compute the TUI [`Palette`] layout — the shared geometry this module's
/// [`draw_palette`] paints from and `Backend::palette_layout` exposes for
/// hit-testing, ensuring they remain synchronized. Thin delegate onto
/// [`crate::primitives::palette::cell_layout`], which documents the row
/// reservations, the local coordinate frame and the deliberately-omitted
/// scrollbar hit regions.
pub fn tui_palette_layout(area: Rect, palette: &Palette) -> PaletteLayout {
    cell_layout(palette, area.width as f32, area.height as f32)
}

/// Draw a [`Palette`] modal into `area` on `buf`, painting with
/// [`Style::default`].
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph
/// and ASCII fallback. Caller is responsible for sizing / centring
/// the popup within the editor area.
///
/// Callers that need a live, caller-supplied [`Style`] (backdrop
/// opacity included) go through [`draw_palette_with_style`] instead —
/// this free function's signature is part of the crate's public API and
/// stays fixed so downstream consumers calling it directly don't break;
/// `Backend::draw_palette` implementations call the `_with_style`
/// sibling with their own live style.
pub fn draw_palette(
    buf: &mut Buffer,
    area: Rect,
    palette: &Palette,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    draw_palette_with_style(
        buf,
        area,
        palette,
        theme,
        &Style::default(),
        nerd_fonts_enabled,
    );
}

/// Draw a [`Palette`] modal into `area` on `buf` with an explicit
/// [`Style`] — the live-style counterpart to [`draw_palette`], used by
/// `Backend::draw_palette` implementations and by the widget contract's
/// own `Primitive::paint_cells`.
///
/// Only [`Style::backdrop_opacity`] reaches the cell painter; see
/// [`cell_paint::paint`] for what it does with it.
pub(crate) fn draw_palette_with_style(
    buf: &mut Buffer,
    area: Rect,
    palette: &Palette,
    theme: &Theme,
    style: &Style,
    nerd_fonts_enabled: bool,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(
        palette,
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        &mut surface,
        theme,
        style.backdrop_opacity,
        nerd_fonts_enabled,
    );
}
