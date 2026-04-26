//! Public TUI (ratatui) rasterisers for `quadraui` primitives.
//!
//! Enabled via the `tui` Cargo feature. Apps depend on `quadraui` with
//! `features = ["tui"]` and call these `draw_*` functions to paint
//! primitives into a [`ratatui::buffer::Buffer`].
//!
//! Per D6 (see `docs/BACKEND_TRAIT_PROPOSAL.md` §9): primitives own
//! layout, backends rasterise. Each rasteriser takes a pre-computed
//! `*Layout` from the primitive's `.layout()` method along with the
//! primitive itself and a [`crate::Theme`] for default colours.
//!
//! This module is the destination of issue #223 — the per-primitive
//! rasterisers are being lifted out of vimcode (`src/tui_main/quadraui_tui.rs`)
//! and kubeui (private `draw_status_bar` in `kubeui/src/main.rs`) one
//! primitive at a time. StatusBar is the pilot.

use ratatui::buffer::Buffer;
use ratatui::style::Color as RatatuiColor;

use crate::types::Color;

mod status_bar;

pub use status_bar::draw_status_bar;

/// Convert a `quadraui::Color` to the ratatui palette colour used by
/// `set_cell`. Public so apps adopting these rasterisers can mirror the
/// conversion when they paint extra cells alongside (e.g. their own
/// borders or background fills).
pub fn ratatui_color(c: Color) -> RatatuiColor {
    RatatuiColor::Rgb(c.r, c.g, c.b)
}

/// Set a single buffer cell, clearing modifier and underline_color so the
/// rasterisers don't leave stale style bits from prior frames. Mirrors
/// `vimcode::tui_main::set_cell`.
fn set_cell(buf: &mut Buffer, x: u16, y: u16, ch: char, fg: RatatuiColor, bg: RatatuiColor) {
    let area = buf.area;
    if x < area.x + area.width && y < area.y + area.height {
        let cell = &mut buf[(x, y)];
        cell.set_char(ch).set_fg(fg).set_bg(bg);
        cell.modifier = ratatui::style::Modifier::empty();
        cell.underline_color = RatatuiColor::Reset;
    }
}
