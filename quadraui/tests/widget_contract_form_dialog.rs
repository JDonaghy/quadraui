#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `FormDialog` on the widget contract: a `Dialog`
//! whose body is a `Form`, pushed as a `Surface::Widget` and registered
//! in the app's `FrameTree`. The chrome and buttons paint through
//! `Dialog`'s own painters and the fields through `Form`'s; typing and
//! clicks on the form reach `Form`'s own `Interactive`, and the one focus
//! order across fields and buttons is the dialog's. Field events arrive
//! as `UiEvent::Form` under the dialog's id and the app applies them; a
//! button arrives as `UiEvent::MessageDialogResolved`. Driven on the TUI
//! backend and on the macOS pixel backend.

use quadraui::{
    AppLogic, Backend, DialogButton, FieldKind, Form, FormDialog, FormEvent, FormField, FrameTree,
    Reaction, Rect, ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};

const TITLE: &str = "New milestone";

fn dialog_id() -> WidgetId {
    WidgetId::new("milestone")
}

fn create_id() -> WidgetId {
    WidgetId::new("milestone:create")
}

fn cancel_id() -> WidgetId {
    WidgetId::new("milestone:cancel")
}

fn name_id() -> WidgetId {
    WidgetId::new("milestone:name")
}

fn due_id() -> WidgetId {
    WidgetId::new("milestone:due")
}

fn text_field(id: WidgetId, label: &str, placeholder: &str) -> FormField {
    FormField {
        id,
        label: StyledText::plain(label),
        kind: FieldKind::TextInput {
            value: String::new(),
            placeholder: placeholder.into(),
            cursor: None,
            selection_anchor: None,
        },
        hint: StyledText::default(),
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

fn dialog() -> FormDialog {
    let mut d = FormDialog::new(dialog_id(), TITLE);
    d.buttons = vec![
        DialogButton {
            id: cancel_id(),
            label: "Cancel".into(),
            is_default: false,
            is_cancel: true,
            tint: None,
            spec: None,
        },
        DialogButton {
            id: create_id(),
            label: "Create".into(),
            is_default: true,
            is_cancel: false,
            tint: None,
            spec: None,
        },
    ];
    d.form = Some(Form {
        id: WidgetId::new("milestone:form"),
        fields: vec![
            text_field(name_id(), "Name", "Milestone name"),
            text_field(due_id(), "Due", "YYYY-MM-DD"),
        ],
        focused_field: Some(name_id()),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });
    d
}

/// One form dialog routed through the engine; the app owns the form's
/// values and focus and records what the buttons resolved to.
struct DialogApp {
    dialog: FormDialog,
    area: Rect,
    resolved: Vec<WidgetId>,
}

impl DialogApp {
    fn new(area: Rect) -> Self {
        Self {
            dialog: dialog(),
            area,
            resolved: Vec::new(),
        }
    }

    fn value(&self, id: WidgetId) -> String {
        let form = self.dialog.form.as_ref().expect("form");
        match &form.fields.iter().find(|f| f.id == id).expect("field").kind {
            FieldKind::TextInput { value, .. } => value.clone(),
            other => panic!("not a text field: {other:?}"),
        }
    }

    fn focused(&self) -> Option<WidgetId> {
        self.dialog.form.as_ref().and_then(|f| f.focused_field)
    }
}

impl AppLogic for DialogApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.area,
            widget: Box::new(self.dialog.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.dialog.id, self.area, Box::new(self.dialog.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MessageDialogResolved(id) => self.resolved.push(id),
            UiEvent::Form(id, form_event) if id == dialog_id() => {
                let form = self.dialog.form.as_mut().expect("form");
                match form_event {
                    FormEvent::FocusChanged { id } => form.focused_field = Some(id),
                    FormEvent::TextInputChanged { id, value } => {
                        if let Some(FieldKind::TextInput { value: v, .. }) = form
                            .fields
                            .iter_mut()
                            .find(|f| f.id == id)
                            .map(|f| &mut f.kind)
                        {
                            *v = value;
                        }
                    }
                    _ => return Reaction::Continue,
                }
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const COLS: u16 = 70;
    const ROWS: u16 = 20;
    const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

    fn driver() -> TuiDriver<DialogApp> {
        TuiDriver::new(DialogApp::new(AREA), COLS, ROWS)
    }

    /// Click the Name field: focuses the dialog on the form.
    fn click_name(driver: &mut TuiDriver<DialogApp>) {
        let (x, y) = driver.find("Name").expect("Name label on screen");
        driver.click(x, y);
    }

    #[test]
    fn paints_the_dialog_chrome_the_form_fields_and_the_buttons() {
        let driver = driver();
        for text in [TITLE, "Name", "Due", "Milestone name", "Cancel", "Create"] {
            assert!(driver.screen_contains(text), "{text}: {}", driver.screen());
        }
    }

    #[test]
    fn typing_into_the_focused_field_reaches_the_app_as_a_form_event() {
        let mut driver = driver();
        click_name(&mut driver);
        for c in "v1".chars() {
            driver.type_char(c);
        }
        assert_eq!(driver.app().value(name_id()), "v1");
        assert!(driver.screen_contains("v1"), "{}", driver.screen());
    }

    #[test]
    fn tab_walks_the_fields_then_the_buttons_as_one_order() {
        let mut driver = driver();
        click_name(&mut driver);
        driver.press_named(NamedKey::Tab);
        assert_eq!(driver.app().focused(), Some(due_id()));

        // Past the last field: the first button, which Enter activates.
        driver.press_named(NamedKey::Tab);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);

        // Past the last button: back to the first field.
        driver.press_named(NamedKey::Tab);
        driver.press_named(NamedKey::Tab);
        assert_eq!(driver.app().focused(), Some(name_id()));
        driver.type_char('x');
        assert_eq!(driver.app().value(name_id()), "x");
    }

    #[test]
    fn clicking_a_button_resolves_it_and_escape_resolves_cancel() {
        let mut driver = driver();
        let b = driver.find_bounds("Create").expect("Create on screen");
        driver.click(b.x + 0.5, b.y);
        assert_eq!(driver.app().resolved, vec![create_id()]);

        click_name(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().resolved, vec![create_id(), cancel_id()]);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    const W: u32 = 720;
    const H: u32 = 480;
    const AREA: Rect = Rect::new(0.0, 0.0, W as f32, H as f32);

    #[test]
    fn paints_the_form_and_types_into_the_clicked_field() {
        let mut driver = MacDriver::new(DialogApp::new(AREA), W, H);
        for text in [TITLE, "Name", "Due", "Cancel", "Create"] {
            assert!(
                driver.screen_contains(text),
                "{text}: {:?}",
                driver.painted_texts()
            );
        }
        let (x, y) = driver.find("Milestone name").expect("placeholder painted");
        driver.click(x, y);
        driver.type_char('q');
        assert_eq!(driver.app().value(name_id()), "q");

        let (x, y) = driver.find("Create").expect("Create painted");
        driver.click(x, y);
        assert_eq!(driver.app().resolved, vec![create_id()]);
    }
}
