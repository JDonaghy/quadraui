#![cfg(feature = "tui")]
//! `quadraui::tui::draw_float` with `Float::transparent` set: the
//! background fill is skipped, so cells the host painted beneath the
//! float survive everywhere the float puts no border glyph.

use quadraui::layout::{Anchor, Side};
use quadraui::tui::draw_float;
use quadraui::{Float, FloatLayout, FloatMeasure, Rect, Theme, WidgetId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;

fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
    buf.cell((x, y)).unwrap().symbol().chars().next().unwrap()
}

fn transparent_float(border: bool) -> (Float, FloatLayout) {
    let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
    let mut float = Float::new(WidgetId::new("f"), anchor);
    float.transparent = true;
    float.border = border;
    let layout = float.layout(
        Rect::new(0.0, 0.0, 40.0, 20.0),
        FloatMeasure::new(10.0, 4.0),
    );
    (float, layout)
}

fn bounds(layout: &FloatLayout) -> (u16, u16, u16, u16) {
    let b = layout.bounds;
    (
        b.x.round() as u16,
        b.y.round() as u16,
        b.width.round() as u16,
        b.height.round() as u16,
    )
}

#[test]
fn transparent_float_leaves_underlying_cells_outside_the_border_untouched() {
    let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
    let (float, layout) = transparent_float(true);
    let (x, y, w, h) = bounds(&layout);

    // The host's base layer, painted earlier in the frame, under the
    // float's interior.
    buf.cell_mut((x + 1, y + 1)).unwrap().set_char('*');

    draw_float(&mut buf, &float, &layout, &Theme::default());

    assert_eq!(cell_char(&buf, x, y), '┌');
    assert_eq!(cell_char(&buf, x + w - 1, y), '┐');
    assert_eq!(cell_char(&buf, x, y + h - 1), '└');
    assert_eq!(cell_char(&buf, x + w - 1, y + h - 1), '┘');
    assert_eq!(cell_char(&buf, x + 1, y + 1), '*');
}

#[test]
fn transparent_borderless_float_paints_nothing() {
    let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
    let (float, layout) = transparent_float(false);
    let (x, y, _, _) = bounds(&layout);
    buf.cell_mut((x, y)).unwrap().set_char('*');

    draw_float(&mut buf, &float, &layout, &Theme::default());

    assert_eq!(cell_char(&buf, x, y), '*');
}
