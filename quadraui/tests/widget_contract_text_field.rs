#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `TextField` on the widget contract: the
//! field reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::Form(_, FormEvent::TextInput{Changed,Committed})` this
//! test's own `AppLogic::handle` records.
//!
//! Never a direct call into `TextField::handle`/`Primitive::layout` that
//! would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FormEvent, FrameTree, Icon, NamedKey, Reaction, Rect, ScreenLayout, Surface,
    TextField, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 3;
/// One bracketed row, wide enough for an icon, text and a clear button.
const AREA: Rect = Rect::new(2.0, 1.0, 14.0, 1.0);

fn field_id() -> WidgetId {
    WidgetId::new("field:search")
}

/// One standalone search field routed through the generic
/// `Interactive`/`FrameTree` engine, plus a log of what the app saw.
struct FieldApp {
    field: TextField,
    changes: Vec<String>,
    commits: Vec<String>,
    copied: Vec<String>,
}

impl FieldApp {
    fn new(value: &str) -> Self {
        Self {
            field: TextField::new(field_id())
                .with_value(value)
                .with_placeholder("Search")
                .with_leading_icon(Icon::new("S", "S"))
                .with_clearable(true)
                .with_has_focus(true),
            changes: Vec::new(),
            commits: Vec::new(),
            copied: Vec::new(),
        }
    }
}

impl AppLogic for FieldApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.field.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(field_id(), AREA, Box::new(self.field.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
                self.changes.push(value.clone());
                // The app owns the value; it also parks its own caret at
                // the end of what it applied.
                self.field.cursor = Some(value.len());
                self.field.value = value;
            }
            UiEvent::Form(_, FormEvent::TextInputCommitted { value, .. }) => {
                self.commits.push(value)
            }
            UiEvent::TextCopied(text) => self.copied.push(text),
            _ => {}
        }
        Reaction::Redraw
    }
}

/// Click the first text column, which both focuses the field and parks
/// the caret at the start of the value.
///
/// The text column starts after the opening bracket and the leading-icon
/// cell, so that is `AREA.x + 2.0`.
fn click_text_start(driver: &mut TuiDriver<FieldApp>) {
    driver.click(AREA.x + 2.0, AREA.y);
}

#[test]
fn surface_widget_text_field_paints_its_brackets_icon_value_and_clear_button() {
    let driver = TuiDriver::new(FieldApp::new("abc"), COLS, ROWS);

    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(AREA.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    assert_eq!(row[AREA.x as usize], '[', "{}", driver.screen());
    assert_eq!(
        row[(AREA.x + AREA.width) as usize - 1],
        ']',
        "{}",
        driver.screen()
    );
    assert_eq!(row[AREA.x as usize + 1], 'S', "{}", driver.screen());
    assert!(driver.screen_contains("abc"), "{}", driver.screen());
    assert!(
        row.contains(&'\u{00D7}'),
        "a clearable non-empty field shows the × button: {}",
        driver.screen()
    );
}

#[test]
fn surface_widget_text_field_paints_its_placeholder_when_empty() {
    let driver = TuiDriver::new(FieldApp::new(""), COLS, ROWS);

    assert!(driver.screen_contains("Search"), "{}", driver.screen());
}

#[test]
fn typing_reports_each_keystroke_as_a_text_input_change() {
    let mut driver = TuiDriver::new(FieldApp::new(""), COLS, ROWS);
    click_text_start(&mut driver);

    driver.type_char('h');
    driver.type_char('i');

    assert_eq!(
        driver.app().changes,
        vec!["h".to_string(), "hi".to_string()]
    );
    assert!(driver.screen_contains("hi"), "{}", driver.screen());
}

#[test]
fn enter_commits_the_value_instead_of_inserting_a_newline() {
    let mut driver = TuiDriver::new(FieldApp::new("abc"), COLS, ROWS);
    click_text_start(&mut driver);

    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().commits, vec!["abc".to_string()]);
    assert!(
        driver.app().changes.is_empty(),
        "a commit must not change the value: {:?}",
        driver.app().changes
    );
}

#[test]
fn clicking_the_clear_button_empties_the_field() {
    let mut driver = TuiDriver::new(FieldApp::new("abc"), COLS, ROWS);

    let clear_x = AREA.x + AREA.width - 2.0;
    driver.click(clear_x, AREA.y);

    assert_eq!(driver.app().changes, vec![String::new()]);
    assert!(driver.screen_contains("Search"), "{}", driver.screen());
}

#[test]
fn delete_removes_the_character_after_the_caret() {
    let mut driver = TuiDriver::new(FieldApp::new("abc"), COLS, ROWS);
    click_text_start(&mut driver);

    driver.press_named(NamedKey::Delete);

    assert_eq!(driver.app().changes, vec!["bc".to_string()]);
}

#[test]
fn ctrl_a_then_ctrl_c_copies_the_whole_value() {
    let mut driver = TuiDriver::new(FieldApp::new("abc"), COLS, ROWS);
    click_text_start(&mut driver);

    driver.ctrl_char('a');
    driver.ctrl_char('c');

    assert_eq!(driver.app().copied, vec!["abc".to_string()]);
    assert!(
        driver.app().changes.is_empty(),
        "a copy must not change the value: {:?}",
        driver.app().changes
    );
}
