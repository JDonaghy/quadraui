//! macOS layout helper for [`crate::TextDisplay`].
//!
//! Painting uses the shared [`crate::primitives::text_display::paint`].
//! This module carries [`mac_text_display_layout`], the pixel-unit
//! layout query used for both hit-testing
//! (`MacBackend::text_display_layout`) and painting
//! (`MacBackend::draw_text_display`, via `Backend::text_display_layout`).

use crate::event::Rect as QRect;
use crate::primitives::text_display::{TextDisplay, TextDisplayLayout};

/// Compute the layout the macOS rasteriser produces for
/// `display` at `rect` with the supplied `line_height` (font's
/// typographic line height in points). Hosts call this to drive
/// hit-testing for scrollbar drag interaction.
///
/// The returned layout's coordinates are **body-local** (y=0 at the
/// top of the body region). Title-bar painting consumes one
/// `line_height` strip above; the body height passed to the
/// primitive shrinks by that strip when `title` is present. This
/// matches the GTK helper's contract.
///
/// `char_width` is the backend's approximate average character width
/// (`Backend::char_width`). This pure function has no live
/// `CTFont`/`CGContext` to measure real glyph widths with, so it
/// approximates the wrap column budget from pixels the same way
/// `MacBackend::draw_text_display` does, so a click always resolves
/// against the same row boundaries that were actually painted.
pub fn mac_text_display_layout(
    display: &TextDisplay,
    rect: QRect,
    line_height: f64,
    char_width: f64,
) -> TextDisplayLayout {
    crate::primitives::text_display::pixel_layout(
        display,
        rect,
        line_height as f32,
        char_width as f32,
    )
    .layout
}
