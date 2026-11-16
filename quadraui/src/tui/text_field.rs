//! TUI rasteriser for [`crate::TextField`].
//!
//! Exactly one row: `[` + optional leading-icon glyph + text + optional
//! `×` clear button + `]`, with a block caret (reverse-video on the
//! cursor's cell) — the same bracketed-text idiom
//! [`crate::tui::form::draw_form`]'s `TextInput` row already paints.
//! [`draw_text_row_cells`] is the one function both share for the
//! "paint the text between the brackets" half, so they can never drift
//! apart on cursor/selection math; `Form`'s row and this module's own
//! brackets/icon/clear button paint separately because only
//! `TextField` scrolls (`Form`'s row truncates instead).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::{qc, set_cell};
use crate::primitives::text_field::{TextField, TextFieldLayout, TextFieldMeasure};
use crate::theme::Theme;

pub fn tui_text_field_layout(tf: &TextField, rect: Rect) -> TextFieldLayout {
    let r = crate::event::Rect::new(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    tf.layout(r, TextFieldMeasure::new(1.0, 1.0))
}

/// Draw a [`TextField`] into `rect` on `buf`. Returns the resolved
/// layout for hit-testing, same convention as
/// [`crate::tui::draw_text_input`].
pub fn draw_text_field(
    buf: &mut Buffer,
    rect: Rect,
    tf: &TextField,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) -> TextFieldLayout {
    let layout = tui_text_field_layout(tf, rect);
    if rect.width == 0 || rect.height == 0 {
        return layout;
    }

    let y = rect.y;
    let row_bg = qc(theme.tab_bar_bg);
    let field_fg = qc(theme.foreground);
    let dim_fg = qc(theme.muted_fg);
    let bracket_fg = if tf.has_focus {
        qc(theme.accent_fg)
    } else {
        dim_fg
    };

    // Clear the whole row first — same "paint the background, then
    // overlay content" order every other TUI rasteriser uses.
    let x1 = rect.x + rect.width.saturating_sub(1);
    for x in rect.x..=x1 {
        set_cell(buf, x, y, ' ', field_fg, row_bg);
    }

    set_cell(buf, rect.x, y, '[', bracket_fg, row_bg);
    let closing_col = x1;
    if closing_col > rect.x {
        set_cell(buf, closing_col, y, ']', bracket_fg, row_bg);
    }

    if let Some(icon_rect) = layout.icon_rect {
        let glyph = tf
            .leading_icon
            .as_ref()
            .and_then(|icon| icon.resolve(nerd_fonts_enabled).chars().next())
            .unwrap_or(' ');
        set_cell(buf, icon_rect.x as u16, y, glyph, dim_fg, row_bg);
    }

    let text_col = layout.text_rect.x as u16;
    let text_cols = layout.text_rect.width as usize;
    draw_text_row_cells(
        buf,
        y,
        text_col,
        text_col + text_cols as u16,
        text_cols,
        layout.resolved_scroll_chars,
        &tf.value,
        &tf.placeholder,
        tf.cursor,
        tf.selection_anchor,
        field_fg,
        dim_fg,
        row_bg,
    );

    if let Some(clear_rect) = layout.clear_rect {
        set_cell(buf, clear_rect.x as u16, y, '\u{00D7}', dim_fg, row_bg);
    }

    layout
}

/// Paint the inside-the-brackets text content shared by `TextField`'s
/// own rasteriser above and [`crate::tui::form::draw_form`]'s
/// `FieldKind::TextInput` row: `value` (or dimmed `placeholder` when
/// empty) starting at column `text_col`, clipped to `max_cols`, with a
/// selection highlight between the byte offsets `cursor`/
/// `selection_anchor` bracket and a reverse-video block caret at
/// `cursor`.
///
/// `skip_chars` is how many leading characters of the shown text are
/// scrolled out of view before painting starts — `0` for `Form`'s row
/// (which has no scroll, only truncation),
/// [`TextFieldLayout::resolved_scroll_chars`] for `TextField`. `Form`
/// always passes `0`, which reproduces `Form`'s own row pixel-for-pixel
/// regardless of `TextField`'s scrolling.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_text_row_cells(
    buf: &mut Buffer,
    y: u16,
    text_col: u16,
    right_bound: u16,
    max_cols: usize,
    skip_chars: usize,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    selection_anchor: Option<usize>,
    field_fg: RatatuiColor,
    dim_fg: RatatuiColor,
    row_bg: RatatuiColor,
) {
    let shown = if value.is_empty() { placeholder } else { value };
    let input_fg = if value.is_empty() { dim_fg } else { field_fg };
    let visible_chars = shown.chars().count().saturating_sub(skip_chars);
    let desired = max_cols.min(visible_chars);

    let (sel_lo, sel_hi) = if value.is_empty() {
        (0, 0)
    } else {
        match (cursor, selection_anchor) {
            (Some(c), Some(a)) if c != a => (c.min(a), c.max(a)),
            _ => (0, 0),
        }
    };

    let mut byte = 0usize;
    let mut chars = shown.chars();
    for _ in 0..skip_chars {
        match chars.next() {
            Some(ch) => byte += ch.len_utf8(),
            None => break,
        }
    }
    for (i, ch) in chars.take(desired).enumerate() {
        let col = text_col + i as u16;
        if col >= right_bound {
            break;
        }
        let in_selection = sel_hi > sel_lo && byte >= sel_lo && byte < sel_hi;
        let (ch_fg, ch_bg) = if in_selection {
            (row_bg, input_fg)
        } else {
            (input_fg, row_bg)
        };
        set_cell(buf, col, y, ch, ch_fg, ch_bg);
        byte += ch.len_utf8();
    }

    // The caret itself is bound only by `right_bound` (the same bound
    // the text loop above uses), not by `desired` — a cursor sitting
    // just past the last visible character (typing at the end of a
    // field that exactly fills its column budget) still needs a
    // paintable cell there, even though the text loop above painted
    // nothing that far right.
    if let Some(cur) = cursor {
        if !value.is_empty() {
            let mut byte2 = 0usize;
            let mut char_idx = 0usize;
            for ch in shown.chars() {
                if byte2 >= cur {
                    break;
                }
                byte2 += ch.len_utf8();
                char_idx += 1;
            }
            if char_idx >= skip_chars {
                let rel = char_idx - skip_chars;
                let cursor_col = text_col + rel as u16;
                if cursor_col < right_bound {
                    let ch = shown.chars().nth(char_idx).unwrap_or(' ');
                    set_cell(buf, cursor_col, y, ch, row_bg, field_fg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Icon, WidgetId};

    fn theme() -> Theme {
        Theme::default()
    }

    /// Paint/click round-trip (`PRIMITIVE_RULES.md` rule 4): paint, find
    /// the painted glyphs, hit_test those exact coordinates, assert
    /// paint and click identify the same region.
    #[test]
    fn paint_click_round_trip_text_and_clear() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("abc")
            .with_clearable(true)
            .with_leading_icon(Icon::new("S", "S"))
            .with_has_focus(true);
        let area = Rect::new(2, 1, 12, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);

        // Clear glyph painted where the layout says it should be.
        let clear_rect = layout.clear_rect.expect("clearable + non-empty value");
        let cell = &buf[(clear_rect.x as u16, clear_rect.y as u16)];
        assert_eq!(cell.symbol(), "\u{00D7}");
        assert_eq!(
            layout.hit_test(clear_rect.x + 0.5, clear_rect.y + 0.5),
            crate::primitives::text_field::TextFieldHit::Clear
        );

        // Text glyph painted where the layout says it should be.
        let text_rect = layout.text_rect;
        let cell = &buf[(text_rect.x as u16, text_rect.y as u16)];
        assert_eq!(cell.symbol(), "a");
        assert_eq!(
            layout.hit_test(text_rect.x + 0.5, text_rect.y + 0.5),
            crate::primitives::text_field::TextFieldHit::Text
        );

        // Brackets at the row's own edges.
        assert_eq!(buf[(area.x, area.y)].symbol(), "[");
        assert_eq!(buf[(area.x + area.width - 1, area.y)].symbol(), "]");
    }

    #[test]
    fn placeholder_renders_dimmed_when_value_is_empty() {
        let tf = TextField::new(WidgetId::new("search")).with_placeholder("Search");
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        draw_text_field(&mut buf, area, &tf, &theme(), false);
        let cell = &buf[(1, 0)];
        assert_eq!(cell.symbol(), "S");
        assert_eq!(cell.fg, qc(theme().muted_fg));
    }

    #[test]
    fn narrow_rect_drops_clear_then_icon_when_painting() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("hi")
            .with_clearable(true)
            .with_leading_icon(Icon::new("S", "S"));
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);
        assert!(layout.clear_rect.is_none());
        assert!(layout.icon_rect.is_none());
        // Only the brackets + one text column fit; the painted glyph at
        // the sole text column is the value's first (and only visible)
        // character.
        assert_eq!(buf[(1, 0)].symbol(), "h");
    }

    #[test]
    fn cursor_scrolled_into_view_paints_in_the_last_text_column() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("0123456789")
            .with_cursor(Some(10))
            .with_has_focus(true);
        let area = Rect::new(0, 0, 10, 1); // 8 text cols after brackets
        let mut buf = Buffer::empty(area);
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);
        assert_eq!(layout.resolved_scroll_chars, 3);
        // Visible window is "3456789" clipped to 8 cols -> "3456789" (7
        // chars, cursor sits one past the last char at the 8th column).
        assert_eq!(buf[(1, 0)].symbol(), "3");
    }
}
