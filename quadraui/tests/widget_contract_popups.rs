#![cfg(any(feature = "tui", feature = "gtk"))]
//! Black-box tests for widget-owned popups: a `ComboBox`'s dropdown
//! opens through `Interactive::popups`, paints above widgets painted
//! after the box, is hit-tested before them through the app's
//! `FrameTree`, and dismisses on an outside press or Escape.
//!
//! Every leg drives a real driver (`TuiDriver`, `GtkDriver`) through the
//! shared routing engine and asserts on what the frame painted; the app
//! below only applies the `UiEvent`s the box emits back onto the
//! descriptor it owns.

use quadraui::{
    AppLogic, Backend, Button, ComboBox, DrawsPrimitives, FormEvent, FrameTree, Reaction, Rect,
    UiEvent, WidgetId,
};

const OPTIONS: [&str; 3] = ["Apple", "Apricot", "Banana"];

/// The box's field, the button painted after it directly under the
/// field (where the open list drops), and a second button off to the
/// side that the list never covers.
#[derive(Clone, Copy)]
struct Geometry {
    combo: Rect,
    under: Rect,
    aside: Rect,
}

/// One combo box plus two buttons painted after it, all routed through
/// the app's `FrameTree`; records every outcome it is told to apply.
struct PopupApp {
    geometry: Geometry,
    combo: ComboBox,
    under: Button,
    aside: Button,
    /// While set, a modal registered on the backend's `ModalStack`
    /// paints over the area the list drops into.
    modal: bool,
    applied: Vec<String>,
}

/// The modal [`PopupApp::modal`] paints and registers.
const MODAL: &str = "modal";

impl PopupApp {
    fn new(geometry: Geometry) -> Self {
        let mut combo = ComboBox::new(
            WidgetId::new("form:fruit"),
            OPTIONS.iter().map(|s| (*s).to_string()).collect(),
        );
        combo.focused = true;
        Self {
            geometry,
            combo,
            under: Button::new(WidgetId::new("under"), "UNDERNEATH"),
            aside: Button::new(WidgetId::new("aside"), "ASIDE"),
            modal: false,
            applied: Vec::new(),
        }
    }
}

