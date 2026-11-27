//! A whole-cell `Canvas` `Rect` on TUI paints solid cells, driven through
//! the real `TuiDriver` frame path: an opaque fill blanks its cells and a
//! translucent one (a modal scrim) dims the background under existing
//! text without erasing it.

#![cfg(feature = "tui")]

use quadraui::testing::solid_canvas;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Canvas, Color, DrawOp, DrawsPrimitives, Reaction, Rect, UiEvent, WidgetId,
};
use ratatui::style::Color as RColor;

const BASE: Color = Color::rgb(20, 22, 30);
const TEXT_FG: Color = Color::rgb(240, 240, 240);

struct ScrimApp {
    scrim: Color,
}

impl AppLogic for ScrimApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let row = Rect::new(0.0, 0.0, 20.0, 1.0);
        backend.draw_dyn(row, &solid_canvas(row, BASE), &());
        let label = Canvas {
            id: WidgetId::new("label"),
            ops: vec![DrawOp::TextRun {
                rect: Rect::new(2.0, 0.0, 10.0, 1.0),
                text: "UNDERNEATH".into(),
                color: TEXT_FG,
            }],
            regions: Vec::new(),
            focusable: false,
        };
        backend.draw_dyn(row, &label, &());
        backend.draw_dyn(row, &solid_canvas(row, self.scrim), &());
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn rgb(c: Color) -> RColor {
    RColor::Rgb(c.r, c.g, c.b)
}

#[test]
fn translucent_fill_dims_the_background_and_keeps_the_text() {
    let scrim = Color::rgba(0, 0, 0, 102);
    let driver = TuiDriver::new(ScrimApp { scrim }, 20, 1);
    let screen = driver.screen();
    assert!(
        screen.contains("UNDERNEATH"),
        "a translucent fill must leave the text under it: {screen:?}"
    );
    let cell = driver.style_at(2, 0).expect("in-bounds");
    assert_eq!(cell.fg, rgb(TEXT_FG), "the glyph keeps its own colour");
    let label_bg = driver.style_at(0, 0).expect("in-bounds").bg;
    assert_ne!(
        label_bg,
        rgb(BASE),
        "the background under the fill is dimmed"
    );
}

#[test]
fn opaque_fill_blanks_its_cells() {
    let cover = Color::rgb(9, 9, 9);
    let driver = TuiDriver::new(ScrimApp { scrim: cover }, 20, 1);
    let screen = driver.screen();
    assert!(
        screen.trim().is_empty(),
        "an opaque fill clears every glyph under it: {screen:?}"
    );
    let cell = driver.style_at(5, 0).expect("in-bounds");
    assert_eq!(cell.bg, rgb(cover));
    assert_eq!(cell.fg, rgb(cover));
}
