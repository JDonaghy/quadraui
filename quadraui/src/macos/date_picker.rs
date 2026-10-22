//! macOS rasteriser for [`crate::DatePicker`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::date_picker::native_surface_paint::paint`] — see
//! that function's documentation for details; [`crate::macos::backend`]'s
//! `Backend::draw_date_picker` impl reaches it. Focus movement, month
//! navigation and day/time selection semantics live on `DatePicker`
//! itself (`DatePicker::navigate`). This module carries no production
//! code of its own: it holds only the macOS-side round-trip tests
//! exercising that full path, mirroring `win::date_picker`'s structure
//! with [`crate::macos::testing::MacDriver`] standing in for
//! `WinDriver`.

#[cfg(test)]
mod tests {
    use crate::macos::testing::MacDriver;
    use crate::runner::{AppLogic, Reaction};
    use crate::theme::Theme;
    use crate::{
        Backend, CalendarDate, DatePicker, DatePickerEvent, DatePickerLayout, Key, NamedKey,
        UiEvent, WidgetId,
    };

    /// Point metrics for the layout every test paints. Cells are large
    /// relative to Menlo 14pt (the font [`MacDriver::new`] installs) so a
    /// day digit — drawn from the cell's top-left corner by
    /// `MacBackend`'s `surface_draw_text_run` — leaves the lower-right of
    /// the cell as clear fill for
    /// [`selecting_a_day_fills_its_cell_with_the_selected_background`] to
    /// sample.
    const CELL_W: f32 = 30.0;
    const CELL_H: f32 = 30.0;
    const HEADER_H: f32 = 20.0;

    /// The surface is sized to exactly the painted rect: `CELL_W * 7`
    /// wide, `HEADER_H + CELL_H * 6` tall (six complete week rows).
    const SURFACE_W: u32 = 210;
    const SURFACE_H: u32 = 200;

    /// The geometry `DatePickerApp::render` hands to
    /// `Backend::draw_date_picker`. Tests resolve it again to read a
    /// cell's exact `bounds` without re-deriving the grid.
    fn sample_layout(picker: &DatePicker) -> DatePickerLayout {
        picker.layout(0.0, 0.0, CELL_W, CELL_H, HEADER_H, 0.0)
    }

    /// Minimal `AppLogic` wrapping a single focused `DatePicker`, painted
    /// through the real `Backend::draw_date_picker` — used only by this
    /// driver test, mirroring `win::date_picker`'s `DatePickerApp` but
    /// routed through `MacDriver`.
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
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        driver.press_named(NamedKey::Left);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 14));
    }

    #[test]
    fn right_arrow_moves_focus_forward_one_day() {
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        driver.press_named(NamedKey::Right);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 16));
    }

    #[test]
    fn up_and_down_arrows_move_focus_by_a_week() {
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 22));
        driver.press_named(NamedKey::Up);
        driver.press_named(NamedKey::Up);
        assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 8));
    }

    #[test]
    fn page_up_and_page_down_navigate_months_and_update_the_visible_month() {
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
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
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().picker.selected,
            Some(CalendarDate::new(2024, 3, 16))
        );
    }

    /// Real paint assertions against `draw_date_picker`'s output — the
    /// month/year header label, the prev/next glyphs and a day-of-month
    /// digit must all land in `MacBackend`'s painted-text recording.
    #[test]
    fn header_label_and_day_glyphs_are_painted() {
        let driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        assert!(
            driver.screen_contains("2024-03"),
            "month/year header label should paint"
        );
        assert!(driver.screen_contains("<"), "prev-month glyph should paint");
        assert!(driver.screen_contains(">"), "next-month glyph should paint");
        assert!(
            driver.screen_contains("15"),
            "day-15 digit should paint — it is both today and the initial focus"
        );
    }

    /// Selecting a day fills its cell with `theme.selected_bg` — sampled
    /// from the cell's own `DayCell::bounds` in the resolved layout (not
    /// the day digit's text-run box), inset far enough from the top-left
    /// to clear the glyph and from the edges to clear the focus ring,
    /// matching `primitives::progress`'s pixel-sampling precedent
    /// (`paint_via_backend` + `surface.pixel`).
    #[test]
    fn selecting_a_day_fills_its_cell_with_the_selected_background() {
        let mut driver = MacDriver::new(DatePickerApp::new(), SURFACE_W, SURFACE_H);
        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        let selected = CalendarDate::new(2024, 3, 16);
        assert_eq!(driver.app().picker.selected, Some(selected));

        let layout = sample_layout(&driver.app().picker);
        let cell = layout
            .day_cells
            .iter()
            .find(|c| c.is_selected)
            .expect("the selected day has a cell in the resolved layout");
        assert_eq!(cell.date, selected);

        let sample_x = (cell.bounds.x + cell.bounds.width - 5.0) as u32;
        let sample_y = (cell.bounds.y + cell.bounds.height - 5.0) as u32;
        let theme = Theme::default();
        let (r, g, b, _) = driver.pixel(sample_x, sample_y);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "selected day's cell should fill with the theme's selection background"
        );
    }
}
