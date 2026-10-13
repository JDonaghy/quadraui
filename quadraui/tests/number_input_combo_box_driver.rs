#![cfg(feature = "tui")]
//! Headless `TuiDriver` conformance test for the `NumberInput` and
//! editable `ComboBox` primitives: typed-draft/validation, commit, and
//! cancel semantics for both, plus stepper/wheel stepping and dropdown
//! navigation, all driven through the real `event -> handle -> render`
//! path on the in-memory TUI backend.
//!
//! Neither primitive has a `Backend` painter of its own (none is in scope
//! for this seam issue), so this test's `render` paints a status line per
//! control via `TextDisplay`, the one primitive with an
//! already-implemented `Backend::draw_text_display`. Each primitive's own
//! `handle_key`/`click`/`handle_wheel` is a pure `&self` method that
//! returns an event without mutating the struct — `handle` below applies
//! the returned event back onto the owned field itself, matching the
//! "host applies the event" split each primitive's module doc states.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, ComboBox, ComboBoxEvent, ComboBoxHit, CompletionItemMeasure, Decoration,
    Key, NamedKey, NumberInput, NumberInputEvent, Point, Reaction, Rect, ScrollDelta, StyledSpan,
    TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

/// Which of the two controls currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Number,
    Combo,
}

const NUMBER_BOUNDS: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 10.0,
    height: 2.0,
};
const COMBO_FIELD_BOUNDS: Rect = Rect {
    x: 0.0,
    y: 3.0,
    width: 20.0,
    height: 1.0,
};
const COMBO_VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 40.0,
    height: 20.0,
};

fn combo_item_measure(_index: usize) -> CompletionItemMeasure {
    CompletionItemMeasure::new(1.0)
}

/// Minimal `AppLogic` wiring a `NumberInput` and an editable `ComboBox`
/// together with an app-owned `Focus` cursor and an event log, used only
/// by this conformance test (not a demo — this issue ships no backend
/// painter).
struct NumberComboApp {
    focus: Focus,
    number: NumberInput,
    combo: ComboBox,
    events: Vec<String>,
}

impl NumberComboApp {
    fn new() -> Self {
        let mut number = NumberInput::new(WidgetId::new("demo:volume"), 0.0, 10.0);
        number.focused = true;
        let combo = ComboBox::new(
            WidgetId::new("demo:fruit"),
            vec!["Apple".into(), "Apricot".into(), "Banana".into()],
        );
        Self {
            focus: Focus::Number,
            number,
            combo,
            events: Vec::new(),
        }
    }

    fn sync_focus_flags(&mut self) {
        self.number.focused = self.focus == Focus::Number;
        self.combo.focused = self.focus == Focus::Combo;
    }

    fn advance_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Number => Focus::Combo,
            Focus::Combo => Focus::Number,
        };
        self.sync_focus_flags();
    }

    fn apply_number_event(&mut self, event: NumberInputEvent) {
        match event {
            NumberInputEvent::ValueChanged { value, .. } => {
                self.number.value = value;
                self.events.push(format!("NumberValueChanged({value})"));
            }
            NumberInputEvent::DraftChanged { draft, .. } => {
                self.events.push(format!("NumberDraftChanged({draft})"));
                self.number.draft = Some(draft);
            }
            NumberInputEvent::Committed { value, .. } => {
                self.number.value = value;
                self.number.draft = None;
                self.events.push(format!("NumberCommitted({value})"));
            }
            NumberInputEvent::Cancelled { .. } => {
                self.number.draft = None;
                self.events.push("NumberCancelled".to_string());
            }
        }
    }

    fn apply_combo_event(&mut self, event: ComboBoxEvent) {
        match event {
            ComboBoxEvent::TextChanged { text, .. } => {
                self.events.push(format!("ComboTextChanged({text})"));
                self.combo.text = text;
                self.combo.open = true;
            }
            ComboBoxEvent::HighlightChanged { index, .. } => {
                self.events.push(format!("ComboHighlightChanged({index})"));
                self.combo.highlighted = Some(index);
            }
            ComboBoxEvent::Committed { value, .. } => {
                self.events.push(format!("ComboCommitted({value})"));
                self.combo.committed = value.clone();
                self.combo.text = value;
                self.combo.open = false;
                self.combo.highlighted = None;
            }
            ComboBoxEvent::Cancelled { .. } => {
                self.events.push("ComboCancelled".to_string());
                self.combo.text = self.combo.committed.clone();
                self.combo.open = false;
                self.combo.highlighted = None;
            }
        }
    }
}

