#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Form` on the widget contract: the
//! form reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned `Form`,
//! which the next `render` paints.
//!
//! Never a direct call into `Interactive::handle`/`Primitive::layout`
//! that would skip the routing engine, and never a call into
//! `compose::FormController` — this is the contract path, not the
//! composed-controller one.

use quadraui::{
    AppLogic, Backend, FieldKind, Form, FormEvent, FormField, FrameTree, Key, Modifiers, NamedKey,
    Reaction, Rect, ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};

use quadraui::tui::testing::TuiDriver;

const COLS: u16 = 44;
const ROWS: u16 = 12;
const FORM: Rect = Rect::new(1.0, 1.0, 40.0, 8.0);

fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(label),
        kind,
        hint: StyledText::plain(""),
        disabled: false,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

/// One form, drawn and routed as a `Surface::Widget`, plus a record of
/// every event the host was told to apply.
struct FormApp {
    form: Form,
    applied: Vec<String>,
}

impl FormApp {
    fn new() -> Self {
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![
                field("hdr", "Editor", FieldKind::Label),
                field("wrap", "Wrap", FieldKind::Toggle { value: false }),
                field(
                    "font",
                    "Font",
                    FieldKind::TextInput {
                        value: "Mono".to_string(),
                        placeholder: String::new(),
                        cursor: None,
                        selection_anchor: None,
                    },
                ),
                field("apply", "Apply", FieldKind::Button),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        Self {
            form,
            applied: Vec::new(),
        }
    }

    /// `kind` of the field named `id`, for asserting on an applied value.
    fn kind(&self, id: &str) -> &FieldKind {
        let id = WidgetId::new(id);
        &self
            .form
            .fields
            .iter()
            .find(|f| f.id == id)
            .expect("field is on the form")
            .kind
    }
}

impl AppLogic for FormApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: FORM,
            widget: Box::new(self.form.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.form.id, FORM, Box::new(self.form.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::TextCopied(text) = &event {
            self.applied.push(format!("copied({text})"));
            return Reaction::Continue;
        }
        let UiEvent::Form(id, form_event) = event else {
            return Reaction::Continue;
        };
        if id != self.form.id {
            return Reaction::Continue;
        }
        match form_event {
            FormEvent::FocusChanged { id } => {
                self.applied.push(format!("focus({})", id.as_str()));
                self.form.focused_field = Some(id);
            }
            FormEvent::ToggleChanged { id, value } => {
                self.applied
                    .push(format!("toggle({}={value})", id.as_str()));
                if let Some(f) = self.form.fields.iter_mut().find(|f| f.id == id) {
                    if let FieldKind::Toggle { value: v } = &mut f.kind {
                        *v = value;
                    }
                }
            }
            FormEvent::TextInputChanged { id, ref value } => {
                self.applied.push(format!("text({}={value})", id.as_str()));
                if let Some(f) = self.form.fields.iter_mut().find(|f| f.id == id) {
                    if let FieldKind::TextInput { value: v, .. } = &mut f.kind {
                        *v = value.clone();
                    }
                }
            }
            FormEvent::ButtonClicked { id } => {
                self.applied.push(format!("click({})", id.as_str()));
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<FormApp> {
    let mut driver = TuiDriver::new(FormApp::new(), COLS, ROWS);
    // Two deliberate single clicks on the same row back to back would
    // otherwise fold into one double-click.
    driver.set_double_click_folding(false);
    driver
}

/// Shift-Tab, which `TuiDriver` has no shorthand for — delivered as the
/// `KeyPressed` a backend sends, so it goes through the same routing
/// engine every other key in this file does.
fn shift_tab(driver: &mut TuiDriver<FormApp>) {
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
}

/// Click a row so the widget-contract click-to-focus step runs —
/// nothing routes a key to the form until then. Found by its label, so
/// this never hardcodes a coordinate.
fn click_row(driver: &mut TuiDriver<FormApp>, label: &str) {
    let (x, y) = driver
        .find(label)
        .unwrap_or_else(|| panic!("{label}'s row is painted:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn a_surface_widget_form_paints_its_label_column_and_every_field_control() {
    let driver = driver();
    let screen = driver.screen();
    for label in ["Editor", "Wrap", "Font", "Apply"] {
        assert!(driver.screen_contains(label), "{label}:\n{screen}");
    }
    // The toggle's own glyph and the text field's value, not just the
    // labels — proof the per-`FieldKind` controls painted too.
    assert!(driver.screen_contains("[ ]"), "toggle glyph:\n{screen}");
    assert!(driver.screen_contains("Mono"), "text value:\n{screen}");
}

#[test]
fn clicking_a_toggle_row_flips_it_through_the_routing_engine() {
    let mut driver = driver();
    click_row(&mut driver, "Wrap");

    assert_eq!(
        driver.app().applied,
        vec!["toggle(wrap=true)".to_string()],
        "{}",
        driver.screen()
    );
    assert_eq!(
        driver.app().kind("wrap"),
        &FieldKind::Toggle { value: true }
    );
    assert!(driver.screen_contains("[x]"), "{}", driver.screen());
}

#[test]
fn clicking_a_button_row_emits_its_click_not_a_focus_change() {
    let mut driver = driver();
    click_row(&mut driver, "Apply");
    assert_eq!(
        driver.app().applied,
        vec!["click(apply)".to_string()],
        "{}",
        driver.screen()
    );
}

#[test]
fn tab_walks_the_forms_own_focus_stops_and_shift_tab_walks_back() {
    let mut driver = driver();
    // Focus the form by clicking a row that only focuses (a text field),
    // then let Tab take over from there.
    click_row(&mut driver, "Font");
    assert_eq!(driver.app().form.focused_field, Some(WidgetId::new("font")));

    driver.press_named(NamedKey::Tab);
    assert_eq!(
        driver.app().form.focused_field,
        Some(WidgetId::new("apply")),
        "Tab moves to the next focusable field, skipping the Label header"
    );

    // Wrapping round the end lands back on the first stop, not on the
    // `FieldKind::Label` header, which is not a focus stop.
    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().form.focused_field, Some(WidgetId::new("wrap")));

    shift_tab(&mut driver);
    assert_eq!(
        driver.app().form.focused_field,
        Some(WidgetId::new("apply"))
    );
}

#[test]
fn typing_into_a_focused_text_field_edits_it_and_repaints_the_value() {
    let mut driver = driver();
    click_row(&mut driver, "Font");

    driver.type_char('X');
    assert_eq!(
        driver.app().kind("font"),
        &FieldKind::TextInput {
            value: "MonoX".to_string(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
        },
        "{}",
        driver.screen()
    );
    assert!(driver.screen_contains("MonoX"), "{}", driver.screen());

    driver.press_named(NamedKey::Backspace);
    driver.press_named(NamedKey::Backspace);
    assert!(driver.screen_contains("Mon"), "{}", driver.screen());
    assert!(!driver.screen_contains("Mono"), "{}", driver.screen());
}

#[test]
fn a_space_on_a_focused_toggle_flips_it_instead_of_typing_into_it() {
    let mut driver = driver();
    click_row(&mut driver, "Font");
    shift_tab(&mut driver);
    assert_eq!(driver.app().form.focused_field, Some(WidgetId::new("wrap")));

    driver.type_char(' ');
    assert_eq!(
        driver.app().kind("wrap"),
        &FieldKind::Toggle { value: true }
    );
    assert!(driver.screen_contains("[x]"), "{}", driver.screen());
}

fn chord(driver: &mut TuiDriver<FormApp>, ch: char) {
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Char(ch),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
}

#[test]
fn ctrl_a_then_ctrl_c_copies_a_focused_text_fields_value() {
    let mut driver = driver();
    click_row(&mut driver, "Font");
    driver.app_mut().applied.clear();

    chord(&mut driver, 'a');
    chord(&mut driver, 'c');
    assert_eq!(driver.app().applied, vec!["copied(Mono)".to_string()]);
}

#[test]
fn ctrl_z_on_a_focused_text_field_does_not_type_a_z() {
    let mut driver = driver();
    click_row(&mut driver, "Font");
    driver.app_mut().applied.clear();

    chord(&mut driver, 'z');
    assert!(
        driver.app().applied.is_empty(),
        "{:?}",
        driver.app().applied
    );
    assert!(driver.screen_contains("Mono"), "{}", driver.screen());
    assert!(!driver.screen_contains("Monoz"), "{}", driver.screen());
}
