//! TUI entry points for [`crate::ProgressBar`] — thin delegates onto
//! the widget's own cell painter,
//! `crate::primitives::progress::cell_paint`, which documents the
//! track/fill/label/cancel rules. This module only adapts a raw ratatui
//! [`Buffer`] into the [`crate::CellSurface`] that painter takes, and
//! converts a ratatui cell [`Rect`] into the quadraui one the widget's
//! own cell layout takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitives::progress::{
    cell_paint, cell_progress_layout, ProgressBar, ProgressBarLayout,
};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`ProgressBar`] without painting.
pub fn tui_progress_layout(bar: &ProgressBar, area: Rect) -> ProgressBarLayout {
    cell_progress_layout(
        bar,
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
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
    draw_progress_layout(buf, bar, &layout, theme);
    layout
}

/// Draw a [`ProgressBar`] into `layout.bounds` on `buf`.
///
/// `layout` must be the same [`ProgressBarLayout`] the host hit-tests
/// against (typically [`tui_progress_layout`]'s return value) so paint
/// and click-routing can never disagree.
pub(crate) fn draw_progress_layout(
    buf: &mut Buffer,
    bar: &ProgressBar,
    layout: &ProgressBarLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(bar, layout, &mut surface, theme);
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

    // Shared bodies parametrized over the area's origin — LESSONS.md
    // "Layout helpers must return coords in the same frame across
    // backends". `tui_progress_layout` bakes `area.x`/
    // `area.y` straight into `bar.layout`'s returned bounds (absolute
    // frame), so the regression guard is a full paint+hit_test round
    // trip re-run at a non-zero origin, not a relative-coords check.

    fn determinate_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = det_bar(0.5);
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        // Fill should cover ~10 cells (50% of 20) starting at the area's origin.
        let fb = layout.fill_bounds.expect("fill bounds present");
        let fill_end = (fb.x + fb.width).round() as u16;
        assert_eq!(fill_end, origin_x + 10);
        assert_eq!(cell_char(&buf, origin_x, origin_y), '█');
        assert_eq!(cell_char(&buf, origin_x + 9, origin_y), '█');
        assert_eq!(cell_char(&buf, origin_x + 10, origin_y), '░');

        let hit = layout.hit_test(origin_x as f32 + 5.0, origin_y as f32 + 0.5);
        assert_eq!(hit, ProgressBarHit::Body(WidgetId::new("prog")));
    }

    #[test]
    fn determinate_fill_paint_and_click_round_trip() {
        determinate_round_trip_at(0, 0);
    }

    /// The same fill+hit_test round trip must hold when the bar isn't
    /// painted at the screen origin.
    #[test]
    fn determinate_fill_paint_and_click_round_trip_at_nonzero_origin() {
        determinate_round_trip_at(7, 13);
    }

    fn cancel_button_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 20, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = det_bar(0.5);
        bar.cancellable = true;
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        let cb = layout.cancel_bounds.expect("cancel bounds present");
        let cx = cb.x.round() as u16 + 1;
        assert_eq!(cell_char(&buf, cx, origin_y), '×');

        let hit = layout.hit_test(cx as f32 + 0.5, origin_y as f32 + 0.5);
        assert_eq!(hit, ProgressBarHit::Cancel(WidgetId::new("prog")));
    }

    #[test]
    fn cancel_button_paint_and_click_round_trip() {
        cancel_button_round_trip_at(0, 0);
    }

    /// Non-zero-origin round-trip invariant: the same fill+hit_test must hold at any origin.
    #[test]
    fn cancel_button_paint_and_click_round_trip_at_nonzero_origin() {
        cancel_button_round_trip_at(7, 13);
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
