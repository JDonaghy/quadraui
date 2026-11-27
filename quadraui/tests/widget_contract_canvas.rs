#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Canvas` on the widget contract: the
//! canvas reaches the frame as a `Surface::Widget`, paints its ops through
//! the cell painter, and turns pointer gestures on its declared
//! regions into `UiEvent::Canvas` events.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Canvas, CanvasEvent, CanvasRegion, Color, DrawOp, MouseButton, Rect, UiEvent, WidgetId,
};

const COLS: u16 = 24;
const ROWS: u16 = 8;
const AREA: Rect = Rect::new(2.0, 1.0, 16.0, 4.0);

fn canvas_id() -> WidgetId {
    WidgetId::new("canvas:diagram")
}

fn canvas() -> Canvas {
    Canvas {
        id: canvas_id(),
        ops: vec![
            DrawOp::Rect {
                rect: Rect::new(0.0, 0.0, 4.0, 2.0),
                color: Color::rgb(200, 0, 0),
            },
            DrawOp::TextRun {
                rect: Rect::new(8.0, 1.0, 6.0, 1.0),
                text: "label".into(),
                color: Color::rgb(220, 220, 220),
            },
        ],
        regions: vec![CanvasRegion {
            id: WidgetId::new("box"),
            rect: Rect::new(0.0, 0.0, 4.0, 2.0),
            cursor: None,
        }],
        focusable: false,
    }
}

type CanvasApp = one_widget::OneWidgetApp<Canvas>;

fn apply(_: &mut Canvas, _: &UiEvent) -> bool {
    false
}

fn driver() -> TuiDriver<CanvasApp> {
    one_widget::driver(canvas(), AREA, COLS, ROWS, apply)
}

fn events(driver: &TuiDriver<CanvasApp>) -> Vec<CanvasEvent> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Canvas(id, ev) if *id == canvas_id() => Some(ev.clone()),
            _ => None,
        })
        .collect()
}

fn cell(driver: &TuiDriver<CanvasApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

#[test]
fn surface_widget_canvas_paints_a_whole_cell_rect_solid_and_text_on_top() {
    let driver = driver();
    let (x0, y0) = (AREA.x as u16, AREA.y as u16);
    let red = ratatui::style::Color::Rgb(200, 0, 0);
    for (x, y) in [(x0, y0), (x0 + 3, y0 + 1)] {
        assert_eq!(cell(&driver, x, y), ' ', "{}", driver.screen());
        assert_eq!(driver.style_at(x, y).expect("in-bounds").bg, red);
    }
    assert_ne!(driver.style_at(x0 + 4, y0).expect("in-bounds").bg, red);
    assert!(driver.screen_contains("label"), "{}", driver.screen());
}

#[test]
fn clicking_a_region_reports_it_with_the_canvas_local_point() {
    let mut driver = driver();
    driver.click(AREA.x + 1.0, AREA.y + 1.0);
    let clicked: Vec<_> = events(&driver)
        .iter()
        .filter(|e| matches!(e, CanvasEvent::RegionClicked { .. }))
        .cloned()
        .collect();
    assert_eq!(clicked.len(), 1, "{:?}", events(&driver));
    let CanvasEvent::RegionClicked {
        region,
        button,
        x,
        y,
        ..
    } = &clicked[0]
    else {
        unreachable!()
    };
    assert_eq!(*region, WidgetId::new("box"));
    assert_eq!(*button, MouseButton::Left);
    assert_eq!((*x, *y), (1.0, 1.0));
}

#[test]
fn a_click_on_bare_canvas_reports_no_region_event() {
    let mut driver = driver();
    driver.click(AREA.x + 10.0, AREA.y + 3.0);
    assert!(
        !events(&driver)
            .iter()
            .any(|e| matches!(e, CanvasEvent::RegionClicked { .. })),
        "{:?}",
        events(&driver)
    );
}

#[test]
fn hover_reports_entering_a_region_and_leaving_it() {
    let mut driver = driver();
    // Entering the canvas over the region, then leaving the canvas.
    driver.mouse_move(AREA.x + 1.0, AREA.y + 1.0);
    driver.mouse_move(AREA.x + AREA.width + 2.0, AREA.y + 1.0);
    assert_eq!(
        events(&driver),
        vec![
            CanvasEvent::RegionHovered {
                region: Some("box".into())
            },
            CanvasEvent::RegionHovered { region: None },
        ]
    );
}

#[test]
fn double_clicking_a_region_reports_it() {
    let mut driver = driver();
    driver.double_click(AREA.x + 2.0, AREA.y);
    assert!(
        events(&driver).iter().any(|e| matches!(
            e,
            CanvasEvent::RegionDoubleClicked { region, .. } if *region == WidgetId::new("box")
        )),
        "{:?}",
        events(&driver)
    );
}

#[test]
fn moving_inside_the_canvas_reports_each_region_change() {
    let mut driver = driver();
    // Onto the region, across to bare canvas, and back: the pointer never
    // leaves the canvas after the first move.
    driver.mouse_move(AREA.x + 1.0, AREA.y + 1.0);
    driver.mouse_move(AREA.x + 2.0, AREA.y + 1.0);
    driver.mouse_move(AREA.x + 10.0, AREA.y + 3.0);
    driver.mouse_move(AREA.x + 1.0, AREA.y);
    assert_eq!(
        events(&driver),
        vec![
            CanvasEvent::RegionHovered {
                region: Some("box".into())
            },
            CanvasEvent::RegionHovered { region: None },
            CanvasEvent::RegionHovered {
                region: Some("box".into())
            },
        ]
    );
}
