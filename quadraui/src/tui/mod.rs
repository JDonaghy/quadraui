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

mod activity_bar;
pub mod backend;
mod board;
mod braille;
mod chart;
mod command_center;
mod command_line;
mod completions;
mod context_menu;
mod data_table;
mod dialog;
mod diff_view;
mod drop_overlay;
mod editor;
pub mod events;
mod find_replace;
mod form;
mod image;
mod list;
mod menu_bar;
mod message_list;
mod minimap;
mod multi_section_view;
mod palette;
mod panel;
mod pipeline_view;
mod progress;
mod rich_text_popup;
mod run;
mod scrollbar;
pub mod services;
pub mod shell_runner;
mod sidebar_panel;
mod spinner;
mod split;
mod split_tree;
mod status_bar;
mod tab_bar;
mod terminal;
pub mod testing;
pub mod text;
// vt100/ANSI-byte-stream conformance observer (quadraui#555) — needs
// `vt100` itself, which only `terminal` pulls in (`terminal = ["dep:vt100",
// ...]` in Cargo.toml), so this is gated on both features, same as
// `tests/tui_pty_smoke.rs`.
mod text_display;
mod text_input;
mod toast;
mod toolbar;
mod tooltip;
mod tree;
#[cfg(feature = "terminal")]
pub mod vt_testing;

pub use activity_bar::{draw_activity_bar, draw_activity_bar_with_style};
pub use backend::TuiBackend;
pub use board::{draw_board, tui_board_layout};
pub use chart::{draw_chart, tui_chart_layout};
pub use command_center::{draw_command_center, tui_command_center_layout};
pub use completions::draw_completions;
pub use context_menu::{draw_context_menu, draw_context_menu_with_submenus};
pub use data_table::{data_table_layout, draw_data_table};
pub use dialog::{draw_dialog, tui_dialog_layout};
pub use diff_view::draw_diff_view;
pub use drop_overlay::draw_drop_overlay;
pub use editor::{draw_editor, EditorPaintResult};
pub use find_replace::draw_find_replace;
pub use form::{draw_form, draw_settings_chrome, tui_form_layout};
pub use image::draw_image;
pub use list::draw_list;
pub use menu_bar::{draw_menu_bar, tui_menu_bar_layout};
pub use message_list::draw_message_list;
pub use minimap::{draw_minimap, tui_minimap_layout};
pub use multi_section_view::{draw_multi_section_view, tui_msv_layout};
pub use palette::draw_palette;
pub use panel::{draw_panel, tui_panel_layout};
pub use pipeline_view::{draw_pipeline_view, tui_pipeline_view_layout};
pub use progress::{draw_progress, tui_progress_layout};
pub use rich_text_popup::draw_rich_text_popup;
pub use run::run;
pub use scrollbar::draw_scrollbar;
pub use services::TuiPlatformServices;
pub use sidebar_panel::{draw_sidebar_panel, tui_sidebar_panel_layout};
pub use spinner::{draw_spinner, tui_spinner_layout};
pub use split::{draw_split, tui_split_layout};
pub use split_tree::{draw_split_tree, tui_split_tree_layout};
pub use status_bar::draw_status_bar;
pub use tab_bar::{
    draw_tab_bar, draw_tab_bar_icons, draw_tab_bar_icons_with_chrome, draw_tab_bar_with_chrome,
    TAB_CLOSE_CHAR, TAB_CLOSE_COLS,
};
pub use terminal::{draw_terminal, draw_terminal_divider};
pub use text::{char_cell_width, display_width, truncate_to_width, truncate_to_width_ellipsis};
pub use text_display::{draw_text_display, tui_text_display_layout};
pub use text_input::{draw_text_input, tui_text_input_layout};
pub use toast::{draw_toast_stack, tui_toast_stack_layout};
pub use toolbar::{draw_toolbar, tui_toolbar_layout};
pub use tooltip::{
    draw_tooltip, draw_tooltip_with_chrome, painted_bounds as tooltip_painted_bounds,
};
pub use tree::{draw_tree, tui_tree_layout};

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
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
        // ratatui ≥ 0.30 debug_asserts when a single-byte ASCII control character is
        // stored in a cell symbol (they are not renderable glyphs). Replace with space.
        let ch = if ch.is_ascii_control() { ' ' } else { ch };
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
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
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
///
/// Strides by each character's [`char_cell_width`] (via [`set_cell_wide`]
/// for double-width glyphs) rather than one buffer column per `char`, so
/// the painted extent agrees with [`StyledText::visible_width`] — see
/// #471, where the two fell out of sync after `visible_width` became
/// display-width-aware but this loop stayed char-count-strided.
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
            let w = char_cell_width(ch) as usize;
            if w == 2 && col + 1 < area.width as usize {
                set_cell_wide(buf, area.x + col as u16, y, ch, span_fg, span_bg);
            } else {
                set_cell(buf, area.x + col as u16, y, ch, span_fg, span_bg);
            }
            col += w;
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
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
        let ch = if ch.is_ascii_control() { ' ' } else { ch };
        let cell = &mut buf[(x, y)];
        cell.set_char(ch).set_fg(fg).set_bg(bg);
        cell.modifier = modifier;
        cell.underline_color = underline_color.unwrap_or(RatatuiColor::Reset);
    }
}

