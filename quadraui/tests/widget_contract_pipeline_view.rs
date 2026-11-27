#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `PipelineView` on the widget contract:
//! the pipeline reaches the frame as a type-erased `Surface::Widget`,
//! paints its stages through `Primitive::paint_cells`, and keys and clicks
//! route through `Interactive::handle` back out as `UiEvent::PipelineView`
//! — `focused_stage` staying the app's (controlled mode).

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, PipelineEvent, PipelineStage, PipelineView, Reaction,
    Rect, ScreenLayout, StageStatus, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 64;
const ROWS: u16 = 10;
const AREA: Rect = Rect::new(1.0, 1.0, 60.0, 7.0);

fn pipeline_id() -> WidgetId {
    WidgetId::new("pipeline:deploy")
}

fn view() -> PipelineView {
    PipelineView {
        id: pipeline_id(),
        stages: vec![
            PipelineStage {
                label: "Build".to_string(),
                status: StageStatus::Done,
                action: None,
            },
            PipelineStage {
                label: "Test".to_string(),
                status: StageStatus::Active,
                action: Some("Retry".to_string()),
            },
            PipelineStage {
                label: "Ship".to_string(),
                status: StageStatus::Pending,
                action: None,
            },
        ],
        focused_stage: Some(0),
    }
}

struct PipelineApp {
    view: PipelineView,
    events: Vec<PipelineEvent>,
}

impl AppLogic for PipelineApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.view.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.view.id, AREA, Box::new(self.view.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::PipelineView(id, event) = event {
            if id == pipeline_id() {
                if let PipelineEvent::StageFocused { index } = event {
                    self.view.focused_stage = Some(index);
                }
                self.events.push(event);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver() -> TuiDriver<PipelineApp> {
    TuiDriver::new(
        PipelineApp {
            view: view(),
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

#[test]
fn surface_widget_pipeline_paints_every_stage() {
    let driver = driver();
    for needle in ["Build", "Test", "Ship", "Retry"] {
        assert!(driver.screen_contains(needle), "{needle}:\n{}", driver.screen());
    }
}

#[test]
fn clicking_a_stage_selects_it_and_its_button_fires_the_action() {
    let mut driver = driver();
    let (x, y) = driver.find("Ship").expect("stage painted");
    driver.click(x, y);
    let (x, y) = driver.find("Retry").expect("action painted");
    driver.click(x, y);
    assert_eq!(
        driver.app().events,
        vec![
            PipelineEvent::StageSelected { index: 2 },
            PipelineEvent::StageAction { index: 1 },
        ]
    );
}

#[test]
fn arrows_report_the_focus_move_and_enter_activates_the_focused_stage() {
    let mut driver = driver();
    let (x, y) = driver.find("Build").expect("stage painted");
    driver.click(x, y);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Enter);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().events,
        vec![
            PipelineEvent::StageSelected { index: 0 },
            PipelineEvent::StageFocused { index: 1 },
            PipelineEvent::StageAction { index: 1 },
            PipelineEvent::StageFocused { index: 2 },
            PipelineEvent::StageFocused { index: 2 },
            PipelineEvent::StageSelected { index: 2 },
        ]
    );
}

#[test]
fn other_keys_bubble_to_the_app() {
    let mut driver = driver();
    let (x, y) = driver.find("Build").expect("stage painted");
    driver.click(x, y);
    driver.type_char('q');
    assert_eq!(
        driver.app().events,
        vec![PipelineEvent::StageSelected { index: 0 }]
    );
}
