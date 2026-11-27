#![cfg(feature = "tui")]
//! The `TuiDriver` leg of the event parity for the group-F widgets that
//! bindings place as generic layout leaves (`WIDGET_KINDS`): `Checkbox`,
//! `Select`, `Slider`, `Canvas`, `Chart`, `BoardModel` and `PipelineView`.
//!
//! Each widget fills the viewport as a `Surface::Widget` registered in
//! the app's `FrameTree`, so a gesture routes through the shared
//! widget-contract engine and comes back out as the `UiEvent` this app
//! records. The binding-host leg drives the same fixture, as a layout
//! leaf of the same size, through `quadraui-binding-core`'s
//! `InteractionEngine` (that crate's `group_f_event_parity` tests), and
//! the widgets with accessibility actions (`Checkbox`, `Select`, `Slider`)
//! have an `a11y::route_widget_action` leg in their own module tests.
//! Every leg asserts the same events:
//!
//! | widget         | gesture                                   | events |
//! |----------------|-------------------------------------------|--------|
//! | `Checkbox`     | click the box                             | `Form(cb, ToggleChanged { cb, true })` |
//! | `Select`       | click the field                           | `Form(sel, DropdownChanged { sel, 1 })` |
//! | `Slider`       | click the track's last column             | `Form(sl, SliderChanged { sl, 10.0 })` |
//! | `Canvas`       | enter on the region, move within, leave, click it | `RegionHovered(a)`, `RegionHovered(None)`, `RegionClicked(a, 1, 0)` |
//! | `Chart`        | enter on point 3                          | `DataPointHovered((0, 3))` |
//!
//! Hover is tracked per widget on both paths: a move that enters or
//! leaves a widget reaches it as `Enter`/`Leave`, and a move within one
//! does not reach it.
//! | `BoardModel`   | click "Third card"                        | `Board(board, SelectCard(card:3))` |
//! | `PipelineView` | click "Ship"                              | `PipelineView(pipe, StageSelected { 2 })` |

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::interactive::Interactive;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    BadgeStatus, BoardAction, BoardCard, BoardColumn, BoardModel, Canvas, CanvasEvent,
    CanvasRegion, Chart, ChartEvent, ChartKind, Checkbox, FormEvent, Modifiers, MouseButton,
    PipelineEvent, PipelineStage, PipelineView, Rect, Select, Series, Slider, StageStatus, UiEvent,
    WidgetId,
};

fn ignore<W>(_: &mut W, _: &UiEvent) -> bool {
    false
}

fn driver<W: Interactive + Clone + 'static>(
    widget: W,
    cols: u16,
    rows: u16,
) -> TuiDriver<one_widget::OneWidgetApp<W>> {
    one_widget::driver(
        widget,
        Rect::new(0.0, 0.0, cols as f32, rows as f32),
        cols,
        rows,
        ignore::<W>,
    )
}

fn recorded<W: Interactive + Clone + 'static>(
    driver: &TuiDriver<one_widget::OneWidgetApp<W>>,
    keep: impl Fn(&UiEvent) -> bool,
) -> Vec<UiEvent> {
    driver
        .app()
        .events
        .iter()
        .filter(|e| keep(e))
        .cloned()
        .collect()
}

fn is_form(e: &UiEvent) -> bool {
    matches!(e, UiEvent::Form(..))
}

#[test]
fn a_click_on_a_checkbox_matches_the_parity_event() {
    let id = WidgetId::new("cb");
    let mut driver = driver(Checkbox::new(id), 6, 1);
    driver.click(1.0, 0.0);
    assert_eq!(
        recorded(&driver, is_form),
        vec![UiEvent::Form(
            id,
            FormEvent::ToggleChanged { id, value: true }
        )]
    );
}

#[test]
fn a_click_on_a_select_matches_the_parity_event() {
    let id = WidgetId::new("sel");
    let select = Select::new(id, vec!["One".into(), "Two".into(), "Three".into()]);
    let mut driver = driver(select, 16, 1);
    driver.click(2.0, 0.0);
    assert_eq!(
        recorded(&driver, is_form),
        vec![UiEvent::Form(
            id,
            FormEvent::DropdownChanged {
                id,
                selected_idx: 1
            }
        )]
    );
}

#[test]
fn a_click_at_the_slider_track_end_matches_the_parity_event() {
    let id = WidgetId::new("sl");
    let mut driver = driver(Slider::new(id, 0.0, 0.0, 10.0), 24, 1);
    driver.click(23.0, 0.0);
    assert_eq!(
        recorded(&driver, is_form),
        vec![UiEvent::Form(
            id,
            FormEvent::SliderChanged { id, value: 10.0 }
        )]
    );
}

