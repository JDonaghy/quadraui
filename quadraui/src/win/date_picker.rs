//! Windows rasteriser for [`crate::DatePicker`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::date_picker::native_surface_paint::paint`] — see
//! that function's documentation for details; [`crate::win::backend`]'s
//! `Backend::draw_date_picker` impl reaches it. Focus movement, month
//! navigation and day/time selection semantics live on `DatePicker`
//! itself (`DatePicker::navigate`). This module holds only the Win-side
//! `WinDriver` round-trip tests exercising that full path, mirroring
//! `gtk::date_picker`'s structure with [`crate::win::testing::WinDriver`]
//! standing in for `GtkDriver`.

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        Backend, CalendarDate, DatePicker, DatePickerEvent, DatePickerLayout, Key, NamedKey,
        UiEvent, WidgetId,
    };

    fn sample_layout(picker: &DatePicker) -> DatePickerLayout {
        picker.layout(0.0, 0.0, 4.0, 1.0, 1.0, 0.0)
    }

    /// Minimal `AppLogic` wrapping a single focused `DatePicker`, painted
    /// through the real `Backend::draw_date_picker` — used only by this
    /// driver test, mirroring `gtk::date_picker`'s `DatePickerApp` but
    /// routed through `WinDriver`.
    struct DatePickerApp {
        picker: DatePicker,
    }

    impl DatePickerApp {
        fn new() -> Self {
            let mut picker =
                DatePicker::new(WidgetId::new("test:date"), CalendarDate::new(2024, 3, 15));
            picker.has_focus = true;
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
                _ => Reaction::Continue,
            }
        }
    }

    impl AppLogic for DatePickerApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let layout = sample_layout(&self.picker);
            backend.draw_date_picker(&self.picker, &layout);
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

    #[test]
    fn left_arrow_moves_focus_back_one_day() {
        let mut driver = WinDriver::new(DatePickerApp::new(), 80, 40);
        driver.press_named(NamedKey::Left);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 14));
    }

    #[test]
    fn right_arrow_moves_focus_forward_one_day() {
        let mut driver = WinDriver::new(DatePickerApp::new(), 80, 40);
        driver.press_named(NamedKey::Right);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 16));
    }

    #[test]
    fn up_and_down_arrows_move_focus_by_a_week() {
        let mut driver = WinDriver::new(DatePickerApp::new(), 80, 40);
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 22));
        driver.press_named(NamedKey::Up);
        driver.press_named(NamedKey::Up);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 8));
    }

    #[test]
    fn page_up_and_page_down_navigate_months_and_update_the_visible_month() {
        let mut driver = WinDriver::new(DatePickerApp::new(), 80, 40);
        driver.press_named(NamedKey::PageDown);
        let app = driver.app();
        assert_eq!(app.picker.focused, CalendarDate::new(2024, 4, 15));
        assert_eq!(app.picker.visible_month, CalendarDate::new(2024, 4, 1));

        driver.press_named(NamedKey::PageUp);
        let app = driver.app();
        assert_eq!(app.picker.focused, CalendarDate::new(2024, 3, 15));
        assert_eq!(app.picker.visible_month, CalendarDate::new(2024, 3, 1));
    }

    #[test]
    fn enter_selects_the_focused_day() {
        let mut driver = WinDriver::new(DatePickerApp::new(), 80, 40);
        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().picker.selected,
            Some(CalendarDate::new(2024, 3, 16))
        );
    }
}