impl AppLogic for NumberComboApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut display = TextDisplay::new(WidgetId::new("demo:status"));
        display.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(format!(
                "[{}] volume: {} draft: {:?}",
                if self.focus == Focus::Number {
                    "*"
                } else {
                    " "
                },
                self.number.value,
                self.number.draft,
            ))],
            decoration: Decoration::default(),
            timestamp: None,
        });
        display.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(format!(
                "[{}] fruit: {} text: {}",
                if self.focus == Focus::Combo { "*" } else { " " },
                self.combo.committed,
                self.combo.text,
            ))],
            decoration: Decoration::default(),
            timestamp: None,
        });
        backend.draw_text_display(Rect::new(0.0, 0.0, 40.0, 4.0), &display);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if key == Key::Named(NamedKey::Tab) {
                    self.advance_focus();
                    return Reaction::Redraw;
                }
                match self.focus {
                    Focus::Number => {
                        if let Some(e) = self.number.handle_key(&key, &modifiers) {
                            self.apply_number_event(e);
                        }
                    }
                    Focus::Combo => {
                        if let Some(e) = self.combo.handle_key(&key, &modifiers) {
                            self.apply_combo_event(e);
                        }
                    }
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let number_layout = self.number.layout(NUMBER_BOUNDS);
                let combo_layout = self.combo.layout(
                    COMBO_FIELD_BOUNDS,
                    COMBO_VIEWPORT,
                    20.0,
                    10.0,
                    combo_item_measure,
                );

                if number_layout.bounds.contains(position) {
                    let hit = number_layout.hit_test(position);
                    if let Some(e) = self.number.click(hit) {
                        self.apply_number_event(e);
                    }
                } else {
                    let hit = combo_layout.hit_test(position);
                    if hit != ComboBoxHit::Outside {
                        if let Some(e) = self.combo.click(hit) {
                            self.apply_combo_event(e);
                        }
                    }
                }
                Reaction::Redraw
            }
            UiEvent::Scroll {
                delta, position, ..
            } => {
                if self.number.layout(NUMBER_BOUNDS).bounds.contains(position) {
                    if let Some(e) = self.number.handle_wheel(delta, self.number.step) {
                        self.apply_number_event(e);
                    }
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

#[test]
fn starts_with_number_focused_and_zero_value() {
    let driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    assert!(driver.screen_contains("[*] volume: 0 draft: None"));
    assert!(driver.screen_contains("fruit:  text:"));
}

#[test]
fn tab_cycles_focus_between_number_and_combo() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    assert_eq!(driver.app().focus, Focus::Number);

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Combo);
    assert!(driver.screen_contains("[*] fruit:"));

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Number);
    assert!(driver.screen_contains("[*] volume:"));
}

#[test]
fn number_up_down_step_and_clamp_the_value() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().number.value, 1.0);

    for _ in 0..20 {
        driver.press_named(NamedKey::Up);
    }
    assert_eq!(driver.app().number.value, 10.0);

    for _ in 0..20 {
        driver.press_named(NamedKey::Down);
    }
    assert_eq!(driver.app().number.value, 0.0);
}

#[test]
fn number_typed_digits_commit_on_enter() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.type_char('7');
    assert_eq!(driver.app().number.draft, Some("7".to_string()));

    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().number.value, 7.0);
    assert_eq!(driver.app().number.draft, None);
    assert!(driver.screen_contains("[*] volume: 7 draft: None"));
}

#[test]
fn number_invalid_draft_reverts_to_current_value_on_commit() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.type_char('-');
    driver.type_char('-');
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().number.value, 0.0);
}

#[test]
fn number_escape_cancels_a_pending_draft_without_touching_value() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.type_char('5');
    assert_eq!(driver.app().number.draft, Some("5".to_string()));

    driver.press_named(NamedKey::Escape);
    assert_eq!(driver.app().number.draft, None);
    assert_eq!(driver.app().number.value, 0.0);
    assert_eq!(driver.app().events.last().unwrap(), "NumberCancelled");
}

#[test]
fn number_stepper_button_clicks_increment_without_requiring_focus() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    // Two clicks in quick succession on the same stepper column would
    // otherwise fold into a `DoubleClick` under `DoubleClickDetector`'s
    // 400ms/1.5-cell window; this test cares about two independent
    // single clicks.
    driver.set_double_click_folding(false);
    driver.press_named(NamedKey::Tab); // move focus to combo; click still works
    driver.click(9.0, 0.0); // increment button (top half of the stepper column)
    assert_eq!(driver.app().number.value, 1.0);
    driver.click(9.0, 1.0); // decrement button (bottom half)
    assert_eq!(driver.app().number.value, 0.0);
}

#[test]
fn number_wheel_steps_the_value_under_the_cursor() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 1.0),
        position: Point::new(1.0, 0.0),
    });
    assert_eq!(driver.app().number.value, 1.0);
}

#[test]
fn combo_typed_characters_filter_and_open_the_dropdown() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('a');
    driver.type_char('p');
    assert_eq!(driver.app().combo.text, "ap");
    assert!(driver.app().combo.open);
    assert_eq!(driver.app().combo.filtered_indices(), vec![0, 1]);
}

#[test]
fn combo_down_navigates_the_filtered_list_with_wraparound() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('a');
    driver.type_char('p');

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(0));
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(1));
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(0));
}

#[test]
fn combo_enter_commits_the_highlighted_option() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('a');
    driver.type_char('p');
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Down); // highlight index 1: "Apricot"

    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().combo.committed, "Apricot");
    assert_eq!(driver.app().combo.text, "Apricot");
    assert!(!driver.app().combo.open);
    assert!(driver.screen_contains("fruit: Apricot text: Apricot"));
}

#[test]
fn combo_enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('C');
    driver.type_char('a');

    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().combo.committed, "Ca");
    assert_eq!(driver.app().combo.text, "Ca");
}

#[test]
fn combo_escape_cancels_and_reverts_text_to_the_committed_value() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('a');
    driver.press_named(NamedKey::Enter); // commit "a" verbatim
    assert_eq!(driver.app().combo.committed, "a");

    driver.type_char('p');
    assert_eq!(driver.app().combo.text, "ap");

    driver.press_named(NamedKey::Escape);
    assert_eq!(driver.app().combo.text, "a");
    assert_eq!(driver.app().combo.committed, "a");
    assert_eq!(driver.app().events.last().unwrap(), "ComboCancelled");
}

#[test]
fn combo_mouse_click_on_a_dropdown_item_commits_without_requiring_focus() {
    let mut driver = TuiDriver::new(NumberComboApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.type_char('a');
    driver.type_char('p');
    assert!(driver.app().combo.open);

    // The dropdown is placed just below the field (field at y = 3,
    // height 1), so its first item sits at y = 4.
    driver.click(1.0, 4.0);
    assert_eq!(driver.app().combo.committed, "Apple");
    assert_eq!(driver.app().combo.text, "Apple");
    assert!(!driver.app().combo.open);
}
