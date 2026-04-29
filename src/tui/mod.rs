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
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use crate::types::{Color, Decoration, StyledText};

pub mod backend;
mod context_menu;
mod dialog;
pub mod events;
mod form;
mod list;
mod palette;
mod run;
pub mod services;
mod status_bar;
mod tab_bar;
mod text_display;
mod tooltip;
mod tree;

pub use backend::TuiBackend;
pub use context_menu::draw_context_menu;
pub use dialog::draw_dialog;
pub use form::draw_form;
pub use list::draw_list;
pub use palette::draw_palette;
pub use run::run;
pub use services::TuiPlatformServices;
pub use status_bar::draw_status_bar;
pub use tab_bar::{draw_tab_bar, TAB_CLOSE_CHAR, TAB_CLOSE_COLS};
pub use text_display::draw_text_display;
pub use tooltip::draw_tooltip;
pub use tree::draw_tree;

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
        cell.modifier = Modifier::empty();
        cell.underline_color = RatatuiColor::Reset;
    }
}

/// Set a buffer cell with a 2-cell-wide character (e.g. Nerd Font glyph),
/// resetting the trailing cell so ratatui's diff algorithm doesn't emit a
/// stray character on top of the wide glyph's second column. Mirrors
/// `vimcode::tui_main::set_cell_wide`.
fn set_cell_wide(buf: &mut Buffer, x: u16, y: u16, ch: char, fg: RatatuiColor, bg: RatatuiColor) {
    let area = buf.area;
    if x < area.x + area.width && y < area.y + area.height {
        let mut s = String::with_capacity(4);
        s.push(ch);
        let cell = &mut buf[(x, y)];
        cell.set_symbol(&s).set_fg(fg).set_bg(bg);
        cell.modifier = Modifier::empty();
        cell.underline_color = RatatuiColor::Reset;
        if x + 1 < area.x + area.width {
            // Wide-char continuation cell: empty symbol tells ratatui this
            // half is the trailing column of a double-width glyph.
            let cont = &mut buf[(x + 1, y)];
            cont.set_symbol("").set_fg(fg).set_bg(bg);
            cont.modifier = Modifier::empty();
            cont.underline_color = RatatuiColor::Reset;
        }
    }
}

/// Convert a `quadraui::Color` to a ratatui palette colour, with `qc` as
/// the short name internal modules use (mirrors vimcode's tui rasteriser
/// helper of the same name).
fn qc(c: Color) -> RatatuiColor {
    ratatui_color(c)
}

/// Draw a [`StyledText`] onto `buf` starting at `(area.x + start_col,
/// y)`, returning the column past the last drawn character. Honors the
/// caller's `decoration` as a final colour override (e.g. `Muted` dims
/// every span that didn't already specify its own `fg`). Used by the
/// list / form / palette rasterisers.
#[allow(clippy::too_many_arguments)]
fn draw_styled_text(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    start_col: usize,
    text: &StyledText,
    default_fg: RatatuiColor,
    bg: RatatuiColor,
    decoration: Decoration,
    dim_fg: RatatuiColor,
) -> usize {
    let mut col = start_col;
    for span in &text.spans {
        let span_fg = if let Some(c) = span.fg {
            qc(c)
        } else if matches!(decoration, Decoration::Muted) {
            dim_fg
        } else {
            default_fg
        };
        let span_bg = span.bg.map(qc).unwrap_or(bg);
        for ch in span.text.chars() {
            if col >= area.width as usize {
                return col;
            }
            set_cell(buf, area.x + col as u16, y, ch, span_fg, span_bg);
            col += 1;
        }
    }
    col
}

/// Set a buffer cell with explicit modifier + optional underline colour.
/// Used by [`tab_bar::draw_tab_bar`] for the active-tab accent underline.
#[allow(clippy::too_many_arguments)]
fn set_cell_styled(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    ch: char,
    fg: RatatuiColor,
    bg: RatatuiColor,
    modifier: Modifier,
    underline_color: Option<RatatuiColor>,
) {
    let area = buf.area;
    if x < area.x + area.width && y < area.y + area.height {
        let cell = &mut buf[(x, y)];
        cell.set_char(ch).set_fg(fg).set_bg(bg);
        cell.modifier = modifier;
        cell.underline_color = underline_color.unwrap_or(RatatuiColor::Reset);
    }
}
