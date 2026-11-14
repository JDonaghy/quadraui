#![cfg(feature = "tui")]
//! A layout-tree `Button` painted into a cell taller than its one row of
//! chrome, then covered by the generic focus ring a click gives it, keeps
//! its label visible: the ring only paints the cell's edge rows and the
//! button centres its label on an interior row.

use quadraui::tui::{draw_button, draw_focus_ring};
use quadraui::{Button, Theme, WidgetId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[test]
fn focus_ring_over_a_tall_button_cell_keeps_the_label() {
    let theme = Theme::default();
    let rect = Rect::new(0, 0, 8, 3);
    let mut buf = Buffer::empty(rect);
    let button = Button::new(WidgetId::new("b"), "OK");
    draw_button(&mut buf, rect, &button, &theme, false);
    draw_focus_ring(&mut buf, rect, &theme);

    let screen: String = (0..rect.height)
        .flat_map(|y| (0..rect.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        screen.contains("OK"),
        "the label must survive the focus ring, got {screen:?}"
    );
}
