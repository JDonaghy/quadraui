//! TUI rasteriser for [`crate::MenuBar`].
//!
//! Paints a horizontal strip of menu-bar items into a single row.
//! Each item's label is rendered with optional Alt-key underline
//! (the char after `&` in the label, or the first char if no `&`).
//! Active/open items get a highlight; disabled items are dimmed.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{ratatui_color, set_cell, set_cell_styled};
use crate::primitives::menu_bar::{MenuBar, MenuBarItemMeasure, MenuBarLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`MenuBar`] without painting.
/// Consumer click routers call this to resolve mouse events against
/// the same layout the rasteriser used to paint.
pub fn tui_menu_bar_layout(bar: &MenuBar, area: Rect) -> MenuBarLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    bar.layout(bounds, |i| {
        let w = display_width(&bar.items[i].label) + 2; // 1-cell padding each side
        MenuBarItemMeasure::new(w as f32)
    })
}

/// Draw a [`MenuBar`] into `area` on `buf`. Returns the layout for
/// host click dispatch via `layout.hit_test(x, y)`.
pub fn draw_menu_bar(buf: &mut Buffer, area: Rect, bar: &MenuBar, theme: &Theme) -> MenuBarLayout {
    if area.width == 0 || area.height == 0 {
        let bounds = crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        return bar.layout(bounds, |_| MenuBarItemMeasure::new(0.0));
    }

    let layout = tui_menu_bar_layout(bar, area);
    let y = area.y;

    let bar_bg = ratatui_color(theme.tab_bar_bg);
    for col in 0..area.width {
        set_cell(buf, area.x + col, y, ' ', bar_bg, bar_bg);
    }

    for vi in &layout.visible_items {
        let item = &bar.items[vi.item_idx];
        let is_active = bar.open_item == Some(vi.item_idx) || bar.focused_item == Some(vi.item_idx);

        let (fg, bg) = if is_active {
            (
                ratatui_color(theme.tab_active_fg),
                ratatui_color(theme.tab_active_bg),
            )
        } else if item.disabled {
            (ratatui_color(theme.muted_fg), bar_bg)
        } else {
            (ratatui_color(theme.tab_inactive_fg), bar_bg)
        };

        let start_x = area.x + vi.bounds.x.round() as u16;
        let end_x = start_x + vi.bounds.width.round() as u16;

        if is_active {
            for col in start_x..end_x.min(area.x + area.width) {
                set_cell(buf, col, y, ' ', fg, bg);
            }
        }

        let underline_pos = alt_char_index(&item.label);
        let mut cx = start_x + 1; // 1-cell left padding
        let mut char_idx: usize = 0;
        for ch in item.label.chars() {
            if ch == '&' {
                continue;
            }
            if cx >= area.x + area.width {
                break;
            }
            if char_idx == underline_pos {
                set_cell_styled(buf, cx, y, ch, fg, bg, Modifier::UNDERLINED, None);
            } else {
                set_cell(buf, cx, y, ch, fg, bg);
            }
            cx += 1;
            char_idx += 1;
        }
    }

    layout
}

/// Display width of a label in cells (chars minus the `&` marker).
fn display_width(label: &str) -> usize {
    label.chars().filter(|&c| c != '&').count()
}

/// Index (in display chars, skipping `&`) of the Alt-activation char.
/// If `&` is present, the char after it; otherwise char 0.
fn alt_char_index(label: &str) -> usize {
    for (i, ch) in label.chars().enumerate() {
        if ch == '&' {
            return i;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::menu_bar::{MenuBar, MenuBarHit, MenuBarItem};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn make_bar() -> MenuBar {
        MenuBar {
            id: WidgetId::new("menu"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("file"),
                    label: "&File".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("edit"),
                    label: "&Edit".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("view"),
                    label: "&View".into(),
                    disabled: false,
                },
            ],
            open_item: None,
            focused_item: None,
        }
    }

    #[test]
    fn paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = make_bar();
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        for vi in &layout.visible_items {
            let mid_x = vi.bounds.x + vi.bounds.width / 2.0;
            let mid_y = vi.bounds.y + 0.5;
            let hit = layout.hit_test(mid_x, mid_y);
            assert_eq!(
                hit,
                MenuBarHit::Item(vi.item_idx),
                "hit_test at painted item {} center should return Item({})",
                vi.item_idx,
                vi.item_idx,
            );

            let label = &bar.items[vi.item_idx].label;
            let first_display_char = label.chars().find(|&c| c != '&').unwrap();
            let start_col = vi.bounds.x.round() as u16 + 1; // after padding
            assert_eq!(
                cell_char(&buf, start_col, 0),
                first_display_char,
                "first display char of item {} should be '{}' at col {}",
                vi.item_idx,
                first_display_char,
                start_col,
            );
        }
    }

    #[test]
    fn disabled_item_not_clickable() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = make_bar();
        bar.items[1].disabled = true;
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        let vi = &layout.visible_items[1];
        let mid_x = vi.bounds.x + vi.bounds.width / 2.0;
        let hit = layout.hit_test(mid_x, 0.5);
        assert_eq!(
            hit,
            MenuBarHit::Bar,
            "disabled item should not be clickable"
        );
    }

    #[test]
    fn open_item_has_active_bg() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut bar = make_bar();
        bar.open_item = Some(1);
        let layout = draw_menu_bar(&mut buf, area, &bar, &theme);

        let active_bg = ratatui_color(theme.tab_active_bg);
        let vi = &layout.visible_items[1];
        let col = vi.bounds.x.round() as u16 + 1;
        assert_eq!(
            buf[(col, 0)].bg,
            active_bg,
            "open item should have active bg"
        );

        let bar_bg = ratatui_color(theme.tab_bar_bg);
        let vi0 = &layout.visible_items[0];
        let col0 = vi0.bounds.x.round() as u16 + 1;
        assert_eq!(
            buf[(col0, 0)].bg,
            bar_bg,
            "non-open item should have bar bg"
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 1);
        let bar = make_bar();
        // Should not panic, and should not modify the buffer.
        let _layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());
        assert_eq!(
            buf[(0, 0)].symbol().chars().next().unwrap_or(' '),
            ' ',
            "buffer should be untouched"
        );
    }

    #[test]
    fn alt_char_underlined() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = make_bar(); // "&File", "&Edit", "&View"
        draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        // 'F' in "&File" is at col 1 (after 1-cell padding from x=0)
        assert!(
            buf[(1, 0)].modifier.contains(Modifier::UNDERLINED),
            "alt-key char 'F' should be underlined"
        );
        assert_eq!(cell_char(&buf, 1, 0), 'F');

        // 'i' in "&File" is not underlined
        assert!(
            !buf[(2, 0)].modifier.contains(Modifier::UNDERLINED),
            "non-alt char 'i' should not be underlined"
        );
    }
}
