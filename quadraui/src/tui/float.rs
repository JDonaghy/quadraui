//! TUI rasteriser for [`crate::Float`].
//!
//! Paints the float's chrome only: a filled background box, and —
//! when `float.border` is set — a square-corner border
//! (`┌─┐`/`│`/`└─┘`, the same glyph set `tui::tooltip`/
//! `tui::context_menu` already use). Content is the host's
//! responsibility — it draws into `layout.content_bounds`, same
//! contract as [`crate::tui::draw_panel`].

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::float::{Float, FloatLayout};
use crate::theme::Theme;

/// Draw a [`Float`]'s chrome into `buf` at `layout.bounds`. `layout`
/// must be the same [`FloatLayout`] the host uses for hit-testing
/// (typically the value `float.layout(viewport, measure)` returned) so
/// paint and click-routing can never disagree.
pub fn draw_float(buf: &mut Buffer, float: &Float, layout: &FloatLayout, theme: &Theme) {
    let bg = float
        .bg
        .map(ratatui_color)
        .unwrap_or(ratatui_color(theme.surface_bg));
    let border_fg = ratatui_color(theme.border_fg);

    let x = layout.bounds.x.round() as u16;
    let y = layout.bounds.y.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let h = layout.bounds.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }

    // Fill the whole box with the background colour first.
    for row in y..y.saturating_add(h) {
        for col in x..x.saturating_add(w) {
            set_cell(buf, col, row, ' ', bg, bg);
        }
    }

    if !float.border {
        return;
    }
    if w < 2 || h < 2 {
        // Too small for a full box — leave the flat fill in place
        // rather than drawing a border that would overwrite the whole
        // thing with corner glyphs.
        return;
    }

    for col in 0..w {
        let ch = if col == 0 {
            '┌'
        } else if col == w - 1 {
            '┐'
        } else {
            '─'
        };
        set_cell(buf, x + col, y, ch, border_fg, bg);
    }
    for row in (y + 1)..(y + h - 1) {
        set_cell(buf, x, row, '│', border_fg, bg);
        set_cell(buf, x + w - 1, row, '│', border_fg, bg);
    }
    for col in 0..w {
        let ch = if col == 0 {
            '└'
        } else if col == w - 1 {
            '┘'
        } else {
            '─'
        };
        set_cell(buf, x + col, y + h - 1, ch, border_fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Point, Rect};
    use crate::layout::{Anchor, Side};
    use crate::primitives::float::FloatMeasure;
    use crate::types::WidgetId;
    use ratatui::layout::Rect as RRect;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf.cell((x, y)).unwrap().symbol().chars().next().unwrap()
    }

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn draws_bordered_box_at_resolved_bounds() {
        let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
        let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
        let float = Float::new(WidgetId::new("f"), anchor);
        let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
        draw_float(&mut buf, &float, &layout, &theme());

        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        let w = layout.bounds.width.round() as u16;
        let h = layout.bounds.height.round() as u16;
        assert_eq!(cell_char(&buf, x, y), '┌');
        assert_eq!(cell_char(&buf, x + w - 1, y), '┐');
        assert_eq!(cell_char(&buf, x, y + h - 1), '└');
        assert_eq!(cell_char(&buf, x + w - 1, y + h - 1), '┘');
        assert_eq!(cell_char(&buf, x, y + 1), '│');
    }

    #[test]
    fn borderless_float_paints_flat_fill_with_no_box_glyphs() {
        let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
        let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
        let mut float = Float::new(WidgetId::new("f"), anchor);
        float.border = false;
        let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
        draw_float(&mut buf, &float, &layout, &theme());

        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, x, y), ' ');
    }

    /// Paint/click round-trip (`docs/PRIMITIVE_RULES.md` rule 4): find
    /// the painted top-left border glyph, hit-test that exact cell
    /// against `layout.hit_test`, and assert both agree the box starts
    /// there.
    #[test]
    fn paint_and_hit_test_agree_on_the_box_bounds() {
        let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
        let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
        let float = Float::new(WidgetId::new("f"), anchor);
        let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
        draw_float(&mut buf, &float, &layout, &theme());

        // Find the painted top-left corner glyph by scanning the buffer —
        // not by re-deriving it from `layout` (that would just check
        // layout against itself).
        let mut found: Option<(u16, u16)> = None;
        for row in 0..20u16 {
            for col in 0..40u16 {
                if cell_char(&buf, col, row) == '┌' {
                    found = Some((col, row));
                }
            }
        }
        let (fx, fy) = found.expect("top-left border glyph must have painted");

        let p = Point::new(fx as f32, fy as f32);
        assert_eq!(
            layout.hit_test(p),
            crate::primitives::float::FloatHit::Body,
            "the painted top-left cell must hit-test as inside the float"
        );
        // One cell outside (up-left) must miss.
        let outside = Point::new(fx as f32 - 1.0, fy as f32 - 1.0);
        assert_eq!(
            layout.hit_test(outside),
            crate::primitives::float::FloatHit::Outside
        );
    }
}