impl AppLogic for PopupApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let g = self.geometry;
        let _ = backend.draw_dyn(g.combo, &self.combo, &());
        let _ = backend.draw_dyn(g.under, &self.under, &());
        let _ = backend.draw_dyn(g.aside, &self.aside, &());
        let modal = WidgetId::new(MODAL);
        let stack = backend.modal_stack_handle();
        if self.modal {
            let bounds = Rect::new(g.combo.x, g.under.y, g.combo.width, g.under.height * 3.0);
            stack.borrow_mut().push(modal, bounds);
            let _ = backend.draw_dyn(bounds, &Button::new(modal, "MODAL"), &());
        } else {
            stack.borrow_mut().pop(&modal);
        }
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        let g = self.geometry;
        vec![
            (self.combo.id, g.combo),
            (self.under.id, g.under),
            (self.aside.id, g.aside),
        ]
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let g = self.geometry;
        let mut tree = FrameTree::new();
        tree.push(self.combo.id, g.combo, Box::new(self.combo.clone()));
        tree.push(self.under.id, g.under, Box::new(self.under.clone()));
        tree.push(self.aside.id, g.aside, Box::new(self.aside.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Form(id, form) if id == self.combo.id => match form {
                FormEvent::ComboBoxTextChanged { value, .. } => {
                    self.applied.push(format!("text({value})"));
                    self.combo.text = value;
                    self.combo.open = true;
                }
                FormEvent::ComboBoxHighlightChanged { index, .. } => {
                    self.applied.push(format!("highlight({index})"));
                    self.combo.highlighted = Some(index);
                }
                FormEvent::ComboBoxCommitted { value, .. } => {
                    self.applied.push(format!("committed({value})"));
                    self.combo.committed = value.clone();
                    self.combo.text = value;
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
                FormEvent::ComboBoxCancelled { .. } => {
                    self.applied.push("cancelled".to_string());
                    self.combo.text = self.combo.committed.clone();
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
                _ => return Reaction::Continue,
            },
            UiEvent::ButtonActivated { widget } => {
                self.applied.push(format!("button({widget})"));
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::interactive::SubTarget;
    use quadraui::primitive::painted_sub_center;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const GEOMETRY: Geometry = Geometry {
        combo: Rect::new(1.0, 1.0, 16.0, 1.0),
        under: Rect::new(1.0, 2.0, 16.0, 1.0),
        aside: Rect::new(24.0, 1.0, 10.0, 1.0),
    };

    fn driver() -> TuiDriver<PopupApp> {
        let mut driver = TuiDriver::new(PopupApp::new(GEOMETRY), 40, 12);
        driver.set_double_click_folding(false);
        driver.render();
        driver
    }

    fn open_by_click(driver: &mut TuiDriver<PopupApp>) {
        driver.click(GEOMETRY.combo.x + 1.0, GEOMETRY.combo.y);
        assert!(driver.app().combo.open, "a field click opens the list");
    }

    fn row(driver: &TuiDriver<PopupApp>, y: f32) -> String {
        driver
            .screen()
            .lines()
            .nth(y as usize)
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn the_open_list_paints_over_a_widget_painted_after_the_box() {
        let mut driver = driver();
        assert!(
            row(&driver, GEOMETRY.under.y).contains("UNDERNEATH"),
            "{}",
            driver.screen()
        );

        open_by_click(&mut driver);
        let list_row = row(&driver, GEOMETRY.under.y);
        assert!(list_row.contains("Apple"), "{}", driver.screen());
        assert!(!list_row.contains("UNDERNEATH"), "{}", driver.screen());
        for option in OPTIONS {
            assert!(driver.screen_contains(option), "{}", driver.screen());
        }
        let popups = quadraui::popup::painted_popups();
        assert_eq!(popups.len(), 1);
        assert_eq!(popups[0].owner, driver.app().combo.id);
        assert_eq!(popups[0].rect, Rect::new(1.0, 2.0, 16.0, 3.0));
    }

    #[test]
    fn the_keyboard_opens_navigates_and_commits() {
        let mut driver = driver();
        // Focus the box, then close the list it dropped, so the keyboard
        // does the opening.
        open_by_click(&mut driver);
        driver.app_mut().combo.open = false;
        driver.render();
        assert!(!driver.screen_contains("Banana"));

        driver.press_named(NamedKey::Down);
        assert!(driver.app().combo.open, "Down opens a closed box");
        assert!(driver.screen_contains("Banana"), "{}", driver.screen());

        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(2));

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Banana");
        assert!(!driver.app().combo.open);
        assert!(quadraui::popup::painted_popups().is_empty());
        assert!(
            row(&driver, GEOMETRY.under.y).contains("UNDERNEATH"),
            "closing uncovers the widget beneath:\n{}",
            driver.screen()
        );
    }

    #[test]
    fn a_click_on_a_row_selects_it_and_never_reaches_the_widget_beneath() {
        let mut driver = driver();
        open_by_click(&mut driver);

        let dropdown = driver.app().combo.dropdown_id();
        let (x, y) = painted_sub_center(&dropdown, SubTarget(0)).expect("first row painted");
        assert_eq!(
            y.floor(),
            GEOMETRY.under.y,
            "the first row covers the button"
        );
        driver.click(x, y);

        assert_eq!(driver.app().combo.committed, "Apple");
        assert!(!driver.app().combo.open);
        assert!(
            !driver.app().applied.iter().any(|a| a.starts_with("button")),
            "{:?}",
            driver.app().applied
        );
    }

    #[test]
    fn an_outside_click_dismisses_the_list_and_still_reaches_its_target() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.type_char('b');
        assert_eq!(driver.app().combo.text, "b");

        driver.click(GEOMETRY.aside.x + 1.0, GEOMETRY.aside.y);
        assert!(!driver.app().combo.open);
        assert_eq!(driver.app().combo.text, "", "the edit is cancelled");
        let applied = &driver.app().applied;
        let cancelled = applied.iter().position(|a| a == "cancelled");
        let pressed = applied.iter().position(|a| a == "button(aside)");
        assert!(
            cancelled.is_some() && pressed.is_some(),
            "dismissed, then the press routed on: {applied:?}"
        );
        assert!(!driver.screen_contains("Banana"), "{}", driver.screen());
    }

    #[test]
    fn escape_dismisses_the_list() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert!(!driver.app().combo.open);
        assert_eq!(driver.app().applied.last().unwrap(), "cancelled");
        assert!(!driver.screen_contains("Apricot"), "{}", driver.screen());
    }

    #[test]
    fn an_outside_click_cancels_an_edit_that_matches_nothing() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.type_char('z');
        assert_eq!(driver.app().combo.text, "z");

        driver.click(GEOMETRY.aside.x + 1.0, GEOMETRY.aside.y);
        assert!(!driver.app().combo.open);
        assert_eq!(driver.app().combo.text, "", "the edit is cancelled");
        let applied = &driver.app().applied;
        assert!(applied.iter().any(|a| a == "cancelled"), "{applied:?}");
        assert!(applied.iter().any(|a| a == "button(aside)"), "{applied:?}");
    }

    #[test]
    fn a_modal_paints_above_an_open_list() {
        let mut driver = driver();
        open_by_click(&mut driver);
        assert!(driver.screen_contains("Apricot"), "{}", driver.screen());

        driver.app_mut().modal = true;
        driver.render();
        assert!(driver.screen_contains("MODAL"), "{}", driver.screen());
        for option in OPTIONS {
            assert!(!driver.screen_contains(option), "{}", driver.screen());
        }
        assert!(quadraui::popup::painted_popups().is_empty());

        driver.app_mut().modal = false;
        driver.render();
        assert!(driver.app().combo.open, "the list was hidden, not closed");
        assert!(driver.screen_contains("Apricot"), "{}", driver.screen());
    }

    /// Every cell the open list covers, with its style.
    fn list_cells(driver: &TuiDriver<PopupApp>) -> Vec<String> {
        let rect = quadraui::popup::painted_popups()[0].rect;
        let screen = driver.screen();
        let lines: Vec<&str> = screen.lines().collect();
        let mut cells = Vec::new();
        for y in rect.y as u16..(rect.y + rect.height) as u16 {
            for x in rect.x as u16..(rect.x + rect.width) as u16 {
                let ch = lines[y as usize].chars().nth(x as usize);
                cells.push(format!("{ch:?} {:?}", driver.style_at(x, y)));
            }
        }
        cells
    }

    #[test]
    fn the_focus_ring_of_a_widget_under_the_list_stays_beneath_it() {
        let mut driver = driver();
        open_by_click(&mut driver);
        let before = list_cells(&driver);

        driver.press_named(NamedKey::Tab);
        driver.render();
        assert_eq!(
            driver.backend().focus_tree().focused().map(|(id, _)| *id),
            Some(driver.app().under.id),
            "Tab moves focus to the widget under the list"
        );
        assert!(driver.app().combo.open);
        assert_eq!(list_cells(&driver), before, "{}", driver.screen());
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;
    use quadraui::interactive::SubTarget;
    use quadraui::primitive::painted_sub_center;
    use quadraui::{NamedKey, Theme};

    const GEOMETRY: Geometry = Geometry {
        combo: Rect::new(10.0, 10.0, 200.0, 24.0),
        under: Rect::new(10.0, 34.0, 200.0, 120.0),
        aside: Rect::new(260.0, 10.0, 100.0, 30.0),
    };

    fn driver() -> GtkDriver<PopupApp> {
        let mut driver = GtkDriver::new(PopupApp::new(GEOMETRY), 400, 240);
        driver.set_double_click_folding(false);
        driver.render();
        driver
    }

    fn open_by_click(driver: &mut GtkDriver<PopupApp>) {
        driver.click(GEOMETRY.combo.x + 5.0, GEOMETRY.combo.y + 5.0);
        assert!(driver.app().combo.open, "a field click opens the list");
    }

    fn rgb(c: quadraui::Color) -> (u8, u8, u8) {
        (c.r, c.g, c.b)
    }

    fn first_row(driver: &GtkDriver<PopupApp>) -> (f32, f32) {
        let dropdown = driver.app().combo.dropdown_id();
        painted_sub_center(&dropdown, SubTarget(0)).expect("first row painted")
    }

    #[test]
    fn the_open_list_paints_over_a_widget_painted_after_the_box() {
        let mut driver = driver();
        open_by_click(&mut driver);

        let (x, y) = first_row(&driver);
        assert!(GEOMETRY.under.contains(quadraui::Point::new(x, y)));
        let popup = quadraui::popup::painted_popups()[0];
        // Sample near the list's right edge: labels start at the left, and
        // how far a glyph's antialiasing reaches depends on the system font.
        let sample_x = (popup.rect.x + popup.rect.width - 6.0) as i32;
        assert!(GEOMETRY
            .under
            .contains(quadraui::Point::new(sample_x as f32, y)));
        let px = driver.pixel(sample_x, y as i32);
        assert_eq!(
            px,
            rgb(Theme::default().completion_bg),
            "the list's background, not the button painted after it"
        );
        assert!(driver.screen_contains("Banana"));

        driver.press_named(NamedKey::Escape);
        let closed = driver.pixel(sample_x, y as i32);
        assert_ne!(
            closed,
            rgb(Theme::default().completion_bg),
            "with the list closed the button shows through"
        );
    }

    #[test]
    fn the_keyboard_opens_navigates_and_commits() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.app_mut().combo.open = false;
        driver.render();

        driver.press_named(NamedKey::Down);
        assert!(driver.app().combo.open);
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(1));
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Apricot");
        assert!(quadraui::popup::painted_popups().is_empty());
    }

    #[test]
    fn a_click_on_a_row_selects_it_and_never_reaches_the_widget_beneath() {
        let mut driver = driver();
        open_by_click(&mut driver);
        let dropdown = driver.app().combo.dropdown_id();
        let (x, y) = painted_sub_center(&dropdown, SubTarget(2)).expect("third row painted");
        driver.click(x, y);
        assert_eq!(driver.app().combo.committed, "Banana");
        assert!(
            !driver.app().applied.iter().any(|a| a.starts_with("button")),
            "{:?}",
            driver.app().applied
        );
    }

    #[test]
    fn an_outside_click_dismisses_the_list_and_still_reaches_its_target() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.click(GEOMETRY.aside.x + 10.0, GEOMETRY.aside.y + 10.0);
        assert!(!driver.app().combo.open);
        let applied = &driver.app().applied;
        assert!(applied.iter().any(|a| a == "cancelled"), "{applied:?}");
        assert!(applied.iter().any(|a| a == "button(aside)"), "{applied:?}");
    }

    #[test]
    fn escape_dismisses_the_list() {
        let mut driver = driver();
        open_by_click(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert!(!driver.app().combo.open);
        assert!(quadraui::popup::painted_popups().is_empty());
    }
}
