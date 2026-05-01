//! TUI rasteriser for [`crate::ListView`].
//!
//! Per D6: this function asks the primitive for a [`crate::ListViewLayout`]
//! (one cell per item; title row 1 cell when present) and paints the
//! resolved positions verbatim. Apps that need their own measurer
//! (variable-height items, e.g.) can compute the layout externally —
//! this rasteriser computes it inline because TUI list rows are
//! always uniform 1 cell tall.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::list::{ListItemMeasure, ListView};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`ListView`] into `area` on `buf`. Honours
/// [`ListView::bordered`] (rounded box border + title overlay) and
/// [`ListView::has_focus`] (the selected row only highlights when the
/// list has focus).
///
/// # Visual contract
///
/// - **Bordered:** filled with [`Theme::surface_bg`], rounded
///   `╭─╮ │ ╰─╯` glyphs in [`Theme::border_fg`], optional title
///   centred-ish on the top border in [`Theme::title_fg`].
/// - **Non-bordered with title:** the title row is painted as a flat
///   [`Theme::header_bg`] / [`Theme::header_fg`] strip.
/// - **Selected row:** [`Theme::selected_bg`] background and a `▶`
///   selection prefix in the row's foreground.
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, others → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], skipped
///   when there isn't room past the main text.
///
/// `nerd_fonts_enabled` controls which icon variant gets painted —
/// pass `crate::icons::nerd_fonts_enabled()` from the consumer's icon
/// registry, or `false` to always use ASCII fallbacks.
pub fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    list: &ListView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hdr_fg = ratatui_color(theme.header_fg);
    let hdr_bg = ratatui_color(theme.header_bg);
    let fg = ratatui_color(theme.surface_fg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let row_bg = if list.bordered {
        ratatui_color(theme.surface_bg)
    } else {
        ratatui_color(theme.background)
    };
    let dim_fg = ratatui_color(theme.muted_fg);
    let error_fg = ratatui_color(theme.error_fg);
    let warn_fg = ratatui_color(theme.warning_fg);
    let border_fg = ratatui_color(theme.border_fg);
    let title_fg = ratatui_color(theme.title_fg);

    let title_h = if list.title.is_some() { 1.0 } else { 0.0 };
    let layout = list.layout(area.width as f32, area.height as f32, title_h, |_| {
        ListItemMeasure::new(1.0)
    });

    if list.bordered {
        let top_y = area.y;
        for col in 0..area.width {
            let cx = area.x + col;
            let ch = if col == 0 {
                '╭'
            } else if col + 1 == area.width {
                '╮'
            } else {
                '─'
            };
            set_cell(buf, cx, top_y, ch, border_fg, row_bg);
        }
        if let Some(ref title) = list.title {
            let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            let label = format!(" {} ", title_text.trim());
            for (i, ch) in label.chars().enumerate() {
                let cx = area.x + 2 + i as u16;
                if cx + 1 >= area.x + area.width {
                    break;
                }
                set_cell(buf, cx, top_y, ch, title_fg, row_bg);
            }
        }
        if area.height >= 2 {
            let bot_y = area.y + area.height - 1;
            for col in 0..area.width {
                let cx = area.x + col;
                let ch = if col == 0 {
                    '╰'
                } else if col + 1 == area.width {
                    '╯'
                } else {
                    '─'
                };
                set_cell(buf, cx, bot_y, ch, border_fg, row_bg);
            }
        }
        for row in (area.y + 1)..(area.y + area.height - 1) {
            set_cell(buf, area.x, row, '│', border_fg, row_bg);
            set_cell(buf, area.x + area.width - 1, row, '│', border_fg, row_bg);
            for col in 1..(area.width - 1) {
                set_cell(buf, area.x + col, row, ' ', fg, row_bg);
            }
        }
    } else if let Some(title_bounds) = layout.title_bounds {
        if let Some(ref title) = list.title {
            let y = area.y + title_bounds.y.round() as u16;
            for x in area.x..area.x + area.width {
                set_cell(buf, x, y, ' ', hdr_fg, hdr_bg);
            }
            draw_styled_text(
                buf,
                area,
                y,
                1,
                title,
                hdr_fg,
                hdr_bg,
                Decoration::Normal,
                dim_fg,
            );
        }
    }

    for visible_item in &layout.visible_items {
        let item = &list.items[visible_item.item_idx];
        let y = area.y + visible_item.bounds.y.round() as u16;
        let item_x = area.x + visible_item.bounds.x.round() as u16;
        let item_w = visible_item.bounds.width.round() as u16;
        let item_area = Rect {
            x: item_x,
            y,
            width: item_w,
            height: 1,
        };
        let is_selected = visible_item.item_idx == list.selected_idx && list.has_focus;
        let bg = if is_selected { sel_bg } else { row_bg };
        let decoration_fg = match item.decoration {
            Decoration::Error => error_fg,
            Decoration::Warning => warn_fg,
            Decoration::Muted => dim_fg,
            _ => fg,
        };

        for x in item_x..item_x + item_w {
            set_cell(buf, x, y, ' ', decoration_fg, bg);
        }

        let mut col = 0u16;

        let prefix = if is_selected { "▶ " } else { "  " };
        for ch in prefix.chars() {
            if col >= item_w {
                break;
            }
            set_cell(buf, item_x + col, y, ch, decoration_fg, bg);
            col += 1;
        }

        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            for ch in glyph.chars() {
                if col >= item_w {
                    break;
                }
                set_cell(buf, item_x + col, y, ch, decoration_fg, bg);
                col += 1;
            }
            if col < item_w {
                set_cell(buf, item_x + col, y, ' ', decoration_fg, bg);
                col += 1;
            }
        }

        let text_end_col = draw_styled_text(
            buf,
            item_area,
            y,
            col as usize,
            &item.text,
            decoration_fg,
            bg,
            item.decoration,
            dim_fg,
        );

        if let Some(ref detail) = item.detail {
            let detail_w: usize = detail.spans.iter().map(|s| s.text.chars().count()).sum();
            let start = (item_w as usize).saturating_sub(detail_w + 1);
            if start > text_end_col + 1 {
                draw_styled_text(
                    buf,
                    item_area,
                    y,
                    start,
                    detail,
                    dim_fg,
                    bg,
                    Decoration::Muted,
                    dim_fg,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListView};
    use crate::types::{Color, StyledSpan, StyledText, WidgetId};

    fn item(text: &str, dec: Decoration) -> ListItem {
        ListItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(text)],
            },
            detail: None,
            icon: None,
            decoration: dec,
        }
    }

    fn make_list(selected: usize) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![
                item("alpha", Decoration::Normal),
                item("beta", Decoration::Normal),
                item("gamma", Decoration::Normal),
            ],
            selected_idx: selected,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_three_items_with_selection_marker() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let list = make_list(1);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Selection marker '▶' is on row 1 (the second item).
        assert_eq!(cell_char(&buf, 0, 1), '▶');
        // First and third rows show ' ' selection placeholder.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 0, 2), ' ');
    }

    #[test]
    fn no_selection_marker_when_unfocused() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(1);
        list.has_focus = false;
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );
        // Row 1 should NOT have the '▶' marker.
        for y in 0..3 {
            assert_ne!(cell_char(&buf, 0, y), '▶');
        }
    }

    #[test]
    fn bordered_paints_corner_glyphs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_list(0);
        list.bordered = true;
        list.title = Some(StyledText {
            spans: vec![StyledSpan::plain("Picker")],
        });
        draw_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        assert_eq!(cell_char(&buf, 0, 0), '╭');
        assert_eq!(cell_char(&buf, 9, 0), '╮');
        assert_eq!(cell_char(&buf, 0, 4), '╰');
        assert_eq!(cell_char(&buf, 9, 4), '╯');
    }

    #[test]
    fn decoration_error_uses_error_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![item("oops", Decoration::Error)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
        };
        let theme = Theme {
            error_fg: Color::rgb(255, 0, 0),
            ..Theme::default()
        };
        draw_list(&mut buf, Rect::new(0, 0, 20, 3), &list, &theme, false);
        // The 'o' of "oops" should be drawn in error_fg.
        // Selection marker is ' ' (no focus); icon prefix runs cols 0..2;
        // text starts at col 2.
        assert_eq!(cell_char(&buf, 2, 0), 'o');
        let fg = buf[(2u16, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(255, 0, 0));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = make_list(0);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 0, 5),
            &list,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
