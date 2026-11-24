#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `TextInput` on the widget contract: the
//! input reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::Form(_, FormEvent::TextInputChanged { .. })` this test's own
//! `AppLogic::handle` applies.
//!
//! Never a direct call into `TextInput::handle`/`Primitive::layout` that
//! would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FormEvent, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, Surface,
    TextInput, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 6;
/// A bordered box with room for three content rows.
const AREA: Rect = Rect::new(2.0, 1.0, 20.0, 5.0);

fn input_id() -> WidgetId {
    WidgetId::new("input:notes")
}

/// One multi-line text input routed through the generic
/// `Interactive`/`FrameTree` engine, plus a log of what the app saw.
struct InputApp {
    input: TextInput,
    changes: Vec<String>,
    copied: Vec<String>,
}

impl InputApp {
    fn new(lines: Vec<&str>) -> Self {
        let mut input = TextInput::new(input_id())
            .with_placeholder("Notes…")
            .with_has_focus(true);
        input.lines = lines.into_iter().map(String::from).collect();
        Self {
            input,
            changes: Vec::new(),
            copied: Vec::new(),
        }
    }
}

impl AppLogic for InputApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.input.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(input_id(), AREA, Box::new(self.input.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
                self.changes.push(value.clone());
                // The app owns the buffer; it also parks its own caret at
                // the end of the last line it applied.
                self.input.lines = value.split('\n').map(String::from).collect();
                let last = self.input.lines.len().saturating_sub(1);
                self.input.cursor_line = last;
                self.input.cursor_col = self.input.lines[last].chars().count();
            }
            UiEvent::TextCopied(text) => self.copied.push(text),
            _ => {}
        }
        Reaction::Redraw
    }
}

/// Click the second character column of the first content row, which
/// both focuses the widget and parks the caret after that row's first
/// character.
///
/// The content column starts one border cell plus one pad column in, so
/// `AREA.x + 3.0` is the boundary between characters 0 and 1.
fn click_first_row(driver: &mut TuiDriver<InputApp>) {
    driver.click(AREA.x + 3.0, AREA.y + 1.0);
}

#[test]
fn surface_widget_text_input_paints_its_border_and_lines() {
    let driver = TuiDriver::new(InputApp::new(vec!["alpha", "beta"]), COLS, ROWS);

    assert!(driver.screen_contains("alpha"), "{}", driver.screen());
    assert!(driver.screen_contains("beta"), "{}", driver.screen());

    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(AREA.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    assert_eq!(row[AREA.x as usize], '┌', "{}", driver.screen());
    assert_eq!(
        row[(AREA.x + AREA.width) as usize - 1],
        '┐',
        "{}",
        driver.screen()
    );
}

#[test]
fn surface_widget_text_input_paints_its_placeholder_when_empty() {
    let driver = TuiDriver::new(InputApp::new(vec![""]), COLS, ROWS);

    assert!(driver.screen_contains("Notes…"), "{}", driver.screen());
}

#[test]
fn typing_reports_each_keystroke_as_a_text_input_change() {
    let mut driver = TuiDriver::new(InputApp::new(vec![""]), COLS, ROWS);
    click_first_row(&mut driver);

    driver.type_char('h');
    driver.type_char('i');

    assert_eq!(
        driver.app().changes,
        vec!["h".to_string(), "hi".to_string()]
    );
    assert!(driver.screen_contains("hi"), "{}", driver.screen());
}

#[test]
fn backspace_removes_the_character_before_the_caret() {
    let mut driver = TuiDriver::new(InputApp::new(vec!["abc"]), COLS, ROWS);
    click_first_row(&mut driver);
    driver.press_named(NamedKey::Backspace);

    assert_eq!(driver.app().changes, vec!["bc".to_string()]);
}

#[test]
fn enter_splits_the_line_the_caret_sits_on() {
    let mut driver = TuiDriver::new(InputApp::new(vec!["ab"]), COLS, ROWS);
    click_first_row(&mut driver);

    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().changes, vec!["a\nb".to_string()]);
    assert!(driver.screen_contains("b"), "{}", driver.screen());
}

#[test]
fn ctrl_a_then_ctrl_c_copies_the_whole_buffer() {
    let mut driver = TuiDriver::new(InputApp::new(vec!["alpha", "beta"]), COLS, ROWS);
    click_first_row(&mut driver);

    driver.ctrl_char('a');
    driver.ctrl_char('c');

    assert_eq!(driver.app().copied, vec!["alpha\nbeta".to_string()]);
    assert!(
        driver.app().changes.is_empty(),
        "a copy must not change the buffer: {:?}",
        driver.app().changes
    );
}

#[test]
fn ctrl_x_copies_the_selection_and_reports_the_shortened_buffer() {
    let mut driver = TuiDriver::new(InputApp::new(vec!["alpha"]), COLS, ROWS);
    click_first_row(&mut driver);

    driver.ctrl_char('a');
    driver.ctrl_char('x');

    assert_eq!(driver.app().copied, vec!["alpha".to_string()]);
    assert_eq!(driver.app().changes, vec![String::new()]);
}

#[test]
fn a_caret_only_arrow_key_changes_nothing() {
    let mut driver = TuiDriver::new(InputApp::new(vec!["abc"]), COLS, ROWS);
    click_first_row(&mut driver);

    driver.press_named(NamedKey::Left);
    driver.press_named(NamedKey::Right);

    assert!(
        driver.app().changes.is_empty(),
        "arrow keys must not report a content change: {:?}",
        driver.app().changes
    );
}
