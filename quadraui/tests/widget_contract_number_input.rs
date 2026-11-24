#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `NumberInput` on the widget contract:
//! the field reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned
//! `NumberInput`, which the next `render` paints.
//!
//! Never a direct call into `NumberInput::handle_key`/`Primitive::layout`
//! that would skip the routing engine. The one arm the engine cannot
//! deliver is the wheel: `route_widget_event`'s `UiEvent` match has no
//! `Scroll` arm, so no `RoutedEvent::Wheel` reaches a widget through it.
//! `NumberInput` answers one anyway, covered by
//! `primitives::number_input::interactive_tests`' direct dispatch.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FormEvent, FrameTree, NamedKey, NumberInput, Reaction, Rect, ScreenLayout,
    Surface, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 10;
/// Four rows tall, so each stepper cell is a whole two-row button.
const FIELD: Rect = Rect::new(2.0, 2.0, 12.0, 4.0);

/// One number field, drawn and routed as a `Surface::Widget`, plus a
/// record of every event the host was told to apply.
struct NumberApp {
    input: NumberInput,
    applied: Vec<String>,
}

impl NumberApp {
    fn new() -> Self {
        let mut input = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0);
        input.value = 42.0;
        // The field tracks its own keyboard-focus flag, which the host
        // owns — the same way every `NumberInput` host (GTK's and Win's
        // own driver tests, `FormController`) sets it.
        input.focused = true;
        Self {
            input,
            applied: Vec::new(),
        }
    }
}

impl AppLogic for NumberApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: FIELD,
            widget: Box::new(self.input.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.input.id, FIELD, Box::new(self.input.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::Form(id, form) = event else {
            return Reaction::Continue;
        };
        if id != self.input.id {
            return Reaction::Continue;
        }
        match form {
            FormEvent::NumberChanged { value, .. } => {
                self.input.value = value;
                self.applied.push(format!("changed({value})"));
            }
            FormEvent::NumberDraftChanged { draft, .. } => {
                self.applied.push(format!("draft({draft})"));
                self.input.draft = Some(draft);
            }
            FormEvent::NumberCommitted { value, .. } => {
                self.input.value = value;
                self.input.draft = None;
                self.applied.push(format!("committed({value})"));
            }
            FormEvent::NumberCancelled { .. } => {
                self.input.draft = None;
                self.applied.push("cancelled".to_string());
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<NumberApp> {
    let mut driver = TuiDriver::new(NumberApp::new(), COLS, ROWS);
    // Two deliberate single clicks on the same stepper column back to
    // back would otherwise fold into one double-click.
    driver.set_double_click_folding(false);
    driver
}

/// The two stepper cells' centres, in screen coordinates — derived from
/// the field's own layout rather than hardcoded.
fn stepper_centres() -> ((f32, f32), (f32, f32)) {
    let layout = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0).layout(FIELD);
    let inc = layout.increment;
    let dec = layout.decrement;
    (
        (inc.x + inc.width / 2.0, inc.y + inc.height / 2.0),
        (dec.x + dec.width / 2.0, dec.y + dec.height / 2.0),
    )
}

/// Click the text area so the widget-contract click-to-focus step runs —
/// nothing routes a key to the field until then.
fn focus(driver: &mut TuiDriver<NumberApp>) {
    let (x, y) = driver.find("42").expect("the value is painted");
    driver.click(x, y);
    assert!(
        driver.app().applied.is_empty(),
        "a text-area click only focuses"
    );
}

#[test]
fn a_surface_widget_field_paints_its_value_border_and_stepper_glyphs() {
    let driver = driver();
    assert!(driver.screen_contains("42"), "{}", driver.screen());

    let rows: Vec<Vec<char>> = driver
        .screen()
        .lines()
        .map(|l| l.chars().collect())
        .collect();
    // The box-drawing frame surrounds the whole control: the bottom-left
    // corner is the one edge the value text never paints over.
    let bottom = (FIELD.y + FIELD.height) as usize - 1;
    assert_eq!(rows[bottom][FIELD.x as usize], '└', "{}", driver.screen());

    // The stepper glyphs sit on the first row of their own cells, on the
    // right of the control.
    let layout = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0).layout(FIELD);
    for (rect, glyph) in [(layout.increment, '+'), (layout.decrement, '-')] {
        assert_eq!(
            rows[rect.y as usize][rect.x as usize],
            glyph,
            "{}",
            driver.screen()
        );
    }
}

#[test]
fn clicking_the_stepper_cells_steps_the_value_through_the_real_routing_engine() {
    let mut driver = driver();
    let ((ix, iy), (dx, dy)) = stepper_centres();

    driver.click(ix, iy);
    assert_eq!(driver.app().input.value, 43.0);
    assert!(driver.screen_contains("43"), "{}", driver.screen());

    driver.click(dx, dy);
    assert_eq!(driver.app().input.value, 42.0);
    assert_eq!(
        driver.app().applied,
        vec!["changed(43)".to_string(), "changed(42)".to_string()]
    );
}

#[test]
fn arrow_keys_step_the_value_and_clamp_at_the_range_edges() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().input.value, 43.0);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().input.value, 42.0);

    driver.app_mut().input.value = 100.0;
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().input.value, 100.0, "clamped at max");

    driver.app_mut().input.value = 0.0;
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().input.value, 0.0, "clamped at min");
}

#[test]
fn typed_digits_edit_a_draft_which_enter_commits_clamped_to_the_range() {
    let mut driver = driver();
    focus(&mut driver);

    driver.type_char('9');
    driver.type_char('9');
    assert_eq!(driver.app().input.draft.as_deref(), Some("99"));
    // The in-progress draft is what the field paints, not the committed
    // value it is replacing.
    assert!(driver.screen_contains("99"), "{}", driver.screen());

    driver.type_char('9');
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().input.value, 100.0, "999 clamps to max");
    assert_eq!(driver.app().input.draft, None);
    assert!(driver.screen_contains("100"), "{}", driver.screen());
}

#[test]
fn escape_cancels_a_pending_draft_and_leaves_the_committed_value_alone() {
    let mut driver = driver();
    focus(&mut driver);

    driver.type_char('7');
    assert_eq!(driver.app().input.draft.as_deref(), Some("7"));

    driver.press_named(NamedKey::Escape);
    assert_eq!(driver.app().input.draft, None);
    assert_eq!(driver.app().input.value, 42.0);
    assert!(driver.screen_contains("42"), "{}", driver.screen());
}

#[test]
fn a_focused_field_paints_its_border_in_the_accent_colour() {
    let driver = driver();
    let style = driver
        .style_at(FIELD.x as u16, (FIELD.y + FIELD.height) as u16 - 1)
        .expect("the frame's bottom-left corner");
    assert_eq!(
        style.fg,
        quadraui::tui::ratatui_color(Theme::default().accent_fg)
    );
}
