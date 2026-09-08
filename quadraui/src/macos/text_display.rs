//! macOS layout helper for [`crate::TextDisplay`].
//!
//! Painting moved to the shared [`crate::primitives::text_display::paint`]
//! (#810, NativeSurface Phase 2c) — see that fn's doc for the divergence
//! (Windows's bold-span support, dropped since `NativeSurface` has no
//! weight parameter) resolved while unifying
//! `gtk::text_display::draw_text_display`,
//! `macos::text_display::draw_text_display` and
//! `win::text_display::draw_text_display` into one implementation. This
//! module now only carries [`mac_text_display_layout`], the pixel-unit
//! layout query used for both hit-testing
//! (`MacBackend::text_display_layout`) and painting
//! (`MacBackend::draw_text_display`, via `Backend::text_display_layout`).
//!
//! The deleted `macos::text_display::draw_text_display` free function
//! had zero call sites in `coord-tui`/`vimcode` (`grep -rn
//! draw_text_display ~/src/coord-tui/src ~/src/vimcode/src` — both only
//! call `Backend::draw_text_display`, whose signature is unchanged), so
//! it needed no deprecation shim (CLAUDE.md rule 1).

use crate::event::Rect as QRect;
use crate::primitives::text_display::{TextDisplay, TextDisplayLayout, TextDisplayLineMeasure};

/// Scrollbar gutter width in points. Matches the GTK rasteriser's
/// 12-pt gutter so the body width — and therefore the resolved
/// layout — stays parity-equivalent across the two pixel backends.
const SCROLLBAR_GUTTER_PT: f32 = 12.0;

/// Minimum scrollbar thumb length in points.
const SCROLLBAR_MIN_THUMB_PT: f32 = 8.0;

/// Compute the layout the macOS rasteriser would produce for
/// `display` at `rect` with the supplied `line_height` (font's
/// typographic line height in points). Hosts call this to drive
/// hit-testing for scrollbar drag interaction without re-deriving
/// metrics.
///
/// The returned layout's coordinates are **body-local** (y=0 at the
/// top of the body region). Title-bar painting consumes one
/// `line_height` strip above; the body height passed to the
/// primitive shrinks by that strip when `title` is present. This
/// matches the GTK helper's contract.
///
/// `char_width` is the backend's approximate average character width
/// (`Backend::char_width`) — see quadraui#905: this pure function has no
/// live `CTFont`/`CGContext` to measure real glyph widths with, so it
/// approximates the wrap column budget from pixels the same way the
/// shared `paint` (`MacBackend::draw_text_display`) does, so a click
/// always resolves against the same row boundaries that were actually
/// painted.
pub fn mac_text_display_layout(
    display: &TextDisplay,
    rect: QRect,
    line_height: f64,
    char_width: f64,
) -> TextDisplayLayout {
    let body_h = if display.title.is_some() {
        (rect.height as f64 - line_height).max(0.0)
    } else {
        rect.height as f64
    };
    if body_h <= 0.0 {
        return display.layout(0.0, 0.0, |_| {
            TextDisplayLineMeasure::new(line_height as f32)
        });
    }
    let body_width_px = if display.show_scrollbar {
        (rect.width - SCROLLBAR_GUTTER_PT).max(0.0)
    } else {
        rect.width
    };
    let col_budget = crate::primitives::text_display::px_to_cols(body_width_px, char_width as f32);
    let measure = |i: usize| {
        let rows = crate::primitives::text_display::wrap_row_count(&display.lines[i], col_budget);
        TextDisplayLineMeasure::new(rows as f32 * line_height as f32)
    };
    if display.show_scrollbar {
        display.layout_with_scrollbar(
            rect.width,
            body_h as f32,
            SCROLLBAR_GUTTER_PT,
            SCROLLBAR_MIN_THUMB_PT,
            measure,
        )
    } else {
        display.layout(rect.width, body_h as f32, measure)
    }
}
