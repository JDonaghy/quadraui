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
use crate::primitives::text_display::{TextDisplay, TextDisplayLayout, TextDisplayLineMeasure};

/// Scrollbar gutter width in DIPs. Matches the GTK/macOS rasterisers'
/// 12-DIP gutter so the body width — and therefore the resolved layout
/// — stays parity-equivalent across every pixel backend.
const SCROLLBAR_GUTTER_DIP: f32 = 12.0;

/// Minimum scrollbar thumb length in DIPs.
const SCROLLBAR_MIN_THUMB_DIP: f32 = 8.0;

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
    let body_h = if display.title.is_some() {
        (rect.height - line_height).max(0.0)
    } else {
        rect.height
    };
    if body_h <= 0.0 {
        return display.layout(0.0, 0.0, |_| TextDisplayLineMeasure::new(line_height));
    }
    let body_width_px = if display.show_scrollbar {
        (rect.width - SCROLLBAR_GUTTER_DIP).max(0.0)
    } else {
        rect.width
    };
    let col_budget = crate::primitives::text_display::px_to_cols(body_width_px, char_width);
    let measure = |i: usize| {
        let rows = crate::primitives::text_display::wrap_row_count(&display.lines[i], col_budget);
        TextDisplayLineMeasure::new(rows as f32 * line_height)
    };
    if display.show_scrollbar {
        display.layout_with_scrollbar(
            rect.width,
            body_h,
            SCROLLBAR_GUTTER_DIP,
            SCROLLBAR_MIN_THUMB_DIP,
            measure,
        )
    } else {
        display.layout(rect.width, body_h, measure)
    }
}
