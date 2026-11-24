#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ToastOverlay` on the widget contract:
//! one widget paints the whole corner stack, reaches the frame as a
//! type-erased `Surface::Widget`, is painted through `DynWidget::paint` →
//! `Primitive::paint_cells`, and is registered in the app's `FrameTree`
//! so input routes through the shared widget-contract engine — `UiEvent`
//! into `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::ToastActionClicked` this test's own `AppLogic::handle`
//! applies to the owned descriptor, which the next `render` paints.
//!
//! Never a direct call into `ToastOverlay::handle`/`Primitive::layout`
//! that would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, Surface, Toast,
    ToastButton, ToastCorner, ToastOverlay, ToastSeverity, UiEvent, WidgetId,
};

const COLS: u16 = 60;
const ROWS: u16 = 16;
/// The stack anchors its corner to the whole screen, so the widget's own
/// rect is the overlay area.
const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

const SAVED: &str = "Saved";
const RECONNECTED: &str = "Reconnected";

fn undo_id() -> WidgetId {
    WidgetId::new("toast:saved:undo")
}

fn saved_id() -> WidgetId {
    WidgetId::new("toast:saved")
}

fn reconnected_id() -> WidgetId {
    WidgetId::new("toast:reconnected")
}

fn toast(id: WidgetId, title: &str, actions: Vec<ToastButton>) -> Toast {
    Toast {
        id,
        title: title.into(),
        body: String::new(),
        severity: ToastSeverity::Info,
        actions,
        accent: None,
    }
}

/// One toast stack routed through the generic `Interactive`/`FrameTree`
/// engine. The app owns the toast list — the widget never mutates it —
/// so a dismissal is applied here and observed on the next render.
struct ToastApp {
    stack: ToastOverlay,
    actions: Vec<WidgetId>,
    dismissed: Vec<WidgetId>,
}

impl ToastApp {
    fn new() -> Self {
        Self {
            stack: ToastOverlay {
                id: WidgetId::new("toasts"),
                corner: ToastCorner::BottomRight,
                toasts: vec![
                    toast(reconnected_id(), RECONNECTED, Vec::new()),
                    toast(
                        saved_id(),
                        SAVED,
                        vec![ToastButton {
                            id: undo_id(),
                            label: "Undo".into(),
                            primary: true,
                            spec: None,
                        }],
                    ),
                ],
                focus: None,
            },
            actions: Vec::new(),
            dismissed: Vec::new(),
        }
    }
}

impl AppLogic for ToastApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.stack.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.stack.id, AREA, Box::new(self.stack.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        // `ToastActionClicked` carries an action's own id for an
        // activation and the toast's own id for a dismissal — the host
        // already holds the toast list it is about to filter, so matching
        // against it is how the two are told apart.
        if let UiEvent::ToastActionClicked(id) = event {
            if self.stack.toasts.iter().any(|t| t.id == id) {
                self.dismissed.push(id);
                self.stack.toasts.retain(|t| t.id != id);
            } else {
                self.actions.push(id);
            }
        }
        Reaction::Redraw
    }
}

/// Click the painted cell a label starts in — its own row and first
/// column, derived from the rendered screen rather than any hardcoded
/// coordinate.
fn click_label(driver: &mut TuiDriver<ToastApp>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen"));
    driver.click(bounds.x + 0.5, bounds.y + 0.5);
}

/// Column of the right-most painted cell on row `y` — a box's own right
/// border, since nothing else is drawn on a toast row.
fn right_edge(driver: &TuiDriver<ToastApp>, y: usize) -> usize {
    driver
        .screen()
        .lines()
        .nth(y)
        .expect("row on screen")
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .expect("a painted cell on the row")
}

#[test]
fn surface_widget_toast_paints_the_whole_corner_stack() {
    let driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    assert!(driver.screen_contains(SAVED), "{}", driver.screen());
    assert!(driver.screen_contains(RECONNECTED), "{}", driver.screen());
    assert!(driver.screen_contains("Undo"), "{}", driver.screen());
    assert!(driver.screen_contains("×"), "{}", driver.screen());

    // Newest toast nearest the bottom corner; the older one stacks above
    // it, and both sit against the right edge.
    let saved = driver.find_bounds(SAVED).expect("newest toast title");
    let older = driver.find_bounds(RECONNECTED).expect("older toast title");
    assert!(
        older.y < saved.y,
        "the older toast must stack above the newest: {}",
        driver.screen()
    );
    assert_eq!(
        right_edge(&driver, older.y as usize),
        right_edge(&driver, saved.y as usize),
        "a bottom-right stack shares one right edge: {}",
        driver.screen()
    );
}

#[test]
fn clicking_the_dismiss_glyph_removes_that_toast_from_the_next_frame() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    // The newest toast's `×` is the lower of the two on screen.
    let dismiss = driver
        .screen()
        .lines()
        .enumerate()
        .filter_map(|(y, line)| {
            line.chars()
                .position(|c| c == '×')
                .map(|x| (x as f32, y as f32))
        })
        .last()
        .expect("a dismiss glyph on screen");
    driver.click(dismiss.0 + 0.5, dismiss.1 + 0.5);

    assert_eq!(driver.app().dismissed, vec![saved_id()]);
    assert!(driver.app().actions.is_empty());
    assert!(
        !driver.screen_contains(SAVED),
        "the dismissed toast must be gone from the repaint: {}",
        driver.screen()
    );
    assert!(driver.screen_contains(RECONNECTED), "{}", driver.screen());
}

#[test]
fn clicking_an_action_button_activates_that_action() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    click_label(&mut driver, "Undo");

    assert_eq!(driver.app().actions, vec![undo_id()]);
    assert!(driver.app().dismissed.is_empty());
    assert!(
        driver.screen_contains(SAVED),
        "an action click must not dismiss the toast: {}",
        driver.screen()
    );
}

#[test]
fn tab_then_enter_reaches_the_focused_action() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    // A body click focuses that toast's dismiss `×` — the same seed
    // `ToastStackController::give_focus` uses — so keys route to the
    // stack from here on.
    click_label(&mut driver, SAVED);
    assert!(driver.app().dismissed.is_empty());
    assert!(driver.app().actions.is_empty());

    // Tab walks dismiss → the first action; Enter activates it.
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().actions, vec![undo_id()]);
    assert!(driver.app().dismissed.is_empty());
}

#[test]
fn enter_on_the_focused_dismiss_glyph_dismisses_the_toast() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    click_label(&mut driver, SAVED);
    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().dismissed, vec![saved_id()]);
}

#[test]
fn escape_dismisses_the_focused_toast() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    click_label(&mut driver, SAVED);
    driver.press_named(NamedKey::Escape);

    assert_eq!(driver.app().dismissed, vec![saved_id()]);
    assert!(
        !driver.screen_contains(SAVED),
        "the dismissed toast must be gone from the repaint: {}",
        driver.screen()
    );
}

#[test]
fn up_moves_focus_to_the_older_toast() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    click_label(&mut driver, SAVED);
    // `toasts` is oldest-first, so Up steps from the newest to the
    // older toast; Enter then dismisses *that* one.
    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().dismissed, vec![reconnected_id()]);
    assert!(driver.screen_contains(SAVED), "{}", driver.screen());
}

#[test]
fn a_click_on_empty_space_still_reaches_the_app_as_a_mouse_event() {
    let mut driver = TuiDriver::new(ToastApp::new(), COLS, ROWS);

    driver.click(1.0, 1.0);

    assert!(driver.app().actions.is_empty());
    assert!(driver.app().dismissed.is_empty());
}
