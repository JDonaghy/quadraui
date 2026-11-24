//! Direct2D / DirectWrite layout helper for [`crate::TextDisplay`]
//!
//! This module carries [`win_text_display_layout`], the DIP-unit layout
//! query used for both hit-testing (`WinBackend::text_display_layout`) and
//! painting (`WinBackend::draw_text_display`, via
//! `Backend::text_display_layout`). Painting uses the shared
//! [`crate::primitives::text_display::paint`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod text_display;` and `backend.rs`'s
//! module docs.

use crate::event::Rect;
use crate::primitives::text_display::{TextDisplay, TextDisplayLayout};

/// Compute the layout the Win-GUI rasteriser produces for `display` at
/// `rect` with the supplied `line_height`. Hosts call this to drive
/// hit-testing for scrollbar drag interaction without re-deriving metrics.
///
/// The returned layout's coordinates are **body-local** (y=0 at the top
/// of the body region). Title-bar painting consumes one `line_height`
/// strip above; the body height passed to the primitive shrinks by that
/// strip when `title` is present — same contract as
/// `gtk_text_display_layout` / `mac_text_display_layout`.
///
/// `char_width` is the backend's approximate average character width
/// (`Backend::char_width`). This pure function has no live
/// `IDWriteTextLayout` to measure real glyph widths with, so it
/// approximates the wrap column budget from pixels the same way the
/// shared `paint` (`WinBackend::draw_text_display`) does, ensuring a
/// click resolves against the same row boundaries that were painted.
pub fn win_text_display_layout(
    display: &TextDisplay,
    rect: Rect,
    line_height: f32,
    char_width: f32,
) -> TextDisplayLayout {
    crate::primitives::text_display::pixel_layout(display, rect, line_height, char_width).layout
}
