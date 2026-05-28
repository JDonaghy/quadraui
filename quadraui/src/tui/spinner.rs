//! TUI rasteriser for [`crate::Spinner`].
//!
//! Paints a braille animation glyph + label. The app advances
//! `frame_idx` on its own ticker; we render `FRAMES[frame_idx % N]`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
use crate::theme::Theme;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Compute the TUI cell-unit layout for a [`Spinner`] without painting.
pub fn tui_spinner_layout(spinner: &Spinner, area: Rect) -> SpinnerLayout {
    let label_w = if spinner.label.is_empty() {
        0
    } else {
        spinner.label.chars().count() + 1
    };
    let total_w = (1 + label_w).min(area.width as usize);
    spinner.layout(
        area.x as f32,
        area.y as f32,
        SpinnerMeasure::new(total_w as f32, 1.0),
    )
}

/// Draw a [`Spinner`] into `area` on `buf`. Returns the layout for
/// host hit-testing.
pub fn draw_spinner(
    buf: &mut Buffer,
    area: Rect,
    spinner: &Spinner,
    theme: &Theme,
) -> SpinnerLayout {
    let layout = tui_spinner_layout(spinner, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let x = layout.bounds.x.round() as u16;
    let y = layout.bounds.y.round() as u16;

    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    let glyph_fg = spinner
        .accent
        .map(ratatui_color)
        .unwrap_or_else(|| ratatui_color(theme.accent_fg));
    let bg = ratatui_color(theme.background);

    set_cell(buf, x, y, glyph, glyph_fg, bg);

    if !spinner.label.is_empty() {
        let fg = ratatui_color(theme.foreground);
        for (col, ch) in (x + 2..).zip(spinner.label.chars()) {
            if col >= area.x + area.width {
                break;
            }
            set_cell(buf, col, y, ch, fg, bg);
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::spinner::SpinnerHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_spinner(label: &str, frame: usize) -> Spinner {
        Spinner {
            id: WidgetId::new("spin"),
            label: label.into(),
            frame_idx: frame,
            accent: None,
        }
    }

    #[test]
    fn glyph_paint_and_hit_round_trip() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("Loading", 0);
        let layout = draw_spinner(&mut buf, area, &spinner, &Theme::default());

        assert_eq!(cell_char(&buf, 0, 0), '⠋');
        assert_eq!(cell_char(&buf, 2, 0), 'L');

        let hit = layout.hit_test(0.5, 0.5, &spinner.id);
        assert_eq!(hit, SpinnerHit::Body(WidgetId::new("spin")));
    }

    #[test]
    fn frame_advances_glyph() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("", 3);
        draw_spinner(&mut buf, area, &spinner, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), '⠸');
    }

    #[test]
    fn outside_returns_empty() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("Test", 0);
        let layout = draw_spinner(&mut buf, area, &spinner, &Theme::default());
        assert_eq!(layout.hit_test(50.0, 0.5, &spinner.id), SpinnerHit::Empty);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let spinner = mk_spinner("Test", 0);
        let _layout = draw_spinner(&mut buf, Rect::new(0, 0, 0, 0), &spinner, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