fn canvas() -> Canvas {
    Canvas {
        id: WidgetId::new("canvas"),
        ops: vec![],
        regions: vec![CanvasRegion {
            id: WidgetId::new("region:a"),
            rect: Rect::new(0.0, 0.0, 10.0, 1.0),
            cursor: None,
        }],
        focusable: false,
    }
}

#[test]
fn hover_and_a_click_on_a_canvas_region_match_the_parity_events() {
    let mut driver = driver(canvas(), 20, 4);
    driver.mouse_move(1.0, 0.0);
    driver.mouse_move(15.0, 3.0);
    driver.mouse_move(30.0, 10.0);
    driver.click(1.0, 0.0);
    let id = WidgetId::new("canvas");
    assert_eq!(
        recorded(&driver, |e| matches!(e, UiEvent::Canvas(..))),
        vec![
            UiEvent::Canvas(
                id,
                CanvasEvent::RegionHovered {
                    region: Some("region:a".into())
                }
            ),
            UiEvent::Canvas(id, CanvasEvent::RegionHovered { region: None }),
            UiEvent::Canvas(
                id,
                CanvasEvent::RegionClicked {
                    region: WidgetId::new("region:a"),
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    x: 1.0,
                    y: 0.0,
                }
            ),
        ]
    );
}

fn chart() -> Chart {
    Chart {
        id: WidgetId::new("chart"),
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

#[test]
fn hovering_a_chart_point_matches_the_parity_events() {
    let mut driver = driver(chart(), 24, 8);
    let layout = chart().layout_in(Rect::new(0.0, 0.0, 24.0, 8.0), 1.0, 1.0);
    let &(_, _, x, y) = layout
        .data_point_positions
        .iter()
        .find(|&&(si, di, _, _)| si == 0 && di == 3)
        .expect("point laid out");
    driver.mouse_move(x, y);
    let points: Vec<Option<(usize, usize)>> =
        recorded(&driver, |e| matches!(e, UiEvent::Chart(..)))
            .into_iter()
            .filter_map(|e| match e {
                UiEvent::Chart(_, ChartEvent::DataPointHovered { point, .. }) => Some(point),
                _ => None,
            })
            .collect();
    assert_eq!(points, vec![Some((0, 3))]);
}

fn card(id: &str, title: &str) -> BoardCard {
    BoardCard {
        id: WidgetId::new(id),
        title: title.to_string(),
        labels: vec![],
        badges: vec![quadraui::CardBadge {
            label: "T".into(),
            status: BadgeStatus::Passed,
        }],
        hint: None,
    }
}

fn board() -> BoardModel {
    BoardModel {
        id: WidgetId::new("board"),
        columns: vec![
            BoardColumn {
                id: WidgetId::new("col:todo"),
                title: "Todo".to_string(),
                cards: vec![card("card:1", "First card"), card("card:2", "Second card")],
                scroll_offset: 0,
            },
            BoardColumn {
                id: WidgetId::new("col:done"),
                title: "Done".to_string(),
                cards: vec![card("card:3", "Third card")],
                scroll_offset: 0,
            },
        ],
        selected_card_id: None,
        col_scroll_offset: 0,
    }
}

#[test]
fn a_click_on_a_board_card_matches_the_parity_event() {
    let mut driver = driver(board(), 50, 13);
    let (x, y) = driver.find("Third card").expect("card painted");
    driver.click(x, y);
    assert_eq!(
        recorded(&driver, |e| matches!(e, UiEvent::Board(..))),
        vec![UiEvent::Board(
            WidgetId::new("board"),
            BoardAction::SelectCard(WidgetId::new("card:3"))
        )]
    );
}

fn pipeline() -> PipelineView {
    let stage = |label: &str, status, action: Option<&str>| PipelineStage {
        label: label.to_string(),
        status,
        action: action.map(str::to_string),
    };
    PipelineView {
        id: WidgetId::new("pipe"),
        stages: vec![
            stage("Build", StageStatus::Done, None),
            stage("Test", StageStatus::Active, Some("Retry")),
            stage("Ship", StageStatus::Pending, None),
        ],
        focused_stage: Some(0),
    }
}

#[test]
fn a_click_on_a_pipeline_stage_matches_the_parity_event() {
    let mut driver = driver(pipeline(), 60, 7);
    let (x, y) = driver.find("Ship").expect("stage painted");
    driver.click(x, y);
    assert_eq!(
        recorded(&driver, |e| matches!(e, UiEvent::PipelineView(..))),
        vec![UiEvent::PipelineView(
            WidgetId::new("pipe"),
            PipelineEvent::StageSelected { index: 2 }
        )]
    );
}
