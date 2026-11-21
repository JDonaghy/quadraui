//! Public TUI (ratatui) rasterisers for `quadraui` primitives.
//!
//! Enabled via the `tui` Cargo feature. Apps depend on `quadraui` with
//! `features = ["tui"]` and call these `draw_*` functions to paint
//! primitives into a [`ratatui::buffer::Buffer`].
//!
//! Primitives own layout, backends rasterise. Each rasteriser takes a
//! pre-computed `*Layout` from the primitive's `.layout()` method along
//! with the primitive itself and a [`crate::Theme`] for default colours.
//!
//! Most of the free functions re-exported at the bottom of this module are
//! painter internals, not a documented API: **unstable, not covered by
//! semver** — they can move, split, or disappear between any two 0.1.x
//! releases with no version bump. The few re-exports with doc comments on
//! this page (`TuiBackend`, `run`, …) are the stable, consumer-depended-on
//! subset; everything else is `#[doc(hidden)]`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use crate::backend::ColorDepth;
use crate::cell_surface::{BorderSet, CellStyle, CellSurface, CursorShape};
use crate::types::{Color, Decoration, StyledText};

mod activity_bar;
pub mod backend;
mod board;
mod braille;
mod button;
mod canvas;
mod cell_grid;
// No reacher in `~/src/vimcode/src`, `~/src/coord-tui/src`, or any in-tree
// consumer reaches either module-qualified (`crate::tui::caps::…`) —
// `#[doc(hidden)]`: unstable, not covered by semver, like the flat
// re-exports in the `pub use` block below.
#[doc(hidden)]
pub mod caps;
mod chart;
#[doc(hidden)]
pub mod color;
mod combo_box;
mod command_center;
mod command_line;
mod completions;
mod context_menu;
mod data_table;
mod date_picker;
mod dialog;
mod diff_view;
mod drop_overlay;
mod editor;
pub mod events;
mod find_replace;
mod float;
mod focus_ring;
mod form;
mod image;
// Decode + downscale for the Kitty graphics protocol.
// Gated separately from `kitty_graphics` below: this module alone needs
// `dep:image`, so only it sits behind `tui-images` rather than the bare
// `tui` feature — see that feature's own comment in `Cargo.toml`.
#[cfg(feature = "tui-images")]
mod image_decode;
// Kitty graphics protocol byte encoding: pure
// control-data/payload formatting, no image-format decoder of its own,
// so it stays on the bare `tui` feature `kitty_graphics` and `services`'
// (OSC 52) escape-sequence helpers share — `kitty_graphics::base64_encode`
// and `kitty_graphics::tmux_passthrough_wrap` are generic terminal
// escape-sequence plumbing, not graphics-specific, so `services::
// emit_osc52_with` reuses them instead of carrying its own copy.
mod kitty_graphics;
mod list;
mod menu_bar;
mod message_list;
mod minimap;
mod multi_section_view;
mod number_input;
mod palette;
mod pipeline_view;
mod progress;
mod radio_group;
mod rich_text_popup;
mod run;
mod scrollbar;
pub mod services;
pub mod shell_runner;
mod sidebar_panel;
mod solid_fill;
mod spinner;
mod split;
mod split_junction;
mod split_tree;
mod status_bar;
mod switch;
mod tab_bar;
mod terminal;
pub mod testing;
pub mod text;
// vt100/ANSI-byte-stream conformance observer — needs
// `vt100` itself, which only `terminal` pulls in (`terminal = ["dep:vt100",
// ...]` in Cargo.toml), so this is gated on both features, same as
// `tests/tui_pty_smoke.rs`.
mod text_display;
mod text_field;
mod text_input;
mod toast;
mod toolbar;
mod tooltip;
mod tree;
#[cfg(feature = "terminal")]
pub mod vt_testing;

