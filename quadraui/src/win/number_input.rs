//! Windows (Direct2D) rasteriser for [`crate::NumberInput`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::number_input::native_surface_paint::paint`] — see
//! that function's documentation for details. Callers reach the paint
//! through [`crate::Backend::draw_number_input`]. Validation, commit and
//! cancel semantics live on `NumberInput` itself; this module only turns a
//! resolved [`crate::NumberInputLayout`] into pixels, mirroring
//! `gtk::number_input`'s structure with [`crate::win::testing::WinDriver`]
//! standing in for `GtkDriver`.

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        Backend, NamedKey, NumberInput, NumberInputEvent, NumberInputLayout, Point, Rect,
        ScrollDelta, UiEvent, WidgetId,
    };

    const BOUNDS: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 20.0,
    };

    fn sample_layout() -> NumberInputLayout {
        NumberInput::new(WidgetId::new("test:volume"), 0.0, 10.0).layout(BOUNDS)
    }

    /// Minimal `AppLogic` wrapping a single focused `NumberInput`, painted
    /// through the real `Backend::draw_number_input` — used only by this
    /// driver test, mirroring `gtk::number_input`'s `NumberInputApp` but
    /// routed through `WinDriver`.
    struct NumberInputApp {
        number: NumberInput,
        events: Vec<String>,
    }

    impl NumberInputApp {
        fn new() -> Self {
            let mut number = NumberInput::new(WidgetId::new("test:volume"), 0.0, 10.0);
            number.focused = true;
            Self {
                number,
                events: Vec::new(),
            }
        }

        fn apply(&mut self, event: NumberInputEvent) {
            match event {
                NumberInputEvent::ValueChanged { value, .. } => {
                    self.number.value = value;
                    self.events.push(format!("ValueChanged({value})"));
                }
                NumberInputEvent::DraftChanged { draft, .. } => {
                    self.events.push(format!("DraftChanged({draft})"));
                    self.number.draft = Some(draft);
                }
                NumberInputEvent::Committed { value, .. } => {
                    self.number.value = value;
                    self.number.draft = None;
                    self.events.push(format!("Committed({value})"));
                }
                NumberInputEvent::Cancelled { .. } => {
                    self.number.draft = None;
                    self.events.push("Cancelled".to_string());
                }
            }
        }
    }

    impl AppLogic for NumberInputApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let layout = self.number.layout(BOUNDS);
            backend.draw_number_input(&self.number, &layout);
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    if let Some(e) = self.number.handle_key(&key, &modifiers) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::MouseDown { position, .. } => {
                    let hit = self.number.layout(BOUNDS).hit_test(position);
                    if let Some(e) = self.number.click(hit) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::Scroll {
                    delta, position, ..
                } => {
                    if self.number.layout(BOUNDS).bounds.contains(position) {
                        if let Some(e) = self.number.handle_wheel(delta, self.number.step) {
                            self.apply(e);
                        }
                    }
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            }
        }
    }

    #[test]
    fn stepper_button_clicks_increment_and_decrement_the_value() {
        let mut driver = WinDriver::new(NumberInputApp::new(), 80, 40);
        let layout = sample_layout();
        let inc = layout.increment;
        let dec = layout.decrement;

        // Unlike `gtk::number_input`'s counterpart, this doesn't disable
        // double-click folding first: the increment/decrement rects split
        // `BOUNDS`' height in half (10.0 DIPs apart here), which already
        // exceeds `WIN_DOUBLE_CLICK_RADIUS` (4.0), so
        // `DoubleClickDetector::process` (both axes must be within radius
        // to fold) never folds these two clicks regardless.
        driver.click(inc.x + inc.width / 2.0, inc.y + inc.height / 2.0);
        assert_eq!(driver.app().number.value, 1.0);

        driver.click(dec.x + dec.width / 2.0, dec.y + dec.height / 2.0);
        assert_eq!(driver.app().number.value, 0.0);
    }

    #[test]
    fn mouse_wheel_steps_the_value_under_the_cursor() {
        let mut driver = WinDriver::new(NumberInputApp::new(), 80, 40);
        let text_area = sample_layout().text_area;
        let position = Point::new(text_area.x + 1.0, text_area.y + 1.0);

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, 1.0),
            position,
        });
        assert_eq!(driver.app().number.value, 1.0);
    }

    #[test]
    fn typed_digits_commit_on_enter_and_clear_the_draft() {
        let mut driver = WinDriver::new(NumberInputApp::new(), 80, 40);
        driver.type_char('7');
        assert_eq!(driver.app().number.draft, Some("7".to_string()));

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().number.value, 7.0);
        assert_eq!(driver.app().number.draft, None);
        assert_eq!(driver.app().events.last().unwrap(), "Committed(7)");
    }
}
