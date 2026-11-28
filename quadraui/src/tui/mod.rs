//! TUI backend: a terminal through ratatui.
//!
//! Enabled via the `tui` Cargo feature. Every widget paints through its
//! widget contract (`Backend::draw_dyn`) onto `TuiBackend`'s
//! [`crate::cell_surface::CellSurface`]; this module carries only the
//! platform plumbing (terminal setup, events, text measurement, services,
//! the run loop and the test driver).
//!
//! The re-exports with doc comments on this page (`TuiBackend`, `run`, …)
//! are the stable, consumer-depended-on subset; everything else is
//! `#[doc(hidden)]`: **unstable, not covered by semver**.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use crate::cell_surface::{solid_style, CellSurface};
use crate::types::{Color, Decoration, StyledText};

pub mod backend;
// `pub(crate)` rather than private: widget code outside `crate::tui` builds
// a `TuiCellSurface` over a bare `Buffer` for its own painters and tests.
// The public adapter is `BufferCellSurface`, re-exported below.
pub(crate) mod cell_surface;
// No reacher in `~/src/vimcode/src`, `~/src/coord-tui/src`, or any in-tree
// consumer reaches either module-qualified (`crate::tui::caps::…`) —
// `#[doc(hidden)]`: unstable, not covered by semver, like the flat
// re-exports in the `pub use` block below.
#[doc(hidden)]
pub mod caps;
#[doc(hidden)]
pub mod color;
// `pub(crate)` rather than private: `CommandLine`'s own
// `Primitive::paint_cells` reaches this module's `Buffer` →
// `CellSurface` adapter, which cannot be built outside `crate::tui`.
pub mod events;
mod focus_ring;
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
mod run;
pub mod services;
pub mod shell_runner;
pub mod testing;
pub mod text;
// vt100/ANSI-byte-stream conformance observer — needs
// `vt100` itself, which only `terminal` pulls in (`terminal = ["dep:vt100",
// ...]` in Cargo.toml), so this is gated on both features, same as
// `tests/tui_pty_smoke.rs`.
#[cfg(feature = "terminal")]
pub mod vt_testing;

// The flat re-exports below a real caller reaches by this exact path stay
// documented; the rest are `#[doc(hidden)]` — unstable, not covered by
// semver (see `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`).
pub use backend::TuiBackend;
pub use cell_surface::BufferCellSurface;
#[doc(hidden)]
pub use focus_ring::draw_focus_ring;
pub use run::{run, run_with, RunConfig, StepOutcome, TuiRunner};
#[doc(hidden)]
pub use services::TuiPlatformServices;
// The cell-measure constants live with the primitive, next to the shared
// cell-grid painter that reads them; `tui::` re-exports them under the
// names apps already build their `TabMeasure`s against.
#[doc(hidden)]
pub use crate::primitives::tab_bar::{
    TAB_CLOSE_CHAR, TAB_CLOSE_COLS, TAB_SEPARATOR_CHAR, TAB_SEPARATOR_COLS,
};
pub(crate) use crate::primitives::tree::paint_cells::paint_tree_cell;
pub use crate::primitives::tree::paint_cells::tui_tree_layout;
#[doc(hidden)]
pub use crate::primitives::tree::paint_cells::tui_tree_layout_with_scroll_px;
#[doc(hidden)]
pub use text::truncate_to_width_ellipsis;
pub use text::{char_cell_width, display_width, truncate_to_width};

/// Convert a `quadraui::Color` to the ratatui palette colour used by
/// [`BufferCellSurface`]. Public so an app that paints its own cells into
/// the same ratatui buffer (borders, background fills) matches the colours
/// the widgets paint.
pub fn ratatui_color(c: Color) -> RatatuiColor {
    RatatuiColor::Rgb(c.r, c.g, c.b)
}

