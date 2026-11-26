#![cfg(feature = "tui")]
//! Painting a [`Float`] with `Float::transparent` set: the background
//! fill is skipped, so cells the host painted beneath the float survive
//! everywhere the float puts no border glyph.

use quadraui::layout::{Anchor, Side};
use quadraui::tui::TuiBackend;
use quadraui::{DrawsPrimitives, Float, FloatLayout, FloatMeasure, Rect, WidgetId};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

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

/// Paint `base` (the host's own earlier layer) and then the float, and
/// hand back the resulting terminal buffer — the real
/// `enter_frame_scope` -> `draw_dyn` path a TUI host uses, so the
/// transparency assertions below are made against cells a backend
/// actually wrote.
fn painted(float: &Float, layout: &FloatLayout, base: &[(u16, u16, char)]) -> Buffer {
    let mut backend = TuiBackend::new();
    let mut terminal = Terminal::new(TestBackend::new(40, 20)).expect("test terminal");
    terminal
        .draw(|frame| {
            for (x, y, ch) in base {
                frame.buffer_mut().cell_mut((*x, *y)).unwrap().set_char(*ch);
            }
            backend.enter_frame_scope(frame, |b| {
                b.draw_dyn(layout.bounds, float, &());
            });
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

#[test]
fn transparent_float_leaves_underlying_cells_outside_the_border_untouched() {
    let (float, layout) = transparent_float(true);
    let (x, y, w, h) = bounds(&layout);

    let buf = painted(&float, &layout, &[(x + 1, y + 1, '*')]);

    assert_eq!(cell_char(&buf, x, y), '┌');
    assert_eq!(cell_char(&buf, x + w - 1, y), '┐');
    assert_eq!(cell_char(&buf, x, y + h - 1), '└');
    assert_eq!(cell_char(&buf, x + w - 1, y + h - 1), '┘');
    assert_eq!(cell_char(&buf, x + 1, y + 1), '*');
}

#[test]
fn transparent_borderless_float_paints_nothing() {
    let (float, layout) = transparent_float(false);
    let (x, y, _, _) = bounds(&layout);

    let buf = painted(&float, &layout, &[(x, y, '*')]);

    assert_eq!(cell_char(&buf, x, y), '*');
}
