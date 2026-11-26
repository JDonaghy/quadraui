//! Windows (Direct2D) rasteriser for [`crate::ComboBox`].
//!
//! Painting is the shared
//! [`crate::primitives::combo_box::native_surface_paint`], reached
//! through [`crate::DrawsPrimitives::draw_dyn`]; the open dropdown is a
//! widget-owned popup the runner paints above the frame. Filtering,
//! highlight movement, commit and cancel semantics live on `ComboBox`
//! itself.
//! Mirrors `gtk::combo_box`'s tests with
//! [`crate::win::testing::WinDriver`] standing in for `GtkDriver`.

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use crate::backend::DrawsPrimitives;
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        Backend, ComboBox, ComboBoxEvent, FormEvent, FrameTree, NamedKey, Rect, UiEvent, WidgetId,
    };

    const FIELD_BOUNDS: Rect = Rect {
        x: 0.0,
        y: 3.0,
        width: 200.0,
        height: 24.0,
    };

    /// Minimal `AppLogic` wrapping a single focused `ComboBox`, painted
    /// through the real `DrawsPrimitives::draw_dyn` — used only by this
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
            backend.draw_dyn(FIELD_BOUNDS, &self.combo, &());
        }

        /// The box is routed through the widget-contract engine, so a
        /// press on its open dropdown reaches it as a popup event.
        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(self.combo.id, FIELD_BOUNDS, Box::new(self.combo.clone()));
            tree
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    if let Some(e) = self.combo.handle_key(&key, &modifiers) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCommitted { id, value }) => {
                    self.apply(ComboBoxEvent::Committed { id, value });
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCancelled { id }) => {
                    self.apply(ComboBoxEvent::Cancelled { id });
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
        let mut driver = WinDriver::new(ComboBoxApp::new(), 400, 300);
        driver.type_char('a');
        driver.type_char('p');
        assert!(driver.app().combo.open);

        let dropdown = driver.app().combo.dropdown_id();
        let (x, y) =
            crate::primitive::painted_sub_center(&dropdown, crate::interactive::SubTarget(0))
                .expect("the open dropdown paints its first row");
        assert!(y > FIELD_BOUNDS.y, "the list hangs below the field");
        driver.click(x, y);
        assert_eq!(driver.app().combo.committed, "Apple");
        assert_eq!(driver.app().combo.text, "Apple");
        assert!(!driver.app().combo.open);
    }
}