/// Draw a [`StyledText`] onto `surface` starting at `(area.x + start_col,
/// y)`, returning the column past the last drawn character. Honors the
/// caller's `decoration` as a final colour override (e.g. `Muted` dims
/// every span that didn't already specify its own `fg`). Used by the
/// form and tree cell painters.
///
/// Strides by each character's [`char_cell_width`] (via
/// [`CellSurface::put_wide`] for double-width glyphs) rather than one
/// column per `char`, so the painted extent agrees with
/// [`StyledText::visible_width`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_styled_text(
    surface: &mut dyn CellSurface,
    area: Rect,
    y: u16,
    start_col: usize,
    text: &StyledText,
    default_fg: Color,
    bg: Color,
    decoration: Decoration,
    dim_fg: Color,
) -> usize {
    let mut col = start_col;
    for span in &text.spans {
        let span_fg = if let Some(c) = span.fg {
            c
        } else if matches!(decoration, Decoration::Muted) {
            dim_fg
        } else {
            default_fg
        };
        let span_bg = span.bg.unwrap_or(bg);
        for ch in span.text.chars() {
            if col >= area.width as usize {
                return col;
            }
            let w = char_cell_width(ch) as usize;
            if w == 2 && col + 1 < area.width as usize {
                surface.put_wide(area.x + col as u16, y, ch, solid_style(span_fg, span_bg));
            } else {
                surface.put_char(area.x + col as u16, y, ch, solid_style(span_fg, span_bg));
            }
            col += w;
        }
    }
    col
}

/// Whether column `col` (relative to the painted run's own left edge)
/// holds the trailing half of a double-width glyph from `text`, as
/// [`draw_styled_text`] and the list's scrolled-row painter lay it out:
/// the text starts at virtual column `start`, its first `skip` virtual
/// columns are scrolled away, and a double-width glyph paints whole only
/// when both of its columns fall inside `width`.
pub(crate) fn styled_text_continuation_at(
    text: &StyledText,
    start: usize,
    skip: usize,
    width: usize,
    col: usize,
) -> bool {
    let mut vcol = start;
    for ch in text.spans.iter().flat_map(|span| span.text.chars()) {
        if vcol > skip + col {
            break;
        }
        let w = char_cell_width(ch) as usize;
        if w == 2 && vcol >= skip && vcol - skip + 1 < width && vcol - skip + 1 == col {
            return true;
        }
        vcol += w;
    }
    false
}

/// Set a buffer cell with explicit modifier + optional underline colour —
/// the writer behind [`BufferCellSurface`]'s glyph verbs.
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

/// The double-width counterpart of [`set_cell_styled`]: a 2-cell-wide
/// character carrying an explicit modifier + optional underline colour,
/// so a double-width glyph keeps the same modifiers
/// [`set_cell_styled`] applies to narrow glyphs.
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
/// Used by the tree row rename editor in `primitives::tree::paint_cells`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_text_editor(
    surface: &mut dyn CellSurface,
    area: Rect,
    y: u16,
    start_col: usize,
    text: &str,
    cursor: usize,
    selection_anchor: Option<usize>,
    placeholder: Option<&str>,
    fg: Color,
    bg: Color,
    sel_bg: Color,
    dim_fg: Color,
) {
    if text.is_empty() {
        if let Some(ph) = placeholder {
            for (col, ch) in (start_col..).zip(ph.chars()) {
                if col >= area.width as usize {
                    break;
                }
                surface.put_char(area.x + col as u16, y, ch, solid_style(dim_fg, bg));
            }
        }
        // Cursor at position 0 (inverted space).
        if start_col < area.width as usize {
            surface.put_char(area.x + start_col as u16, y, ' ', solid_style(bg, fg));
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
        surface.put_char(area.x + col as u16, y, ch, solid_style(fg, cell_bg));
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
        surface.put_char(area.x + cursor_col as u16, y, ch, solid_style(bg, fg));
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
            &mut BufferCellSurface::new(&mut buf),
            area,
            0,
            1,
            &text,
            Color::rgb(255, 255, 255),
            Color::rgb(0, 0, 0),
            Decoration::Normal,
            Color::rgb(128, 128, 128),
        );

        // The returned column matches `visible_width()`, not `chars().count()`
        // (which would have returned 1 + 3 = 4).
        assert_eq!(end_col, 1 + text.visible_width());

        // Each wide glyph occupies two buffer cells: the glyph itself, then
        // an empty continuation cell (mirrors `put_wide`'s contract).
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
