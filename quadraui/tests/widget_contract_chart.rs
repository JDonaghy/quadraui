#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Chart` on the widget contract: the
//! chart reaches the frame as a `Surface::Widget`, paints through its cell
//! painter, and reports hover, clicks and keyboard stepping as
//! `UiEvent::Chart` events. Hover is app-owned: this app writes each
//! `DataPointHovered` back onto `Chart::hovered_point`, and the marker
//! follows.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Chart, ChartEvent, ChartKind, FrameTree, NamedKey, Reaction, Rect,
    ScreenLayout, Series, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 12;
const AREA: Rect = Rect::new(1.0, 1.0, 24.0, 8.0);

fn chart_id() -> WidgetId {
    WidgetId::new("chart:load")
}

fn chart() -> Chart {
    Chart {
        id: chart_id(),
        kind: ChartKind::Bar,
        series: vec![Series {
            label: "load".into(),
            data: vec![1.0, 3.0, 2.0, 4.0],
            color: None,
            fill: false,
        }],
        x_label: None,
        y_label: None,
        y_range: Some((0.0, 4.0)),
        x_range: None,
        show_legend: true,
        y_ticks: Some(0),
        x_ticks: None,
        show_grid: false,
        hovered_point: None,
        crosshair_x: None,
    }
}

struct ChartApp {
    chart: Chart,
    events: Vec<ChartEvent>,
}

impl AppLogic for ChartApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.chart.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.chart.id, AREA, Box::new(self.chart.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Chart(id, ev) = event {
            if id == chart_id() {
                if let ChartEvent::DataPointHovered { point, .. } = &ev {
                    self.chart.hovered_point = *point;
                }
                self.events.push(ev);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver() -> TuiDriver<ChartApp> {
    TuiDriver::new(
        ChartApp {
            chart: chart(),
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn hovers(driver: &TuiDriver<ChartApp>) -> Vec<Option<(usize, usize)>> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            ChartEvent::DataPointHovered { point, .. } => Some(*point),
            _ => None,
        })
        .collect()
}

/// The screen position of data point `di`: the centre of its bar slot.
fn point_at(driver: &TuiDriver<ChartApp>, di: usize) -> (f32, f32) {
    let layout = driver.app().chart.layout_in(AREA, 1.0, 1.0);
    let &(_, _, x, y) = layout
        .data_point_positions
        .iter()
        .find(|&&(si, d, _, _)| si == 0 && d == di)
        .expect("point laid out");
    (x, y)
}

#[test]
fn surface_widget_chart_paints_bars_and_its_legend() {
    let driver = driver();
    assert!(driver.screen_contains("load"), "{}", driver.screen());
    assert!(driver.screen().contains('█'), "{}", driver.screen());
}

#[test]
fn hovering_a_point_reports_it_and_the_marker_follows() {
    let mut driver = driver();
    let (x, y) = point_at(&driver, 3);
    driver.mouse_move(x, y);
    assert_eq!(hovers(&driver), vec![Some((0, 3))]);
    assert_eq!(driver.app().chart.hovered_point, Some((0, 3)));
    assert!(driver.screen().contains('●'), "{}", driver.screen());

    // The same point again reports nothing new; leaving every point
    // reports `None`.
    driver.mouse_move(x, y);
    driver.mouse_move(AREA.x + AREA.width + 3.0, AREA.y + AREA.height + 2.0);
    assert_eq!(hovers(&driver), vec![Some((0, 3)), None]);
    assert!(!driver.screen().contains('●'), "{}", driver.screen());
}

#[test]
fn clicking_a_point_reports_it() {
    let mut driver = driver();
    let (x, y) = point_at(&driver, 1);
    driver.click(x, y);
    assert!(
        driver.app().events.contains(&ChartEvent::DataPointClicked {
            id: chart_id(),
            series_idx: 0,
            data_idx: 1,
        }),
        "{:?}",
        driver.app().events
    );
}

#[test]
fn clicking_the_legend_reports_its_series() {
    let mut driver = driver();
    let (x, y) = driver.find("load").expect("legend painted");
    driver.click(x, y);
    assert!(
        driver.app().events.contains(&ChartEvent::LegendClicked {
            id: chart_id(),
            series_idx: 0,
        }),
        "{:?}",
        driver.app().events
    );
}

#[test]
fn arrow_keys_step_the_hover_through_the_points() {
    let mut driver = driver();
    driver.click(AREA.x + 1.0, AREA.y + AREA.height - 1.0);
    driver.app_mut().events.clear();
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Left);
    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        hovers(&driver),
        vec![Some((0, 0)), Some((0, 1)), Some((0, 0)), Some((0, 3))]
    );
    assert_eq!(
        driver.app().events.last(),
        Some(&ChartEvent::DataPointClicked {
            id: chart_id(),
            series_idx: 0,
            data_idx: 3,
        })
    );
}
