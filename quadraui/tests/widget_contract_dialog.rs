#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Dialog` on the widget contract: the
//! modal reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::MessageDialogResolved` this test's own `AppLogic::handle`
//! records.
//!
//! Never a direct call into `Dialog::handle`/`Primitive::layout` that
//! would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Dialog, DialogButton, FrameTree, NamedKey, Reaction, Rect, ScreenLayout,
    StyledSpan, StyledText, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 60;
const ROWS: u16 = 20;
/// The modal centres itself in the whole screen, so the widget's own rect
/// is the viewport.
const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

const TITLE: &str = "Confirm";
const BODY: &str = "Unsaved edits remain.";

fn save_id() -> WidgetId {
    WidgetId::new("dialog:save")
}

fn cancel_id() -> WidgetId {
    WidgetId::new("dialog:cancel")
}

/// One modal dialog routed through the generic `Interactive`/`FrameTree`
/// engine, plus a log of what the app saw.
struct DialogApp {
    dialog: Dialog,
    resolved: Vec<WidgetId>,
    events: Vec<UiEvent>,
}

impl DialogApp {
    fn new() -> Self {
        Self {
            dialog: Dialog {
                id: WidgetId::new("dialog:quit"),
                title: StyledText {
                    spans: vec![StyledSpan::plain(TITLE)],
                },
                body: vec![StyledText {
                    spans: vec![StyledSpan::plain(BODY)],
                }],
                buttons: vec![
                    DialogButton {
                        id: save_id(),
                        label: "Save".into(),
                        is_default: true,
                        is_cancel: false,
                        tint: None,
                        spec: None,
                    },
                    DialogButton {
                        id: cancel_id(),
                        label: "Cancel".into(),
                        is_default: false,
                        is_cancel: true,
                        tint: None,
                        spec: None,
                    },
                ],
                severity: None,
                vertical_buttons: false,
                table: None,
                input: None,
            },
            resolved: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl AppLogic for DialogApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.dialog.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.dialog.id, AREA, Box::new(self.dialog.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MessageDialogResolved(id) = &event {
            self.resolved.push(*id);
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

/// Click the painted cell a label starts in.
///
/// The cell's own row (`bounds.y`, a whole row) is what a terminal
/// delivers for a click on that row — the modal's box is centred on a
/// half-cell boundary when the viewport's spare height is odd, so the
/// row's *centre* (`bounds.y + 0.5`) can fall one row past the float
/// rect the layout resolved.
fn click_label(driver: &mut TuiDriver<DialogApp>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen"));
    driver.click(bounds.x + 0.5, bounds.y);
}

/// Click a cell inside the modal's body text — enough to give the widget
/// keyboard focus (`Dialog::focus` is `FocusSpec::Children`, so nothing
/// routes a key to it until the engine's click-to-focus step runs)
/// without activating any button.
fn focus_the_modal(driver: &mut TuiDriver<DialogApp>) {
    let (x, y) = driver.find(BODY).expect("body text on screen");
    driver.click(x, y);
    assert!(
        driver.app().resolved.is_empty(),
        "a click on the modal body must not resolve a button: {:?}",
        driver.app().resolved
    );
}

#[test]
fn surface_widget_dialog_paints_its_chrome_title_body_and_buttons() {
    let driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);

    assert!(driver.screen_contains(TITLE), "{}", driver.screen());
    assert!(driver.screen_contains(BODY), "{}", driver.screen());
    assert!(driver.screen_contains("Save"), "{}", driver.screen());
    assert!(driver.screen_contains("Cancel"), "{}", driver.screen());

    // The rounded box chrome the cell painter draws, on the row above the
    // title's own row is the title row itself — the title is overlaid
    // into the top rule, so the top-left corner shares that row.
    let title = driver.find_bounds(TITLE).expect("title on screen");
    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(title.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    assert_eq!(row[title.x as usize - 3], '╭', "{}", driver.screen());
    assert!(
        row.contains(&'╮'),
        "the top rule must close: {}",
        driver.screen()
    );
}

#[test]
fn clicking_a_button_resolves_that_button() {
    let mut driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);

    click_label(&mut driver, "Cancel");

    assert_eq!(driver.app().resolved, vec![cancel_id()]);
}

#[test]
fn escape_resolves_the_declared_cancel_button() {
    let mut driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);
    focus_the_modal(&mut driver);

    driver.press_named(NamedKey::Escape);

    assert_eq!(driver.app().resolved, vec![cancel_id()]);
}

#[test]
fn enter_activates_the_default_button() {
    let mut driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);
    focus_the_modal(&mut driver);

    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().resolved, vec![save_id()]);
}

#[test]
fn tab_and_shift_tab_walk_the_button_row() {
    let mut driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);
    focus_the_modal(&mut driver);

    // Focus starts on the default button; one Tab steps to the next.
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().resolved, vec![cancel_id()]);

    // Shift-Tab steps back, and both directions wrap at the ends.
    driver.press(quadraui::Key::Named(NamedKey::BackTab));
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().resolved, vec![cancel_id(), save_id()]);

    driver.press(quadraui::Key::Named(NamedKey::BackTab));
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().resolved,
        vec![cancel_id(), save_id(), cancel_id()],
        "Shift-Tab must wrap back to the last button"
    );
}

#[test]
fn a_click_outside_the_modal_box_still_reaches_the_app_as_a_mouse_event() {
    let mut driver = TuiDriver::new(DialogApp::new(), COLS, ROWS);

    driver.click(1.0, 1.0);

    assert!(
        driver.app().resolved.is_empty(),
        "a click outside the box must not answer the modal"
    );
    assert!(
        driver.app().events.iter().any(|e| matches!(
            e,
            UiEvent::MouseDown { position, .. } if position.x == 1.0 && position.y == 1.0
        )),
        "the plain mouse-down must bubble to the app: {:?}",
        driver.app().events
    );
}
