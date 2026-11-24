#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ComboBox` on the widget contract: the
//! box reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned `ComboBox`,
//! which the next `render` paints.
//!
//! Never a direct call into `ComboBox::handle_key`/`Primitive::layout`
//! that would skip the routing engine; the direct-dispatch leg an
//! out-of-process binding host takes is
//! `primitives::combo_box::interactive_tests`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, ComboBox, FormEvent, FrameTree, Key, Modifiers, NamedKey, Reaction, Rect,
    ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 32;
const ROWS: u16 = 12;
/// One row for the field, five below it for the dropdown.
const BOX: Rect = Rect::new(1.0, 1.0, 16.0, 6.0);

const OPTIONS: [&str; 3] = ["Apple", "Apricot", "Banana"];

/// One combo box, drawn and routed as a `Surface::Widget`, plus a record
/// of every event the host was told to apply.
struct ComboApp {
    combo: ComboBox,
    applied: Vec<String>,
}

impl ComboApp {
    fn new() -> Self {
        let mut combo = ComboBox::new(
            WidgetId::new("form:fruit"),
            OPTIONS.iter().map(|s| (*s).to_string()).collect(),
        );
        // The box tracks its own keyboard-focus flag, which the host owns
        // — the same way every `ComboBox` host sets it.
        combo.focused = true;
        Self {
            combo,
            applied: Vec::new(),
        }
    }
}

impl AppLogic for ComboApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: BOX,
            widget: Box::new(self.combo.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.combo.id, BOX, Box::new(self.combo.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::Form(id, form) = event else {
            return Reaction::Continue;
        };
        if id != self.combo.id {
            return Reaction::Continue;
        }
        match form {
            // `ComboBoxTextChanged`'s apply contract: set the live buffer
            // and open the dropdown.
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
            }
            // `ComboBoxCancelled`'s apply contract: revert to the last
            // committed value and close.
            FormEvent::ComboBoxCancelled { .. } => {
                self.applied.push("cancelled".to_string());
                self.combo.text = self.combo.committed.clone();
                self.combo.open = false;
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<ComboApp> {
    let mut driver = TuiDriver::new(ComboApp::new(), COLS, ROWS);
    // Two deliberate single clicks on the field back to back (close it,
    // reopen it) would otherwise fold into one double-click.
    driver.set_double_click_folding(false);
    driver
}

/// Click the field so the widget-contract click-to-focus step runs —
/// nothing routes a key to the box until then. The same click also drops
/// the list open, which is the field's own affordance.
fn open(driver: &mut TuiDriver<ComboApp>) {
    driver.click(BOX.x + 1.0, BOX.y);
    assert!(driver.app().combo.open, "a field click opens the list");
}

fn alt_down(driver: &mut TuiDriver<ComboApp>) -> Reaction {
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Down),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        repeat: false,
    })
}

#[test]
fn a_closed_surface_widget_box_paints_only_its_field() {
    let driver = driver();
    for option in OPTIONS {
        assert!(
            !driver.screen_contains(option),
            "{option} leaked from a closed list:\n{}",
            driver.screen()
        );
    }
    // The field's own box-drawing frame is on screen.
    let rows: Vec<Vec<char>> = driver
        .screen()
        .lines()
        .map(|l| l.chars().collect())
        .collect();
    assert_eq!(
        rows[BOX.y as usize][BOX.x as usize],
        '┌',
        "{}",
        driver.screen()
    );
}

#[test]
fn opening_the_list_paints_every_option_beneath_the_field() {
    let mut driver = driver();
    open(&mut driver);
    for option in OPTIONS {
        assert!(
            driver.screen_contains(option),
            "{option} missing:\n{}",
            driver.screen()
        );
    }
    // The list drops below the field row.
    let apple = driver.find_bounds("Apple").expect("first option visible");
    assert!(apple.y > BOX.y, "{}", driver.screen());
}

#[test]
fn alt_down_opens_the_list_without_a_pointer() {
    let mut driver = driver();
    // Focus first, so the engine has a key target.
    driver.click(BOX.x + 1.0, BOX.y);
    driver.app_mut().combo.open = false;
    driver.render();

    alt_down(&mut driver);
    assert!(driver.app().combo.open);
    assert!(driver.screen_contains("Banana"), "{}", driver.screen());
}

#[test]
fn arrow_keys_move_the_highlight_through_the_real_routing_engine() {
    let mut driver = driver();
    open(&mut driver);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(0));
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(1));
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().combo.highlighted, Some(0));
}

#[test]
fn typing_filters_the_list_and_typeahead_follows_the_highlight() {
    let mut driver = driver();
    open(&mut driver);

    driver.type_char('a');
    driver.type_char('p');
    assert_eq!(driver.app().combo.text, "ap");
    // "ap" matches Apple and Apricot, and drops Banana off the list.
    assert!(driver.screen_contains("Apricot"), "{}", driver.screen());
    assert!(!driver.screen_contains("Banana"), "{}", driver.screen());
    // Typeahead's buffered "ap" prefix-matches Apple, filtered index 0.
    assert_eq!(driver.app().combo.highlighted, Some(0));

    driver.type_char('r');
    // "apr" leaves only Apricot, now filtered index 0.
    assert!(!driver.screen_contains("Apple"), "{}", driver.screen());
    assert_eq!(driver.app().combo.highlighted, Some(0));
}

#[test]
fn enter_commits_the_highlighted_option_and_closes_the_list() {
    let mut driver = driver();
    open(&mut driver);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().combo.highlighted, Some(1));

    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().combo.committed, "Apricot");
    assert!(!driver.app().combo.open);
    assert!(!driver.screen_contains("Banana"), "{}", driver.screen());
    assert!(driver.screen_contains("Apricot"), "{}", driver.screen());
}

#[test]
fn clicking_an_option_chooses_it() {
    let mut driver = driver();
    open(&mut driver);

    let (x, y) = driver.find("Banana").expect("third option visible");
    driver.click(x, y);
    assert_eq!(driver.app().combo.committed, "Banana");
    assert!(!driver.app().combo.open);
}

#[test]
fn escape_cancels_an_open_edit_and_reverts_to_the_committed_value() {
    let mut driver = driver();
    open(&mut driver);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().combo.committed, "Apple");

    open(&mut driver);
    driver.type_char('z');
    assert_eq!(driver.app().combo.text, "Applez");

    driver.press_named(NamedKey::Escape);
    assert_eq!(driver.app().combo.text, "Apple", "reverted to committed");
    assert!(!driver.app().combo.open);
    assert_eq!(driver.app().applied.last().unwrap(), "cancelled");
}
