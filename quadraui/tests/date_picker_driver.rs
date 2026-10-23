#![cfg(feature = "tui")]
//! Headless `TuiDriver` conformance test for `DatePicker`: keyboard
//! navigation (arrow/page keys), date confirmation, and locale-specific
//! date formatting, all driven through the real `event -> handle -> render`
//! path on the in-memory TUI backend.
//!
//! This harness renders `DatePicker`'s `month_grid()` /
//! `CalendarDate::format()` output through the existing `TextDisplay`
//! primitive rather than `Backend::draw_date_picker`'s own rasteriser,
//! matching the text-level assertions a host app sees —
//! `quadraui/tests/scroll_view_driver.rs` is the template this follows.
//! The pixel-level rasteriser itself is covered by `tui::date_picker`'s
//! and `primitives::date_picker::native_surface_paint`'s own unit tests.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CalendarDate, DateFormat, DateLocale, DatePicker, DatePickerEvent,
    Decoration, Key, NamedKey, Reaction, Rect, StyledSpan, TextDisplay, TextDisplayLine, UiEvent,
    Weekday, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 40.0,
    height: 12.0,
};

struct DatePickerApp {
    picker: DatePicker,
}

impl DatePickerApp {
    fn new() -> Self {
        let mut picker =
            DatePicker::new(WidgetId::new("test:date"), CalendarDate::new(2024, 3, 15));
        picker.has_focus = true;
        picker.locale = DateLocale {
            first_weekday: Weekday::Monday,
            format: DateFormat::UsSlash,
        };
        Self { picker }
    }

    fn apply(&mut self, event: Option<DatePickerEvent>) -> Reaction {
        match event {
            Some(DatePickerEvent::FocusMoved(date)) => {
                self.picker.focused = date;
                if !self.picker.is_in_visible_month(date) {
                    self.picker.visible_month = date.first_of_month();
                }
                Reaction::Redraw
            }
            Some(DatePickerEvent::DateSelected(date)) => {
                self.picker.selected = Some(date);
                Reaction::Redraw
            }
            None => Reaction::Continue,
        }
    }

    fn line(text: String) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::default(),
            timestamp: None,
        }
    }
}

impl AppLogic for DatePickerApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut display = TextDisplay::new(WidgetId::new("test:display"));
        display.append_line(Self::line(format!(
            "{:04}-{:02}",
            self.picker.visible_month.year, self.picker.visible_month.month
        )));
        for week in self.picker.month_grid().chunks(7) {
            let mut row = String::new();
            for date in week {
                let marker = if self.picker.selected == Some(*date) {
                    "*"
                } else if self.picker.focused == *date {
                    "["
                } else {
                    " "
                };
                row.push_str(&format!("{marker}{:>2} ", date.day));
            }
            display.append_line(Self::line(row));
        }
        display.append_line(Self::line(format!(
            "Focused: {}",
            self.picker.focused.format(self.picker.locale.format)
        )));
        if let Some(selected) = self.picker.selected {
            display.append_line(Self::line(format!(
                "Selected: {}",
                selected.format(self.picker.locale.format)
            )));
        }
        backend.draw_text_display(VIEWPORT, &display);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => {
                let key_name = match key {
                    Key::Named(NamedKey::Left) => "Left",
                    Key::Named(NamedKey::Right) => "Right",
                    Key::Named(NamedKey::Up) => "Up",
                    Key::Named(NamedKey::Down) => "Down",
                    Key::Named(NamedKey::PageUp) => "PageUp",
                    Key::Named(NamedKey::PageDown) => "PageDown",
                    Key::Named(NamedKey::Enter) => "Enter",
                    _ => return Reaction::Continue,
                };
                let event = self.picker.navigate(key_name);
                self.apply(event)
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> TuiDriver<DatePickerApp> {
    TuiDriver::new(DatePickerApp::new(), 40, 12)
}

#[test]
fn starts_showing_the_focused_months_grid_with_the_focused_day_marked() {
    let driver = driver();
    assert!(driver.screen_contains("2024-03"));
    // March 15, 2024 is the initially focused day.
    assert!(driver.screen_contains("[15"));
}

#[test]
fn right_arrow_moves_focus_forward_a_day() {
    let mut driver = driver();
    driver.press_named(NamedKey::Right);
    assert!(driver.screen_contains("[16"));
    assert!(driver.screen_contains(&driver.app().picker.focused.format(DateFormat::UsSlash)));
}

#[test]
fn down_arrow_moves_focus_forward_a_week() {
    let mut driver = driver();
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 22));
}

#[test]
fn page_down_moves_focus_to_the_next_month_and_updates_the_visible_grid() {
    let mut driver = driver();
    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 4, 15));
    assert_eq!(
        driver.app().picker.visible_month,
        CalendarDate::new(2024, 4, 1)
    );
    assert!(driver.screen_contains("2024-04"));
}

#[test]
fn enter_confirms_the_focused_date_as_selected() {
    let mut driver = driver();
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().picker.selected,
        Some(CalendarDate::new(2024, 3, 16))
    );
    assert!(driver.screen_contains("Selected: 03/16/2024"));
}

#[test]
fn focused_date_renders_in_the_configured_locale_format() {
    // `UsSlash` is configured on the picker itself in `DatePickerApp::new`.
    let driver = driver();
    assert!(driver.screen_contains("Focused: 03/15/2024"));
}