/// [`set_cell_wide`] + [`set_cell_styled`] combined: a 2-cell-wide
/// character carrying an explicit modifier + optional underline colour.
/// Used by [`tab_bar::draw_tab_bar`] so a double-width glyph in the
/// filename portion of an active tab's label still gets the accent
/// underline / preview-italic modifier that [`set_cell_styled`] applies
/// to narrow glyphs (#554).
#[allow(clippy::too_many_arguments)]
fn set_cell_wide_styled(
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
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
        let mut s = String::with_capacity(4);
        s.push(ch);
        let cell = &mut buf[(x, y)];
        cell.set_symbol(&s).set_fg(fg).set_bg(bg);
        cell.modifier = modifier;
        cell.underline_color = underline_color.unwrap_or(RatatuiColor::Reset);
        if x + 1 < area.x + area.width {
            // Wide-char continuation cell: empty symbol tells ratatui this
            // half is the trailing column of a double-width glyph.
            let cont = &mut buf[(x + 1, y)];
            cont.set_symbol("").set_fg(fg).set_bg(bg);
            cont.modifier = modifier;
            cont.underline_color = underline_color.unwrap_or(RatatuiColor::Reset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression tests for #508: `set_cell` and its siblings guarded the
    // right/bottom edge of `buf.area` but not the left/top edge. Every TUI
    // primitive funnels through these helpers, so any painter that
    // under-runs a sub-rect's origin (x < area.x or y < area.y — e.g. a
    // nested panel painting one column before its own left edge) hit
    // ratatui's `buf[(x, y)]` index panic. `Buffer::empty` with a
    // non-zero-origin `Rect` reproduces a sub-rect the way a nested
    // primitive would see it.

    #[test]
    fn set_cell_guards_left_and_top_edge() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        // Below-origin coordinates must not panic.
        set_cell(
            &mut buf,
            0,
            0,
            'x',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        set_cell(
            &mut buf,
            2,
            8,
            'x',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        set_cell(
            &mut buf,
            8,
            2,
            'x',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        // A genuinely in-bounds cell is still painted.
        set_cell(
            &mut buf,
            6,
            6,
            'y',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        assert_eq!(buf[(6, 6)].symbol(), "y");
    }

    #[test]
    fn set_cell_wide_guards_left_and_top_edge() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        set_cell_wide(
            &mut buf,
            0,
            0,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        set_cell_wide(
            &mut buf,
            2,
            8,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        set_cell_wide(
            &mut buf,
            8,
            2,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        set_cell_wide(
            &mut buf,
            6,
            6,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
        );
        assert_eq!(buf[(6, 6)].symbol(), "中");
    }

    #[test]
    fn set_cell_styled_guards_left_and_top_edge() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        set_cell_styled(
            &mut buf,
            0,
            0,
            'x',
            RatatuiColor::White,
            RatatuiColor::Black,
            Modifier::empty(),
            None,
        );
        set_cell_styled(
            &mut buf,
            6,
            6,
            'y',
            RatatuiColor::White,
            RatatuiColor::Black,
            Modifier::empty(),
            None,
        );
        assert_eq!(buf[(6, 6)].symbol(), "y");
    }

    #[test]
    fn set_cell_wide_styled_guards_left_and_top_edge() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        set_cell_wide_styled(
            &mut buf,
            0,
            0,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
            Modifier::empty(),
            None,
        );
        set_cell_wide_styled(
            &mut buf,
            6,
            6,
            '中',
            RatatuiColor::White,
            RatatuiColor::Black,
            Modifier::empty(),
            None,
        );
        assert_eq!(buf[(6, 6)].symbol(), "中");
    }

    // Regression test for #471 (fix iteration 1): `StyledText::visible_width`
    // became display-width-aware (CJK/emoji count double), but
    // `draw_styled_text` kept striding one buffer column per `char` — so a
    // CJK label's *painted* extent (half its measured width) no longer
    // matched what layout code reserved for it via `visible_width()`,
    // producing a gap between painted labels and the controls positioned
    // after them. `draw_styled_text` now strides by `char_cell_width`
    // (mirroring `set_cell_wide`'s callers elsewhere in this file), so its
    // return value — the column layout code implicitly relies on matching
    // `visible_width()` — agrees with `visible_width()` end-to-end.
    #[test]
    fn draw_styled_text_strides_by_display_width_for_cjk() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let area = Rect::new(0, 0, 20, 3);
        let text = StyledText::plain("日本語");
        assert_eq!(text.visible_width(), 6, "CJK: 3 chars, 2 cols each");

        let end_col = draw_styled_text(
            &mut buf,
            area,
            0,
            1,
            &text,
            RatatuiColor::White,
            RatatuiColor::Black,
            Decoration::Normal,
            RatatuiColor::Gray,
        );

        // The returned column matches `visible_width()`, not `chars().count()`
        // (which would have returned 1 + 3 = 4).
        assert_eq!(end_col, 1 + text.visible_width());

        // Each wide glyph occupies two buffer cells: the glyph itself, then
        // an empty continuation cell (mirrors `set_cell_wide`'s contract).
        assert_eq!(buf[(1, 0)].symbol(), "日");
        assert_eq!(buf[(2, 0)].symbol(), "");
        assert_eq!(buf[(3, 0)].symbol(), "本");
        assert_eq!(buf[(4, 0)].symbol(), "");
        assert_eq!(buf[(5, 0)].symbol(), "語");
        assert_eq!(buf[(6, 0)].symbol(), "");
        // Nothing painted past the label's actual display width.
        assert_eq!(buf[(7, 0)].symbol(), " ");
    }
}
