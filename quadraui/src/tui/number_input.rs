//! TUI rasteriser for [`crate::NumberInput`].
//!
//! Paints a 1-cell border around the whole control, the text area's
//! current value (or draft text while editing), and `+`/`-` glyphs in
//! the increment/decrement stepper cells. Mirrors the field/border/fg
//! color rules used by [`crate::primitives::number_input::native_surface_paint`].

use ratatui::buffer::Buffer;

use super::{qc, set_cell};
use crate::primitives::number_input::{NumberInput, NumberInputLayout};
use crate::theme::Theme;

fn display_text(number_input: &NumberInput) -> String {
    match &number_input.draft {
        Some(draft) => draft.clone(),
        None => format!("{:.*}", number_input.precision as usize, number_input.value),
    }
}

fn fill_rect(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    ch: char,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    for y in y0..y0.saturating_add(h) {
        for x in x0..x0.saturating_add(w) {
            set_cell(buf, x, y, ch, fg, bg);
        }
    }
}

fn stroke_rect(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }
    let x1 = x0 + w.saturating_sub(1);
    let y1 = y0 + h.saturating_sub(1);
    for x in x0..=x1 {
        set_cell(buf, x, y0, '─', fg, bg);
        if y1 != y0 {
            set_cell(buf, x, y1, '─', fg, bg);
        }
    }
    for y in y0..=y1 {
        set_cell(buf, x0, y, '│', fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y, '│', fg, bg);
        }
    }
    set_cell(buf, x0, y0, '┌', fg, bg);
    if x1 != x0 {
        set_cell(buf, x1, y0, '┐', fg, bg);
    }
    if y1 != y0 {
        set_cell(buf, x0, y1, '└', fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y1, '┘', fg, bg);
        }
    }
}

fn draw_text(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    for (i, ch) in text.chars().take(w as usize).enumerate() {
        set_cell(buf, x0 + i as u16, y0, ch, fg, bg);
    }
}

/// Draw a [`NumberInput`] at its resolved layout.
pub fn draw_number_input(
    buf: &mut Buffer,
    number_input: &NumberInput,
    layout: &NumberInputLayout,
    theme: &Theme,
) {
    if layout.bounds.width <= 0.0 || layout.bounds.height <= 0.0 {
        return;
    }

    let bg = qc(theme.background);
    let input_bg = qc(theme.input_bg);
    let border = if number_input.disabled {
        qc(theme.border_fg)
    } else if number_input.focused {
        qc(theme.accent_fg)
    } else {
        qc(theme.border_fg)
    };
    let fg = if number_input.disabled {
        qc(theme.muted_fg)
    } else {
        qc(theme.surface_fg)
    };

    fill_rect(buf, layout.text_area, ' ', fg, input_bg);
    stroke_rect(buf, layout.bounds, border, bg);

    let text = display_text(number_input);
    if !text.is_empty() {
        draw_text(buf, layout.text_area, &text, fg, input_bg);
    }

    stroke_rect(buf, layout.increment, border, bg);
    stroke_rect(buf, layout.decrement, border, bg);
    draw_text(buf, layout.increment, "+", fg, bg);
    draw_text(buf, layout.decrement, "-", fg, bg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Point, Rect};
    use crate::primitives::number_input::NumberInputHit;
    use crate::types::WidgetId;
    use ratatui::layout::Rect as RRect;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_number_input() -> NumberInput {
        NumberInput {
            id: WidgetId::new("num"),
            value: 3.0,
            min: 0.0,
            max: 10.0,
            step: 1.0,
            precision: 0,
            draft: None,
            disabled: false,
            focused: true,
        }
    }

    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let bounds = Rect::new(origin_x as f32, origin_y as f32, 10.0, 3.0);
        let number_input = mk_number_input();
        let layout = number_input.layout(bounds);

        let rect = RRect::new(origin_x, origin_y, 10, 3);
        let mut buf = Buffer::empty(rect);
        draw_number_input(&mut buf, &number_input, &layout, &Theme::default());

        // The value renders inside the text area, painted on top of the
        // border (matching `native_surface_paint`'s fill -> stroke -> text
        // layering, where the text run is drawn last).
        let text_x = layout.text_area.x as u16;
        let text_y = layout.text_area.y as u16;
        assert_eq!(cell_char(&buf, text_x, text_y), '3');

        // The border itself is still visible on an edge the text never
        // reaches (the bottom-left corner, one row below the text).
        let border_y = origin_y + layout.bounds.height.round() as u16 - 1;
        assert_eq!(cell_char(&buf, origin_x, border_y), '└');

        // hit_test: a click inside the increment sub-rect resolves to
        // the Increment variant at the absolute screen position.
        let p = Point::new(
            layout.increment.x + layout.increment.width / 2.0,
            layout.increment.y + layout.increment.height / 2.0,
        );
        assert_eq!(layout.hit_test(p), NumberInputHit::Increment);

        let p = Point::new(
            layout.decrement.x + layout.decrement.width / 2.0,
            layout.decrement.y + layout.decrement.height / 2.0,
        );
        assert_eq!(layout.hit_test(p), NumberInputHit::Decrement);
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Paint and hit-test must work correctly when the control is not at
    /// the screen origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }
}