// The flat re-exports below split into two groups, verified by grepping
// `~/src/vimcode/src`, `~/src/coord-tui/src`, and every in-tree consumer
// (`quadraui/examples`, `quadraui/benches`, `quadraui/tests`, `kubeui*`) for
// `quadraui::tui::<name>` / `crate::tui::<name>`: the handful a real caller
// reaches by this exact flat path stay documented; everything else is a
// painter internal duplicating what `Backend::draw_*` already exposes, with
// zero reachers, so it is `#[doc(hidden)]` — unstable, not covered by
// semver, free to move or disappear without a version bump (see
// `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`).
#[doc(hidden)]
pub use activity_bar::{draw_activity_bar, draw_activity_bar_with_style};
pub use backend::TuiBackend;
#[doc(hidden)]
pub use board::{draw_board, tui_board_layout};
#[doc(hidden)]
pub use button::{draw_button, draw_button_interactive, tui_button_layout};
#[doc(hidden)]
pub use canvas::{draw_canvas, tui_canvas_layout, tui_canvas_region_hit_test};
#[doc(hidden)]
pub use cell_grid::{draw_cell_grid, tui_cell_grid_layout};
#[doc(hidden)]
pub use chart::{draw_chart, tui_chart_layout};
#[doc(hidden)]
pub use combo_box::draw_combo_box;
#[doc(hidden)]
pub use command_center::{draw_command_center, tui_command_center_layout};
#[doc(hidden)]
pub use completions::draw_completions;
#[doc(hidden)]
pub use context_menu::{
    draw_context_menu, draw_context_menu_with_glyphs, draw_context_menu_with_submenus,
    BORDER_INSET as CONTEXT_MENU_BORDER_INSET,
};
pub use data_table::draw_data_table;
#[doc(hidden)]
pub use data_table::{data_table_layout, draw_data_table_with_glyphs, draw_data_table_with_state};
#[doc(hidden)]
pub use data_table::{draw_tree_table, tree_table_layout};
#[doc(hidden)]
pub use date_picker::draw_date_picker;
#[doc(hidden)]
pub use dialog::{draw_dialog, draw_dialog_with_nerd_fonts, tui_dialog_layout};
#[doc(hidden)]
pub use diff_view::draw_diff_view;
#[doc(hidden)]
pub use drop_overlay::draw_drop_overlay;
pub use editor::draw_editor;
#[doc(hidden)]
pub use editor::{draw_editor_with_reserve, EditorPaintResult};
#[doc(hidden)]
pub use find_replace::draw_find_replace;
#[doc(hidden)]
pub use float::draw_float;
#[doc(hidden)]
pub use focus_ring::draw_focus_ring;
pub use form::draw_form;
#[doc(hidden)]
pub use form::{draw_settings_chrome, tui_form_layout};
#[doc(hidden)]
pub use image::draw_image;
#[doc(hidden)]
pub use list::{
    draw_list, draw_list_with_scroll_px, draw_list_with_state, tui_list_layout,
    tui_list_layout_with_scroll_px,
};
#[doc(hidden)]
pub use menu_bar::{draw_menu_bar, tui_menu_bar_layout};
#[doc(hidden)]
pub use message_list::draw_message_list;
pub use minimap::draw_minimap;
#[doc(hidden)]
pub use minimap::{
    draw_minimap_with_scale, draw_minimap_with_sizing, tui_minimap_layout,
    tui_minimap_layout_with_sizing, COLS_PER_CELL, LINES_PER_ROW,
};
pub use multi_section_view::{draw_multi_section_view, tui_msv_layout};
#[doc(hidden)]
pub use number_input::draw_number_input;
pub(crate) use palette::draw_palette_with_style;
#[doc(hidden)]
pub use palette::{draw_palette, tui_palette_layout};
#[doc(hidden)]
pub use pipeline_view::{draw_pipeline_view, tui_pipeline_view_layout};
#[doc(hidden)]
pub use progress::{draw_progress, tui_progress_layout};
#[doc(hidden)]
pub use radio_group::{draw_radio_group, tui_radio_group_layout};
#[doc(hidden)]
pub use rich_text_popup::draw_rich_text_popup;
pub use run::{run, run_with, RunConfig, StepOutcome, TuiRunner};
#[doc(hidden)]
pub use scrollbar::draw_scrollbar;
#[doc(hidden)]
pub use services::TuiPlatformServices;
#[doc(hidden)]
pub use sidebar_panel::{draw_sidebar_panel, tui_sidebar_panel_layout};
#[doc(hidden)]
pub use solid_fill::draw_solid_fill;
#[doc(hidden)]
pub use spinner::{draw_spinner, tui_spinner_layout};
#[doc(hidden)]
pub use split::{draw_split, tui_split_layout};
#[doc(hidden)]
pub use split_tree::{draw_split_tree, tui_split_tree_layout};
#[doc(hidden)]
pub use status_bar::draw_status_bar;
#[doc(hidden)]
pub use switch::{draw_switch, tui_switch_layout};
// The cell-measure constants live with the primitive, next to the shared
// cell-grid painter that reads them; `tui::` re-exports them under the
// names apps already build their `TabMeasure`s against.
#[doc(hidden)]
pub use crate::primitives::tab_bar::{
    TAB_CLOSE_CHAR, TAB_CLOSE_COLS, TAB_SEPARATOR_CHAR, TAB_SEPARATOR_COLS,
};
#[doc(hidden)]
pub use tab_bar::{
    draw_tab_bar, draw_tab_bar_icons, draw_tab_bar_icons_with_chrome,
    draw_tab_bar_icons_with_chrome_and_glyphs, draw_tab_bar_with_chrome,
};
#[doc(hidden)]
pub use terminal::{draw_terminal, draw_terminal_divider};
#[doc(hidden)]
pub use text::truncate_to_width_ellipsis;
pub use text::{char_cell_width, display_width, truncate_to_width};
#[doc(hidden)]
pub use text_display::{draw_text_display, tui_text_display_layout};
#[doc(hidden)]
pub use text_field::{draw_text_field, tui_text_field_layout};
#[doc(hidden)]
pub use text_input::{draw_text_input, tui_text_input_layout};
#[doc(hidden)]
pub use toast::{draw_toast_stack, tui_toast_stack_layout};
pub use toolbar::tui_toolbar_layout;
#[doc(hidden)]
pub use toolbar::{
    draw_toolbar, draw_toolbar_oriented, draw_toolbar_with_options, draw_toolbar_with_overflow,
    tui_toolbar_layout_with_options, tui_toolbar_layout_with_overflow,
};
#[doc(hidden)]
pub use tooltip::{
    draw_tooltip, draw_tooltip_with_chrome, painted_bounds as tooltip_painted_bounds,
};
pub use tree::{draw_tree, tui_tree_layout};
#[doc(hidden)]
pub use tree::{draw_tree_with_scroll_px, tui_tree_layout_with_scroll_px};

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
pub(crate) fn set_cell(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    ch: char,
    fg: RatatuiColor,
    bg: RatatuiColor,
) {
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
pub(crate) fn set_cell_wide(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    ch: char,
    fg: RatatuiColor,
    bg: RatatuiColor,
) {
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
/// the painted extent agrees with [`StyledText::visible_width`].
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
/// Used by `primitives::tab_bar`'s `cell_paint` for the accent underline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_cell_styled(
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
/// Used by the same painter so a double-width glyph in the
/// filename portion of an active tab's label still gets the accent
/// underline / preview-italic modifier that [`set_cell_styled`] applies
/// to narrow glyphs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_cell_wide_styled(
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

/// Left/center/right offset (in cells) a `text_len`-wide run starts at
/// within a `col_w`-wide column — the one alignment rule every
/// column/cell text painter in this module shares: [`data_table`]'s
/// header/body/footer cells and [`cell_grid`]'s header/gutter/body
/// cells.
pub(crate) fn align_offset(
    align: crate::primitives::data_table::ColumnAlign,
    text_len: u16,
    col_w: u16,
) -> u16 {
    use crate::primitives::data_table::ColumnAlign;
    match align {
        ColumnAlign::Left => 0,
        ColumnAlign::Center => col_w.saturating_sub(text_len) / 2,
        ColumnAlign::Right => col_w.saturating_sub(text_len),
    }
}

/// Paint a single-line inline text editor — a text buffer, byte-offset
/// cursor (rendered as an inverted-colour block), an optional selection
/// highlight, and (when `text` is empty) a dim placeholder — starting at
/// `start_col` cells into `area`, on row `y`, clipped to `area.width`.
///
/// Shared by every TUI inline-editing surface so there is exactly one
/// cursor/selection/placeholder rasteriser rather than a per-primitive
/// copy: [`crate::tui::tree::paint_edit_input`] (tree row rename) and
/// [`crate::tui::cell_grid::draw_cell_grid`]'s cell editor overlay both
/// call this directly instead of re-deriving the same multibyte-aware
/// cursor/selection arithmetic.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_text_editor(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    start_col: usize,
    text: &str,
    cursor: usize,
    selection_anchor: Option<usize>,
    placeholder: Option<&str>,
    fg: RatatuiColor,
    bg: RatatuiColor,
    sel_bg: RatatuiColor,
    dim_fg: RatatuiColor,
) {
    if text.is_empty() {
        if let Some(ph) = placeholder {
            for (col, ch) in (start_col..).zip(ph.chars()) {
                if col >= area.width as usize {
                    break;
                }
                set_cell(buf, area.x + col as u16, y, ch, dim_fg, bg);
            }
        }
        // Cursor at position 0 (inverted space).
        if start_col < area.width as usize {
            set_cell(buf, area.x + start_col as u16, y, ' ', bg, fg);
        }
        return;
    }

    let (sel_lo, sel_hi) = match selection_anchor {
        Some(a) if a != cursor => (a.min(cursor), a.max(cursor)),
        _ => (0, 0),
    };
    let has_selection = sel_hi > sel_lo;

    let mut byte = 0usize;
    for (col, ch) in (start_col..).zip(text.chars()) {
        if col >= area.width as usize {
            break;
        }
        let in_sel = has_selection && byte >= sel_lo && byte < sel_hi;
        let cell_bg = if in_sel { sel_bg } else { bg };
        set_cell(buf, area.x + col as u16, y, ch, fg, cell_bg);
        byte += ch.len_utf8();
    }

    // Block cursor: find char index for cursor byte offset, invert that cell.
    let mut cursor_char_idx = 0usize;
    let mut b = 0usize;
    for ch in text.chars() {
        if b >= cursor {
            break;
        }
        b += ch.len_utf8();
        cursor_char_idx += 1;
    }
    let cursor_col = start_col + cursor_char_idx;
    if cursor_col < area.width as usize {
        let ch = text.chars().nth(cursor_char_idx).unwrap_or(' ');
        set_cell(buf, area.x + cursor_col as u16, y, ch, bg, fg);
    }
}

// ─── CellSurface ─────────────────────────────────────────────────────────

/// A box-drawing glyph family's six characters — the table
/// [`border_glyphs`] picks a row out of for [`TuiCellSurface::draw_border`].
struct BorderGlyphs {
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
    horizontal: char,
    vertical: char,
}

/// The one place every [`BorderSet`] variant's literal glyphs live.
/// `Rounded` matches the glyphs `crate::tui::dialog`/`crate::tui::board`/
/// `crate::tui::focus_ring` already paint by hand; `Square`/`Double`/
/// `Heavy` are new — [`TuiCellSurface::draw_border`] is their first
/// caller.
fn border_glyphs(set: BorderSet) -> BorderGlyphs {
    match set {
        BorderSet::Square => BorderGlyphs {
            top_left: '┌',
            top_right: '┐',
            bottom_left: '└',
            bottom_right: '┘',
            horizontal: '─',
            vertical: '│',
        },
        BorderSet::Rounded => BorderGlyphs {
            top_left: '╭',
            top_right: '╮',
            bottom_left: '╰',
            bottom_right: '╯',
            horizontal: '─',
            vertical: '│',
        },
        BorderSet::Double => BorderGlyphs {
            top_left: '╔',
            top_right: '╗',
            bottom_left: '╚',
            bottom_right: '╝',
            horizontal: '═',
            vertical: '║',
        },
        BorderSet::Heavy => BorderGlyphs {
            top_left: '┏',
            top_right: '┓',
            bottom_left: '┗',
            bottom_right: '┛',
            horizontal: '━',
            vertical: '┃',
        },
    }
}

/// `style`'s colour/modifier fields, resolved to the trio
/// [`set_cell_styled`]/[`set_cell_wide_styled`] take: `fg`/`bg` default to
/// [`RatatuiColor::Reset`] (the terminal's own default, SGR `39`/`49`) when
/// `None` — see [`CellStyle`]'s own doc for why `None` means that rather
/// than "unspecified".
fn resolve_cell_style(
    style: CellStyle,
) -> (RatatuiColor, RatatuiColor, Modifier, Option<RatatuiColor>) {
    let fg = style.fg.map(ratatui_color).unwrap_or(RatatuiColor::Reset);
    let bg = style.bg.map(ratatui_color).unwrap_or(RatatuiColor::Reset);
    let mut modifier = Modifier::empty();
    if style.bold {
        modifier |= Modifier::BOLD;
    }
    if style.italic {
        modifier |= Modifier::ITALIC;
    }
    if style.dim {
        modifier |= Modifier::DIM;
    }
    if style.reverse {
        modifier |= Modifier::REVERSED;
    }
    let underline_color = style.underline.map(ratatui_color);
    if underline_color.is_some() {
        modifier |= Modifier::UNDERLINED;
    }
    (fg, bg, modifier, underline_color)
}

/// Recover a [`Color`] from a ratatui [`RatatuiColor`] that one of this
/// module's own `set_cell*` helpers wrote (always [`RatatuiColor::Rgb`] —
/// see [`ratatui_color`]). Returns `None` for any other variant (`Reset`,
/// a named colour, `Indexed`), which carries no literal RGB to recover —
/// the same restriction `crate::tui::solid_fill`'s own `color_from_ratatui`
/// documents.
fn color_from_ratatui(c: RatatuiColor) -> Option<Color> {
    match c {
        RatatuiColor::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
        _ => None,
    }
}

/// Clamp `rect`'s backend-native (cell) units down to a `(x, y, width,
/// height)` whole-cell quad — negative origins and sizes both floor to
/// `0`, matching [`CellSurface::fill`]/[`CellSurface::draw_border`]'s
/// contract of painting nothing rather than panicking on an
/// out-of-range `Rect`.
fn cell_quad(rect: crate::Rect) -> (u16, u16, u16, u16) {
    let x = rect.x.max(0.0).round() as u16;
    let y = rect.y.max(0.0).round() as u16;
    let width = rect.width.max(0.0).round() as u16;
    let height = rect.height.max(0.0).round() as u16;
    (x, y, width, height)
}

/// [`CellSurface`] over a live ratatui buffer — the trait's one
/// implementor today, built fresh by [`crate::tui::backend::TuiBackend::cell_surface`]
/// each call. See `crate::cell_surface`'s module doc for scope.
///
/// Holds `buf` as a raw pointer rather than `&mut Buffer` so this type
/// carries no lifetime parameter: [`crate::tui::backend::TuiBackend`]
/// itself has none (matching every other `Backend` implementor), so a
/// field holding the `&mut dyn CellSurface`
/// [`crate::tui::backend::TuiBackend::cell_surface`] hands back could not
/// otherwise be stored on it between calls. Mirrors the type-erased
/// `*mut ()` [`crate::tui::backend::TuiBackend::current_frame_ptr`] already
/// stores for the identical reason — see that field's own doc, "Frame-scope
/// mechanism" — except `Buffer` itself has no lifetime to erase, so the raw
/// pointer here is simply `*mut Buffer`, no `()` type-erasure cast needed.
pub(crate) struct TuiCellSurface {
    /// Valid only while the [`crate::tui::backend::TuiBackend::enter_frame_scope`]
    /// call that produced the `&mut Buffer` passed to [`Self::new`] is
    /// still on the stack — the same invariant
    /// [`crate::tui::backend::TuiBackend::current_frame_mut`]'s own doc
    /// documents for the `Frame` pointer this is ultimately derived from.
    /// Every accessor reaches it only through [`Self::buf`].
    buf: *mut Buffer,
    color_depth: ColorDepth,
    cursor: Option<(u16, u16, CursorShape)>,
}

impl TuiCellSurface {
    pub(crate) fn new(buf: &mut Buffer, color_depth: ColorDepth) -> Self {
        Self {
            buf: buf as *mut Buffer,
            color_depth,
            cursor: None,
        }
    }

    /// Reborrow the live buffer. See [`Self::buf`]'s own field doc for the
    /// validity invariant this relies on — callers reach `TuiCellSurface`
    /// only through [`crate::backend::Backend::cell_surface`], which never
    /// answers `Some` outside that invariant's window.
    fn buf(&mut self) -> &mut Buffer {
        // SAFETY: see this type's own doc and the `buf` field's doc.
        unsafe { &mut *self.buf }
    }

    /// The most recent [`CellSurface::set_cursor`] call this surface
    /// received this frame, if any.
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> Option<(u16, u16, CursorShape)> {
        self.cursor
    }
}

impl CellSurface for TuiCellSurface {
    fn size(&self) -> (u16, u16) {
        // SAFETY: see `Self::buf`'s own doc — read-only use, no reference
        // escapes this method.
        let area = unsafe { &*self.buf }.area;
        (area.width, area.height)
    }

    fn put_char(&mut self, x: u16, y: u16, ch: char, style: CellStyle) {
        let (fg, bg, modifier, underline_color) = resolve_cell_style(style);
        set_cell_styled(self.buf(), x, y, ch, fg, bg, modifier, underline_color);
    }

    fn put_wide(&mut self, x: u16, y: u16, ch: char, style: CellStyle) {
        let (fg, bg, modifier, underline_color) = resolve_cell_style(style);
        set_cell_wide_styled(self.buf(), x, y, ch, fg, bg, modifier, underline_color);
    }

    fn put_str_clipped(
        &mut self,
        x: u16,
        y: u16,
        text: &str,
        max_cols: u16,
        style: CellStyle,
    ) -> u16 {
        let mut col = 0u16;
        for ch in text.chars() {
            let w = char_cell_width(ch);
            if col.saturating_add(w) > max_cols {
                break;
            }
            if w == 2 {
                self.put_wide(x + col, y, ch, style);
            } else {
                self.put_char(x + col, y, ch, style);
            }
            col += w;
        }
        col
    }

    fn fill(&mut self, rect: crate::Rect, style: CellStyle) {
        let (x0, y0, width, height) = cell_quad(rect);
        for y in y0..y0.saturating_add(height) {
            for x in x0..x0.saturating_add(width) {
                self.put_char(x, y, ' ', style);
            }
        }
    }

    fn draw_border(&mut self, rect: crate::Rect, set: BorderSet, style: CellStyle) {
        let (x0, y0, width, height) = cell_quad(rect);
        if width < 2 || height < 2 {
            return;
        }
        let x1 = x0 + width - 1;
        let y1 = y0 + height - 1;
        let glyphs = border_glyphs(set);
        self.put_char(x0, y0, glyphs.top_left, style);
        self.put_char(x1, y0, glyphs.top_right, style);
        self.put_char(x0, y1, glyphs.bottom_left, style);
        self.put_char(x1, y1, glyphs.bottom_right, style);
        for x in (x0 + 1)..x1 {
            self.put_char(x, y0, glyphs.horizontal, style);
            self.put_char(x, y1, glyphs.horizontal, style);
        }
        for y in (y0 + 1)..y1 {
            self.put_char(x0, y, glyphs.vertical, style);
            self.put_char(x1, y, glyphs.vertical, style);
        }
    }

    fn get(&self, x: u16, y: u16) -> Option<(char, CellStyle)> {
        // SAFETY: see `Self::buf`'s own doc — read-only use, no reference
        // escapes this method.
        let buf = unsafe { &*self.buf };
        let area = buf.area;
        if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
            return None;
        }
        let cell = &buf[(x, y)];
        let ch = cell.symbol().chars().next().unwrap_or(' ');
        let style = CellStyle {
            fg: color_from_ratatui(cell.fg),
            bg: color_from_ratatui(cell.bg),
            bold: cell.modifier.contains(Modifier::BOLD),
            italic: cell.modifier.contains(Modifier::ITALIC),
            underline: if cell.modifier.contains(Modifier::UNDERLINED) {
                color_from_ratatui(cell.underline_color)
            } else {
                None
            },
            dim: cell.modifier.contains(Modifier::DIM),
            reverse: cell.modifier.contains(Modifier::REVERSED),
        };
        Some((ch, style))
    }

    fn set_cursor(&mut self, x: u16, y: u16, shape: CursorShape) {
        self.cursor = Some((x, y, shape));
    }

    fn color_depth(&self) -> ColorDepth {
        self.color_depth
    }

    #[cfg(feature = "tui")]
    fn ratatui_buffer(&mut self) -> Option<&mut Buffer> {
        Some(self.buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Guards the left and top edge of `buf.area`: coordinates below the
    // sub-rect's origin (x < area.x or y < area.y) must not panic.
    // `Buffer::empty` with a non-zero-origin `Rect` reproduces a sub-rect
    // the way a nested primitive would see it.

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

    // `StyledText::visible_width` is display-width-aware (CJK/emoji count
    // double). `draw_styled_text` strides by `char_cell_width` to match,
    // so its return value — the column layout code relies on — agrees with
    // `visible_width()`.
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

    // ─── TuiCellSurface ────────────────────────────────────────────────

    #[test]
    fn tui_cell_surface_put_wide_blanks_continuation_cell() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        surface.put_wide(2, 1, '中', CellStyle::default());
        assert_eq!(buf[(2, 1)].symbol(), "中");
        assert_eq!(
            buf[(3, 1)].symbol(),
            "",
            "the continuation cell must be blanked, not left stale"
        );
    }

    #[test]
    fn tui_cell_surface_put_str_clipped_never_splits_a_double_width_glyph() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        // "日本語" is 3 double-width glyphs (6 columns). A 5-column budget
        // fits two glyphs (4 columns) but not a third's two columns.
        let written = surface.put_str_clipped(0, 0, "日本語", 5, CellStyle::default());
        assert_eq!(
            written, 4,
            "only the first two double-width glyphs fit a 5-column budget"
        );
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(2, 0)].symbol(), "本");
        // Nothing painted past the written width.
        assert_eq!(buf[(4, 0)].symbol(), " ");
    }

    #[test]
    fn tui_cell_surface_draw_border_paints_every_glyph_set() {
        let cases = [
            (BorderSet::Square, '┌', '┐', '└', '┘', '─', '│'),
            (BorderSet::Rounded, '╭', '╮', '╰', '╯', '─', '│'),
            (BorderSet::Double, '╔', '╗', '╚', '╝', '═', '║'),
            (BorderSet::Heavy, '┏', '┓', '┗', '┛', '━', '┃'),
        ];
        for (set, top_left, top_right, bottom_left, bottom_right, horizontal, vertical) in cases {
            let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
            let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
            surface.draw_border(
                crate::Rect::new(1.0, 1.0, 4.0, 3.0),
                set,
                CellStyle::default(),
            );
            assert_eq!(
                buf[(1, 1)].symbol(),
                top_left.to_string(),
                "{set:?} top-left"
            );
            assert_eq!(
                buf[(4, 1)].symbol(),
                top_right.to_string(),
                "{set:?} top-right"
            );
            assert_eq!(
                buf[(1, 3)].symbol(),
                bottom_left.to_string(),
                "{set:?} bottom-left"
            );
            assert_eq!(
                buf[(4, 3)].symbol(),
                bottom_right.to_string(),
                "{set:?} bottom-right"
            );
            assert_eq!(
                buf[(2, 1)].symbol(),
                horizontal.to_string(),
                "{set:?} top edge"
            );
            assert_eq!(
                buf[(1, 2)].symbol(),
                vertical.to_string(),
                "{set:?} left edge"
            );
        }
    }

    #[test]
    fn tui_cell_surface_draw_border_too_small_paints_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
        let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        surface.draw_border(
            crate::Rect::new(1.0, 1.0, 1.0, 1.0),
            BorderSet::Square,
            CellStyle::default(),
        );
        assert_eq!(
            buf[(1, 1)].symbol(),
            " ",
            "a 1x1 rect has no border to draw"
        );
    }

    #[test]
    fn tui_cell_surface_get_round_trips_put_char() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        let style = CellStyle {
            fg: Some(Color::rgb(10, 20, 30)),
            bg: Some(Color::rgb(40, 50, 60)),
            bold: true,
            italic: false,
            underline: Some(Color::rgb(70, 80, 90)),
            dim: false,
            reverse: true,
        };
        surface.put_char(3, 1, 'z', style);
        let (ch, got) = surface.get(3, 1).expect("cell was just painted");
        assert_eq!(ch, 'z');
        assert_eq!(got, style);
    }

    #[test]
    fn tui_cell_surface_get_out_of_bounds_is_none() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        let surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        assert!(surface.get(0, 0).is_none());
    }

    #[test]
    fn tui_cell_surface_set_cursor_records_position_and_shape() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut surface = TuiCellSurface::new(&mut buf, ColorDepth::TrueColor);
        assert_eq!(surface.cursor(), None);
        surface.set_cursor(4, 2, CursorShape::Bar);
        assert_eq!(surface.cursor(), Some((4, 2, CursorShape::Bar)));
    }

    #[test]
    fn tui_cell_surface_color_depth_reports_the_constructor_value() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let surface = TuiCellSurface::new(&mut buf, ColorDepth::Ansi16);
        assert_eq!(surface.color_depth(), ColorDepth::Ansi16);
    }
}
