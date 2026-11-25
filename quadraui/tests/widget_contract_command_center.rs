#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `CommandCenter` on the widget contract:
//! the strip reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through the
//! shared widget-contract engine — hit test, `Interactive::handle` — and
//! comes back out as a `UiEvent::CommandCenterActivated` this app records.
//!
//! This is the runtime leg of the widget's event-parity test. The direct
//! `Interactive::handle` leg and the `a11y::route_widget_action` leg live
//! in `src/primitives/command_center.rs`'s own `contract_tests` module,
//! and the binding-host leg in `quadraui-binding-core`'s engine tests;
//! all of them must resolve the same gesture to the same `UiEvent`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CommandCenter, CommandCenterTarget, FrameTree, NamedKey, Reaction, Rect,
    ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 3;
const AREA: Rect = Rect::new(0.0, 1.0, 40.0, 1.0);

fn cc_id() -> WidgetId {
    WidgetId::new("titlebar:cc")
}

/// One command center, drawn and routed as a `Surface::Widget`.
/// `activations` records every `UiEvent::CommandCenterActivated`;
/// `bubbled` counts the plain `MouseDown`s the strip left unclaimed.
struct CommandCenterApp {
    cc: CommandCenter,
    activations: Vec<(WidgetId, CommandCenterTarget)>,
    bubbled: usize,
}

impl CommandCenterApp {
    fn new() -> Self {
        Self {
            cc: CommandCenter {
                id: cc_id(),
                back_enabled: true,
                forward_enabled: false,
                search_label: "my-project".into(),
            },
            activations: Vec::new(),
            bubbled: 0,
        }
    }
}

impl AppLogic for CommandCenterApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.cc.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.cc.id, AREA, Box::new(self.cc.clone()));
        tree
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        vec![(self.cc.id, AREA)]
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::CommandCenterActivated { widget, target } => {
                self.activations.push((widget, target))
            }
            UiEvent::MouseDown { .. } => self.bubbled += 1,
            _ => {}
        }
        Reaction::Continue
    }
}

/// The painted column of the first occurrence of `needle` on row `y`.
fn column_of<A: AppLogic>(driver: &TuiDriver<A>, needle: char, y: usize) -> f32 {
    let screen = driver.screen();
    let row = screen.lines().nth(y).expect("row exists");
    row.chars()
        .position(|c| c == needle)
        .unwrap_or_else(|| panic!("{needle:?} not painted on row {y}:\n{screen}")) as f32
}

#[test]
fn surface_widget_command_center_paints_arrows_and_search_label() {
    let driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);
    assert!(driver.screen_contains("◀"), "{}", driver.screen());
    assert!(driver.screen_contains("▶"), "{}", driver.screen());
    assert!(driver.screen_contains("[my-project"), "{}", driver.screen());
}

#[test]
fn clicking_the_back_arrow_activates_back() {
    let mut driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    let x = column_of(&driver, '◀', 1);
    driver.click(x, AREA.y);

    assert_eq!(
        driver.app().activations,
        vec![(cc_id(), CommandCenterTarget::Back)]
    );
}

#[test]
fn clicking_the_search_box_activates_search() {
    let mut driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    let x = column_of(&driver, 'm', 1);
    driver.click(x, AREA.y);

    assert_eq!(
        driver.app().activations,
        vec![(cc_id(), CommandCenterTarget::Search)]
    );
}

#[test]
fn clicking_the_disabled_forward_arrow_activates_nothing() {
    let mut driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    let x = column_of(&driver, '▶', 1);
    driver.click(x, AREA.y);

    assert!(driver.app().activations.is_empty());
    assert_eq!(
        driver.app().bubbled,
        0,
        "the press is swallowed, not bubbled"
    );
}

#[test]
fn clicking_the_bar_padding_bubbles_to_the_app() {
    let mut driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    driver.click(0.0, AREA.y);

    assert!(driver.app().activations.is_empty());
    assert_eq!(driver.app().bubbled, 1);
}

#[test]
fn tab_then_enter_activates_the_search_box() {
    let mut driver = TuiDriver::new(CommandCenterApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert_eq!(
        driver.app().activations,
        vec![(cc_id(), CommandCenterTarget::Search)]
    );
}
