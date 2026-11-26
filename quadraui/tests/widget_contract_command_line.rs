#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `CommandLine` on the widget contract:
//! the bar reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::Form(_, FormEvent::…)` this test's own `AppLogic::handle`
//! records.
//!
//! Never a direct call into `CommandLine::handle`/`Primitive::layout`
//! that would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CommandLine, FormEvent, FrameTree, NamedKey, Reaction, Rect, ScreenLayout,
    Surface, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 4;
/// The bottom row of the screen, the strip an editor's command bar owns.
const AREA: Rect = Rect::new(0.0, 3.0, COLS as f32, 1.0);

fn bar_id() -> WidgetId {
    WidgetId::new("cmdline:editor")
}

/// One command bar routed through the generic `Interactive`/`FrameTree`
/// engine, plus a log of what the app saw.
struct BarApp {
    bar: CommandLine,
    changes: Vec<String>,
    submits: Vec<String>,
    keys: Vec<String>,
    copied: Vec<String>,
}

impl BarApp {
    fn new(text: &str) -> Self {
        Self {
            bar: CommandLine {
                id: bar_id(),
                text: text.into(),
                cursor_offset: Some(text.len()),
                right_align: false,
                selection: None,
            },
            changes: Vec::new(),
            submits: Vec::new(),
            keys: Vec::new(),
            copied: Vec::new(),
        }
    }
}

impl AppLogic for BarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(bar_id(), AREA, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
                self.changes.push(value.clone());
                // The app owns the text; it also parks its own cursor at
                // the end of what it applied.
                self.bar.cursor_offset = Some(value.len());
                self.bar.text = value;
            }
            UiEvent::Form(_, FormEvent::TextInputCommitted { value, .. }) => {
                self.submits.push(value)
            }
            UiEvent::Form(_, FormEvent::KeyPressed { key, .. }) => self.keys.push(key),
            UiEvent::TextCopied(text) => self.copied.push(text),
            _ => {}
        }
        Reaction::Redraw
    }
}

/// Click the bar's second column, which both focuses it and parks the
/// caret after the first character.
fn click_second_column(driver: &mut TuiDriver<BarApp>) {
    driver.click(AREA.x + 1.0, AREA.y);
}

#[test]
fn surface_widget_command_line_paints_its_text_on_its_own_row() {
    let driver = TuiDriver::new(BarApp::new(":wq"), COLS, ROWS);

    assert!(driver.screen_contains(":wq"), "{}", driver.screen());
    let bounds = driver.find_bounds(":wq").expect(":wq on screen");
    assert_eq!(
        (bounds.x, bounds.y),
        (AREA.x, AREA.y),
        "{}",
        driver.screen()
    );
}

#[test]
fn a_right_aligned_command_line_paints_against_the_far_edge() {
    let mut app = BarApp::new("3/17");
    app.bar.right_align = true;
    app.bar.cursor_offset = None;
    let driver = TuiDriver::new(app, COLS, ROWS);

    let bounds = driver.find_bounds("3/17").expect("count on screen");
    assert_eq!(bounds.x, AREA.x + AREA.width - 4.0, "{}", driver.screen());
}

#[test]
fn typing_reports_each_keystroke_as_a_text_input_change() {
    let mut driver = TuiDriver::new(BarApp::new(":"), COLS, ROWS);
    click_second_column(&mut driver);

    driver.type_char('w');
    driver.type_char('q');

    assert_eq!(
        driver.app().changes,
        vec![":w".to_string(), ":wq".to_string()]
    );
    assert!(driver.screen_contains(":wq"), "{}", driver.screen());
}

#[test]
fn enter_submits_the_command_text() {
    let mut driver = TuiDriver::new(BarApp::new(":wq"), COLS, ROWS);
    click_second_column(&mut driver);

    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().submits, vec![":wq".to_string()]);
    assert!(
        driver.app().changes.is_empty(),
        "submitting must not change the text: {:?}",
        driver.app().changes
    );
}

#[test]
fn escape_reports_the_cancel_passthrough() {
    let mut driver = TuiDriver::new(BarApp::new(":wq"), COLS, ROWS);
    click_second_column(&mut driver);

    driver.press_named(NamedKey::Escape);

    assert_eq!(driver.app().keys, vec!["Escape".to_string()]);
}

#[test]
fn backspace_removes_the_character_before_the_clicked_caret() {
    let mut driver = TuiDriver::new(BarApp::new(":wq"), COLS, ROWS);
    click_second_column(&mut driver);

    driver.press_named(NamedKey::Backspace);

    assert_eq!(driver.app().changes, vec!["wq".to_string()]);
}

#[test]
fn ctrl_a_then_ctrl_c_copies_the_whole_command() {
    let mut driver = TuiDriver::new(BarApp::new(":wq"), COLS, ROWS);
    click_second_column(&mut driver);

    driver.ctrl_char('a');
    driver.ctrl_char('c');

    assert_eq!(driver.app().copied, vec![":wq".to_string()]);
    assert!(
        driver.app().changes.is_empty(),
        "a copy must not change the text: {:?}",
        driver.app().changes
    );
}
