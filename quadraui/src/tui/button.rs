//! TUI rasteriser for [`crate::Button`].
//!
//! Paints `[ Label ]` bracket chrome for [`ButtonVariant::Primary`] /
//! [`ButtonVariant::Secondary`], and the bare label for
//! [`ButtonVariant::Icon`] (no bracket chrome — an icon button typically
//! carries a single glyph). The whole cell range is one hit target, same
//! as [`crate::Button::layout`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::button::{Button, ButtonLayout, ButtonVariant};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`Button`] without painting.
/// The control has no internal sub-regions, so `area` maps straight
/// through to [`Button::layout`]'s bounds.
pub fn tui_button_layout(button: &Button, area: Rect) -> ButtonLayout {
    button.layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Draw a [`Button`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
pub fn draw_button(buf: &mut Buffer, area: Rect, button: &Button, theme: &Theme) -> ButtonLayout {
    let layout = tui_button_layout(button, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let y = layout.bounds.y.round() as u16;
    let x0 = layout.bounds.x.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let end = x0.saturating_add(w);

    let bg = if button.disabled {
        ratatui_color(theme.surface_bg)
    } else {
        match button.variant {
            ButtonVariant::Primary => ratatui_color(theme.accent_bg),
            ButtonVariant::Secondary => ratatui_color(theme.surface_bg),
            ButtonVariant::Icon => ratatui_color(theme.background),
        }
    };
    let fg = if button.disabled {
        ratatui_color(theme.muted_fg)
    } else {
        ratatui_color(theme.foreground)
    };
    let brk_fg = if button.disabled {
        ratatui_color(theme.muted_fg)
    } else if button.focused {
        ratatui_color(theme.accent_fg)
    } else {
        ratatui_color(theme.border_fg)
    };

    let is_icon = matches!(button.variant, ButtonVariant::Icon);
    let text: String = if is_icon {
        button.label.clone()
    } else {
        format!("[ {} ]", button.label)
    };
    let last_idx = text.chars().count().saturating_sub(1);

    let mut col = x0;
    for (i, ch) in text.chars().enumerate() {
        if col >= end {
            break;
        }
        let is_bracket = !is_icon && (i == 0 || i == last_idx);
        let ch_fg = if is_bracket { brk_fg } else { fg };
        set_cell(buf, col, y, ch, ch_fg, bg);
        col += 1;
    }
    while col < end {
        set_cell(buf, col, y, ' ', fg, bg);
        col += 1;
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::button::ButtonHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_button_layout` bakes `area.x`/`area.y` straight into
    // `Button::layout`'s returned bounds (absolute frame), so the
    // regression guard is a full paint+hit_test round trip re-run at a
    // non-zero origin.
    fn primary_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 8, 1);
        let mut buf = Buffer::empty(area);
        let button = mk_button("OK");
        let layout = draw_button(&mut buf, area, &button, &Theme::default());

        assert_eq!(cell_char(&buf, origin_x, origin_y), '[');
        assert_eq!(cell_char(&buf, origin_x + 2, origin_y), 'O');
        assert_eq!(cell_char(&buf, origin_x + 3, origin_y), 'K');
        assert_eq!(cell_char(&buf, origin_x + 5, origin_y), ']');

        let hit = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32));
        assert_eq!(hit, ButtonHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 50.0, origin_y as f32));
        assert_eq!(outside, ButtonHit::Outside);
    }

    #[test]
    fn primary_paint_and_hit_round_trip() {
        primary_round_trip_at(0, 0);
    }

    #[test]
    fn primary_paint_and_hit_round_trip_at_nonzero_origin() {
        primary_round_trip_at(7, 13);
    }

    #[test]
    fn icon_variant_paints_bare_label_without_brackets() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        let mut button = mk_button("X");
        button.variant = ButtonVariant::Icon;
        draw_button(&mut buf, area, &button, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), 'X');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
    }

    #[test]
    fn disabled_button_still_paints_but_dims() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        let mut button = mk_button("OK");
        button.disabled = true;
        let theme = Theme::default();
        draw_button(&mut buf, area, &button, &theme);
        assert_eq!(cell_char(&buf, 2, 0), 'O');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let button = mk_button("OK");
        let _layout = draw_button(&mut buf, Rect::new(0, 0, 0, 0), &button, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
