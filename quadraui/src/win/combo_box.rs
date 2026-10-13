//! Windows (Direct2D) rasteriser for [`crate::ComboBox`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::combo_box::native_surface_paint::paint`] — see
//! that function's documentation for details. Callers reach the paint
//! through [`crate::Backend::draw_combo_box`]. Filtering, highlight
//! movement, commit and cancel semantics live on `ComboBox` itself; this
//! module only turns a resolved [`crate::ComboBoxLayout`] into pixels,
//! mirroring `gtk::combo_box`'s structure with
//! [`crate::win::testing::WinDriver`] standing in for `GtkDriver`.

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        Backend, ComboBox, ComboBoxEvent, ComboBoxHit, ComboBoxLayout, CompletionItemMeasure,
        NamedKey, Rect, UiEvent, WidgetId,
    };

    const FIELD_BOUNDS: Rect = Rect {
        x: 0.0,
        y: 3.0,
        width: 20.0,
        height: 1.0,
    };
    const VIEWPORT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 20.0,
    };

    fn item_measure(_index: usize) -> CompletionItemMeasure {
        CompletionItemMeasure::new(1.0)
    }

    fn sample_layout(combo: &ComboBox) -> ComboBoxLayout {
        combo.layout(FIELD_BOUNDS, VIEWPORT, 20.0, 10.0, item_measure)
    }

    /// Minimal `AppLogic` wrapping a single focused `ComboBox`, painted
    /// through the real `Backend::draw_combo_box` — used only by this
    /// driver test, mirroring `gtk::combo_box`'s `ComboBoxApp` but routed
    /// through `WinDriver`.
    struct ComboBoxApp {
        combo: ComboBox,
        events: Vec<String>,
    }

    impl ComboBoxApp {
        fn new() -> Self {
            let mut combo = ComboBox::new(
                WidgetId::new("test:fruit"),
                vec![
                    "Apple".to_string(),
                    "Apricot".to_string(),
                    "Banana".to_string(),
                ],
            );
            combo.focused = true;
            Self {
                combo,
                events: Vec::new(),
            }
        }

        fn apply(&mut self, event: ComboBoxEvent) {
            match event {
                ComboBoxEvent::TextChanged { text, .. } => {
                    self.events.push(format!("TextChanged({text})"));
                    self.combo.text = text;
                    self.combo.open = true;
                }
                ComboBoxEvent::HighlightChanged { index, .. } => {
                    self.events.push(format!("HighlightChanged({index})"));
                    self.combo.highlighted = Some(index);
                }
                ComboBoxEvent::Committed { value, .. } => {
                    self.events.push(format!("Committed({value})"));
                    self.combo.committed = value.clone();
                    self.combo.text = value;
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
                ComboBoxEvent::Cancelled { .. } => {
                    self.events.push("Cancelled".to_string());
                    self.combo.text = self.combo.committed.clone();
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
            }
        }
    }

    impl AppLogic for ComboBoxApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let layout = sample_layout(&self.combo);
            backend.draw_combo_box(&self.combo, &layout);
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    if let Some(e) = self.combo.handle_key(&key, &modifiers) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::MouseDown { position, .. } => {
                    let hit = sample_layout(&self.combo).hit_test(position);
                    if hit != ComboBoxHit::Outside {
                        if let Some(e) = self.combo.click(hit) {
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
    fn typed_characters_filter_and_open_the_dropdown() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");
        assert!(driver.app().combo.open);
        assert_eq!(driver.app().combo.filtered_indices(), vec![0, 1]);
    }

    #[test]
    fn down_navigates_the_filtered_list_with_wraparound() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
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
    fn enter_commits_the_highlighted_option() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down); // highlight index 1: "Apricot"

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Apricot");
        assert_eq!(driver.app().combo.text, "Apricot");
        assert!(!driver.app().combo.open);
    }

    #[test]
    fn enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('C');
        driver.type_char('a');

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Ca");
        assert_eq!(driver.app().combo.text, "Ca");
    }

    #[test]
    fn escape_cancels_and_reverts_text_to_the_committed_value() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.press_named(NamedKey::Enter); // commit "a" verbatim
        assert_eq!(driver.app().combo.committed, "a");

        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");

        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().combo.text, "a");
        assert_eq!(driver.app().combo.committed, "a");
        assert_eq!(driver.app().events.last().unwrap(), "Cancelled");
    }

    #[test]
    fn mouse_click_on_a_dropdown_item_commits_without_requiring_focus() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
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
}
