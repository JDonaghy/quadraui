#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Toolbar` on the widget contract: the
//! bar reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned `Toolbar`,
//! which the next `render` paints.
//!
//! Never a direct call into `Toolbar::click`/`Primitive::layout` that
//! would skip the routing engine; the direct-dispatch leg an
//! out-of-process binding host takes is
//! `primitives::toolbar::contract`'s own module tests.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, Surface, Toolbar,
    ToolbarButton, ToolbarEvent, UiEvent, WidgetId,
};

const COLS: u16 = 48;
const ROWS: u16 = 6;
/// One row at the top of the screen, the shape a toolbar strip has.
const BAR: Rect = Rect::new(0.0, 0.0, 40.0, 1.0);

fn action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
    ToolbarButton::Action {
        id: WidgetId::new(id),
        label: label.to_string(),
        icon: None,
        key_hint: None,
        enabled,
        is_active: false,
        tooltip: String::new(),
        spec: None,
    }
}

/// One toolbar, drawn and routed as a `Surface::Widget`, plus a record
/// of every event the host was told to apply.
struct ToolbarApp {
    bar: Toolbar,
    applied: Vec<String>,
}

impl ToolbarApp {
    fn new() -> Self {
        Self {
            bar: Toolbar::new(WidgetId::new("tb:main")).with_buttons(vec![
                action("tb:new", "New", true),
                ToolbarButton::Separator,
                action("tb:del", "Del", false),
                ToolbarButton::MenuButton {
                    id: WidgetId::new("tb:sort"),
                    label: "Sort".to_string(),
                    icon: None,
                    menu: vec![],
                },
            ]),
            applied: Vec::new(),
        }
    }
}

impl AppLogic for ToolbarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: BAR,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.bar.id, BAR, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::Toolbar(id, toolbar) = event else {
            return Reaction::Continue;
        };
        if id != self.bar.id {
            return Reaction::Continue;
        }
        match toolbar {
            // `ButtonClicked`'s apply contract: the reported `is_active`
            // is the button's *new* state, so a toggle applies it
            // verbatim.
            ToolbarEvent::ButtonClicked { id, is_active } => {
                self.applied.push(format!("clicked({id}, {is_active})"));
                for button in &mut self.bar.buttons {
                    if let ToolbarButton::Action {
                        id: button_id,
                        is_active: active,
                        label,
                        ..
                    } = button
                    {
                        if *button_id == id {
                            *active = is_active;
                            *label = label.to_uppercase();
                        }
                    }
                }
            }
            ToolbarEvent::MenuRequested { id } => {
                self.applied.push(format!("menu({id})"));
            }
            ToolbarEvent::OverflowRequested { id } => {
                self.applied.push(format!("overflow({id})"));
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<ToolbarApp> {
    let mut driver = TuiDriver::new(ToolbarApp::new(), COLS, ROWS);
    // Two deliberate single clicks on the same button back to back would
    // otherwise fold into one double-click.
    driver.set_double_click_folding(false);
    driver
}

#[test]
fn a_surface_widget_toolbar_paints_its_bracketed_buttons() {
    let driver = driver();
    assert!(driver.screen_contains("[ New ]"), "{}", driver.screen());
    assert!(driver.screen_contains("[ Del ]"), "{}", driver.screen());
    assert!(driver.screen_contains("Sort"), "{}", driver.screen());
    // The separator's own dim rule sits between the first two buttons.
    assert!(driver.screen_contains("│"), "{}", driver.screen());
}

#[test]
fn clicking_a_button_routes_a_button_clicked_event_through_the_engine() {
    let mut driver = driver();
    let (x, y) = driver.find("New").expect("the first button paints");
    driver.click(x, y);

    assert_eq!(
        driver.app().applied,
        vec!["clicked(tb:new, true)".to_string()],
        "{}",
        driver.screen()
    );
    // The host's own apply ran, so the next frame paints the new label.
    assert!(driver.screen_contains("[ NEW ]"), "{}", driver.screen());
}

#[test]
fn clicking_a_disabled_button_routes_nothing() {
    let mut driver = driver();
    let (x, y) = driver.find("Del").expect("the disabled button paints");
    driver.click(x, y);
    assert!(driver.app().applied.is_empty(), "{}", driver.screen());
}

#[test]
fn clicking_a_menu_button_requests_its_dropdown() {
    let mut driver = driver();
    let (x, y) = driver.find("Sort").expect("the menu button paints");
    driver.click(x, y);
    assert_eq!(
        driver.app().applied,
        vec!["menu(tb:sort)".to_string()],
        "{}",
        driver.screen()
    );
}

#[test]
fn keyboard_navigation_skips_the_separator_and_the_disabled_button() {
    let mut driver = driver();
    // Click to focus first — nothing routes a key to the bar until the
    // widget-contract click-to-focus step has run. The click lands on
    // the first button, which parks the cursor there.
    let (x, y) = driver.find("New").expect("the first button paints");
    driver.click(x, y);
    assert_eq!(driver.app().applied.len(), 1);

    // Right moves past the separator and the disabled action, onto the
    // menu button; Enter then activates it.
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().applied.last().map(String::as_str),
        Some("menu(tb:sort)"),
        "{}",
        driver.screen()
    );

    // Left walks back to the first button, skipping the same two items.
    driver.press_named(NamedKey::Left);
    driver.press_named(NamedKey::Enter);
    assert!(
        driver
            .app()
            .applied
            .last()
            .expect("an event was applied")
            .starts_with("clicked(tb:new"),
        "{:?} / {}",
        driver.app().applied,
        driver.screen()
    );
}

/// The cursor crossing into the strip is the position the routing
/// engine delivers to an uncaptured widget (`PointerKind::Enter`), and
/// the bar answers it by tracking which button it landed on — a repaint
/// request, never a routed event.
#[test]
fn hovering_a_button_tints_it_without_routing_an_event() {
    let mut driver = driver();
    // Start the cursor off the bar, so moving onto it is an `Enter`.
    driver.mouse_move(2.0, 4.0);
    let (x, y) = driver.find("New").expect("the first button paints");
    let reaction = driver.mouse_move(x, y);
    assert!(
        matches!(reaction, Reaction::Redraw),
        "a hover change asks for a repaint: {reaction:?}"
    );
    assert!(driver.app().applied.is_empty());
    // The strip is unchanged otherwise — a hover is paint state, not a
    // layout change.
    assert!(driver.screen_contains("[ New ]"), "{}", driver.screen());
}

/// A bar wider than its strip clips against the trailing edge, and the
/// clipped button is not clickable. `Toolbar`'s `ToolbarOverflow::Menu`
/// ladder (the `»` affordance, whose click answers
/// `ToolbarEvent::OverflowRequested` — covered in
/// `primitives::toolbar::contract`'s own tests) needs an overflow
/// request on the descriptor itself, which a host still passes through
/// `crate::tui::draw_toolbar_with_overflow`.
#[test]
fn a_bar_too_wide_for_the_strip_clips_its_trailing_button() {
    let mut driver = driver();
    driver.app_mut().bar = Toolbar::new(WidgetId::new("tb:main")).with_buttons(vec![
        action("tb:alpha", "Alphabetical", true),
        action("tb:beta", "By modified", true),
        action("tb:gamma", "By size", true),
        action("tb:delta", "By kind", true),
    ]);
    driver.render();

    let visible = driver.find("Alphabetical");
    assert!(visible.is_some(), "{}", driver.screen());
    let clipped = driver.find("By kind");
    assert!(
        clipped.is_none(),
        "the trailing button does not fit a 40-cell strip: {}",
        driver.screen()
    );
}
