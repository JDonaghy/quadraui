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

use quadraui::primitive::DynWidget;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Dialog, DialogButton, DialogMeasure, DrawsPrimitives, FrameTree, LayoutCtx,
    NamedKey, Placed, Primitive, Reaction, Rect, ScreenLayout, StyledSpan, StyledText, Surface,
    ToolbarItemMeasure, UiEvent, WidgetId,
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

/// One dialog painted straight through `draw_dyn` into `area` and routed
/// through the app's `FrameTree` — at its own layout, or, once
/// `host_box` is set, wrapped in `Placed` at a box the host resolved.
struct DrawnDialogApp {
    dialog: Dialog,
    area: Rect,
    host_box: Option<Rect>,
    resolved: Vec<WidgetId>,
}

impl DrawnDialogApp {
    fn new(body: &[&str], cols: u16, rows: u16) -> Self {
        let mut dialog = DialogApp::new().dialog;
        dialog.body = body
            .iter()
            .map(|line| StyledText {
                spans: vec![StyledSpan::plain(*line)],
            })
            .collect();
        Self {
            dialog,
            area: Rect::new(0.0, 0.0, cols as f32, rows as f32),
            host_box: None,
            resolved: Vec::new(),
        }
    }

    /// The dialog as the app hands it to the frame: bare, or at the
    /// host-resolved box — the dialog's own cell-grid layout for `area`
    /// with its `DialogLayout` replaced by one centred in `host_box`.
    fn widget(&self) -> Box<dyn DynWidget> {
        let Some(host_box) = self.host_box else {
            return Box::new(self.dialog.clone());
        };
        let mut layout =
            Primitive::layout(&self.dialog, &LayoutCtx::new(1.0, 1.0, false), self.area);
        let measure = DialogMeasure {
            width: host_box.width,
            title_height: 1.0,
            body_height: self.dialog.body.len() as f32,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 1.0,
            button_width: 8.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        layout.layout = self
            .dialog
            .layout(host_box, measure, |_| ToolbarItemMeasure::new(0.0));
        Box::new(Placed::new(self.dialog.clone(), layout))
    }
}

impl AppLogic for DrawnDialogApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let _ = backend.draw_dyn(self.area, self.widget().as_ref(), &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.dialog.id, self.area, self.widget());
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MessageDialogResolved(id) = &event {
            self.resolved.push(*id);
        }
        Reaction::Redraw
    }
}

/// A confirm message 60 columns wide — wider than the dialog's
/// 30…60-column default box leaves for body text.
const LONG_BODY: &str = "Move 'main.rs' to '/Users/john/src/project/src/bin/tools/x'?";

#[test]
fn a_body_line_wider_than_the_default_box_paints_whole() {
    assert_eq!(LONG_BODY.chars().count(), 60);
    let driver = TuiDriver::new(DrawnDialogApp::new(&[LONG_BODY], 80, 24), 80, 24);

    assert!(driver.screen_contains(LONG_BODY), "{}", driver.screen());
}

#[test]
fn a_body_line_wider_than_the_viewport_is_clipped_inside_its_margin() {
    let driver = TuiDriver::new(DrawnDialogApp::new(&[LONG_BODY], 40, 12), 40, 12);
    let screen = driver.screen();

    // The box stops two columns short of either edge.
    for line in screen.lines() {
        let chars: Vec<char> = line.chars().collect();
        assert!(
            chars[..2].iter().chain(&chars[38..]).all(|c| *c == ' '),
            "{screen}"
        );
    }
    assert!(driver.screen_contains("Move 'main.rs'"), "{screen}");
}

#[test]
fn a_body_longer_than_ten_lines_paints_every_line() {
    let lines: Vec<String> = (1..=12).map(|i| format!("body line {i:02}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let driver = TuiDriver::new(DrawnDialogApp::new(&refs, 80, 24), 80, 24);

    for line in &lines {
        assert!(driver.screen_contains(line), "{line}: {}", driver.screen());
    }
}

#[test]
fn a_placed_dialog_paints_and_hit_tests_at_the_host_box() {
    let mut app = DrawnDialogApp::new(&[BODY], 80, 24);
    let host_box = Rect::new(0.0, 0.0, 40.0, 5.0);
    app.host_box = Some(host_box);
    let mut driver = TuiDriver::new(app, 80, 24);

    let title = driver.find_bounds(TITLE).expect("title on screen");
    assert_eq!((title.x, title.y), (3.0, 0.0), "{}", driver.screen());
    let body = driver.find_bounds(BODY).expect("body on screen");
    assert_eq!((body.x, body.y), (1.0, 2.0), "{}", driver.screen());

    click_label_in(&mut driver, "Cancel");
    assert_eq!(driver.app().resolved, vec![cancel_id()]);
}

/// [`click_label`] for a [`DrawnDialogApp`].
fn click_label_in(driver: &mut TuiDriver<DrawnDialogApp>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen"));
    driver.click(bounds.x + 0.5, bounds.y);
}
