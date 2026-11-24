//! TUI entry points for [`crate::Spinner`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::spinner::cell_paint`,
//! which documents the glyph-plus-label rules. This module only adapts
//! a raw ratatui [`Buffer`] into the [`crate::CellSurface`] that painter
//! takes, and converts a ratatui cell [`Rect`] into the quadraui one the
//! widget's own cell layout takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitives::spinner::{cell_paint, cell_spinner_layout, Spinner, SpinnerLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`Spinner`] without painting.
pub fn tui_spinner_layout(spinner: &Spinner, area: Rect) -> SpinnerLayout {
    cell_spinner_layout(
        spinner,
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
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
    draw_spinner_layout(buf, spinner, &layout, theme);
    layout
}

/// Draw a [`Spinner`] into `layout.bounds` on `buf`.
///
/// `layout` must be the same [`SpinnerLayout`] the host hit-tests
/// against (typically [`tui_spinner_layout`]'s return value) so paint
/// and hit-testing can never disagree.
pub(crate) fn draw_spinner_layout(
    buf: &mut Buffer,
    spinner: &Spinner,
    layout: &SpinnerLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(spinner, layout, &mut surface, theme);
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

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_spinner_layout` bakes `area.x`/`area.y`
    // straight into `spinner.layout`'s returned bounds (absolute
    // frame), so the regression guard is a full paint+hit_test round
    // trip re-run at a non-zero origin.
    fn glyph_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("Loading", 0);
        let layout = draw_spinner(&mut buf, area, &spinner, &Theme::default());

        assert_eq!(cell_char(&buf, origin_x, origin_y), '⠋');
        assert_eq!(cell_char(&buf, origin_x + 2, origin_y), 'L');

        let hit = layout.hit_test(origin_x as f32 + 0.5, origin_y as f32 + 0.5, &spinner.id);
        assert_eq!(hit, SpinnerHit::Body(WidgetId::new("spin")));
    }

    #[test]
    fn glyph_paint_and_hit_round_trip() {
        glyph_round_trip_at(0, 0);
    }

    /// The same glyph paint + hit_test round trip must hold when the
    /// spinner isn't painted at the screen origin.
    #[test]
    fn glyph_paint_and_hit_round_trip_at_nonzero_origin() {
        glyph_round_trip_at(7, 13);
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

    /// An area with columns but no rows has nowhere to put the glyph
    /// row, so it paints nothing — the height half of the no-op above,
    /// which a width-only clamp would have let through.
    #[test]
    fn zero_height_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let spinner = mk_spinner("Test", 0);
        let layout = draw_spinner(
            &mut buf,
            Rect::new(0, 0, 10, 0),
            &spinner,
            &Theme::default(),
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(layout.bounds.height, 0.0);
    }
}
