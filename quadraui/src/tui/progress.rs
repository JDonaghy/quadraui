//! TUI rasteriser for [`crate::ProgressBar`].
//!
//! Paints a horizontal bar with filled portion, optional label, and
//! optional cancel `×` affordance.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout, ProgressBarMeasure};
use crate::theme::Theme;

const TUI_CANCEL_WIDTH: f32 = 3.0;

/// Compute the TUI cell-unit layout for a [`ProgressBar`] without painting.
pub fn tui_progress_layout(bar: &ProgressBar, area: Rect) -> ProgressBarLayout {
    let cancel_w = if bar.cancellable {
        TUI_CANCEL_WIDTH
    } else {
        0.0
    };
    bar.layout(
        area.x as f32,
        area.y as f32,
        ProgressBarMeasure {
            width: area.width as f32,
            height: area.height.min(1) as f32,
            cancel_width: cancel_w,
        },
    )
}

/// Draw a [`ProgressBar`] into `area` on `buf`. Returns the layout
/// for host click dispatch.
pub fn draw_progress(
    buf: &mut Buffer,
    area: Rect,
    bar: &ProgressBar,
    theme: &Theme,
) -> ProgressBarLayout {
    let layout = tui_progress_layout(bar, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let y = layout.bounds.y.round() as u16;
    let bx = layout.bounds.x.round() as u16;
    let bw = layout.bounds.width.round() as u16;

    let fill_color = bar
        .accent
        .map(ratatui_color)
        .unwrap_or_else(|| ratatui_color(theme.accent_bg));
    let track_bg = ratatui_color(theme.surface_bg);
    let fg = ratatui_color(theme.foreground);

    // Paint track background.
    let bar_end = if let Some(cb) = layout.cancel_bounds {
        cb.x.round() as u16
    } else {
        bx + bw
    };
    for dx in bx..bar_end {
        set_cell(buf, dx, y, '░', fg, track_bg);
    }

    // Paint fill.
    if let Some(fb) = layout.fill_bounds {
        let fill_end = (fb.x + fb.width).round() as u16;
        for dx in bx..fill_end.min(bar_end) {
            set_cell(buf, dx, y, '█', fill_color, fill_color);
        }
    } else {
        // Indeterminate: sliding 3-cell block.
        let track_w = (bar_end - bx) as usize;
        if track_w > 0 {
            let pulse_w = 3.min(track_w);
            let pos = bar.frame_idx % (track_w.max(1));
            for i in 0..pulse_w {
                let dx = bx + ((pos + i) % track_w) as u16;
                if dx < bar_end {
                    set_cell(buf, dx, y, '█', fill_color, fill_color);
                }
            }
        }
    }

    // Label overlay (left-aligned, 1 cell padding).
    if !bar.label.is_empty() {
        let mut col = bx + 1;
        for ch in bar.label.chars() {
            if col >= bar_end {
                break;
            }
            let existing_bg = buf[(col, y)].bg;
            set_cell(buf, col, y, ch, fg, existing_bg);
            col += 1;
        }
    }

    // Cancel affordance.
    if let Some(cb) = layout.cancel_bounds {
        let cx = cb.x.round() as u16 + 1;
        if cx < bx + bw {
            set_cell(buf, cx, y, '×', fg, track_bg);
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn det_bar(value: f32) -> ProgressBar {
        ProgressBar {
            id: WidgetId::new("prog"),
            label: String::new(),
            value: Some(value),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        }
    }

    #[test]
    fn determinate_fill_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = det_bar(0.5);
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        // Fill should cover ~10 cells (50% of 20).
        let fb = layout.fill_bounds.expect("fill bounds present");
        let fill_end = (fb.x + fb.width).round() as u16;
        assert_eq!(fill_end, 10);
        assert_eq!(cell_char(&buf, 0, 0), '█');
        assert_eq!(cell_char(&buf, 9, 0), '█');
        assert_eq!(cell_char(&buf, 10, 0), '░');

        let hit = layout.hit_test(5.0, 0.5);
        assert_eq!(hit, ProgressBarHit::Body(WidgetId::new("prog")));
    }

    #[test]
    fn cancel_button_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = det_bar(0.5);
        bar.cancellable = true;
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        let cb = layout.cancel_bounds.expect("cancel bounds present");
        let cx = cb.x.round() as u16 + 1;
        assert_eq!(cell_char(&buf, cx, 0), '×');

        let hit = layout.hit_test(cx as f32 + 0.5, 0.5);
        assert_eq!(hit, ProgressBarHit::Cancel(WidgetId::new("prog")));
    }

    #[test]
    fn indeterminate_paints_pulse() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = ProgressBar {
            id: WidgetId::new("prog"),
            label: String::new(),
            value: None,
            frame_idx: 5,
            cancellable: false,
            accent: None,
        };
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        assert!(layout.fill_bounds.is_none());
        // Pulse block should be at frame_idx % width.
        assert_eq!(cell_char(&buf, 5, 0), '█');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let bar = det_bar(0.5);
        let _layout = draw_progress(&mut buf, area, &bar, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn outside_hit_returns_empty() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = det_bar(0.5);
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());
        assert_eq!(layout.hit_test(25.0, 0.5), ProgressBarHit::Empty);
    }
}
