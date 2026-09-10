//! Backend-agnostic [`Form`] demo covering all 14 [`FieldKind`] variants
//! (quadraui#808).
//!
//! Before #808, `macos::form::draw_form` matched only 10 of 14
//! `FieldKind` variants and silently fell through (`_ => {}`) on
//! `Slider` / `ColorPicker` / `Dropdown` / `TextArea` — a form using one
//! of those rendered nothing on macOS, with no error. `gtk::form::
//! draw_form` separately left `Slider` / `ColorPicker` / `Dropdown`
//! blank. This fixture exists to give `tests/cross_backend_parity.rs` a
//! single `Form` exercising every variant, so a regression that drops
//! one again fails a shared assertion instead of shipping silently on
//! whichever backend nobody happened to check.
//!
//! Static display only — no field in this form needs interactive state
//! for the parity check, so `handle` only wires up quit.

use quadraui::{
    AppLogic, Backend, ButtonRowItem, Color, FieldKind, Form, FormField, Key, NamedKey, Reaction,
    Rect, StyledText, ToggleGroupItem, Toolbar, ToolbarButton, UiEvent, WidgetId,
};

pub struct FormAllFieldsApp;

impl FormAllFieldsApp {
    pub fn new() -> Self {
        Self
    }

    fn build_form() -> Form {
        Form {
            id: WidgetId::new("all-fields"),
            fields: vec![
                FormField {
                    id: WidgetId::new("hdr"),
                    label: StyledText::plain("Editor"),
                    kind: FieldKind::Label,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("line-numbers"),
                    label: StyledText::plain("Show line numbers"),
                    kind: FieldKind::Toggle { value: true },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("name"),
                    label: StyledText::plain("Name"),
                    kind: FieldKind::TextInput {
                        value: "quadraui".into(),
                        placeholder: String::new(),
                        cursor: Some(4),
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("save"),
                    label: StyledText::plain("Save settings"),
                    kind: FieldKind::Button,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("version"),
                    label: StyledText::plain("Version"),
                    kind: FieldKind::ReadOnly {
                        value: StyledText::plain("v0zerodotonezero"),
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                // ── The four variants pre-#808 macOS silently dropped ──
                FormField {
                    id: WidgetId::new("font-size"),
                    label: StyledText::plain("Font size"),
                    kind: FieldKind::Slider {
                        value: 14.0,
                        min: 8.0,
                        max: 32.0,
                        step: 1.0,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("accent"),
                    label: StyledText::plain("Accent colour"),
                    kind: FieldKind::ColorPicker {
                        value: Color::rgb(0x7a, 0xb4, 0xff),
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("theme"),
                    label: StyledText::plain("Theme"),
                    kind: FieldKind::Dropdown {
                        options: vec![
                            StyledText::plain("One Dark"),
                            StyledText::plain("Solarizedlight"),
                        ],
                        selected_idx: 1,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("notes"),
                    label: StyledText::plain("Notes"),
                    kind: FieldKind::TextArea {
                        value: "releasenotesgohere".into(),
                        placeholder: String::new(),
                        cursor: None,
                        visible_rows: 3,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                // ── Remaining variants ──────────────────────────────────
                FormField {
                    id: WidgetId::new("password"),
                    label: StyledText::plain("Password"),
                    kind: FieldKind::PasswordInput {
                        value: "secret1".into(),
                        placeholder: String::new(),
                        cursor: None,
                        mask_char: '•',
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("scope"),
                    label: StyledText::default(),
                    kind: FieldKind::SegmentedControl {
                        options: vec!["Filescope".into(), "Projectscope".into()],
                        selected_idx: 0,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("flags"),
                    label: StyledText::plain("Flags"),
                    kind: FieldKind::ToggleGroup {
                        toggles: vec![ToggleGroupItem {
                            id: WidgetId::new("case"),
                            label: "Casesens".into(),
                            value: true,
                        }],
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("actions"),
                    label: StyledText::plain("Actions"),
                    kind: FieldKind::ButtonRow {
                        buttons: vec![ButtonRowItem {
                            id: WidgetId::new("run"),
                            label: "Runaction".into(),
                            disabled: false,
                            icon: None,
                        }],
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("toolbar"),
                    label: StyledText::plain("Toolbar"),
                    kind: FieldKind::Toolbar(Toolbar {
                        id: WidgetId::new("toolbar-field"),
                        buttons: vec![ToolbarButton::Action {
                            id: WidgetId::new("build"),
                            label: "Buildaction".into(),
                            icon: None,
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: "Build the project".into(),
                        }],
                        bg: None,
                        focused_index: None,
                        icon_overrides: Vec::new(),
                    }),
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
        }
    }
}

impl Default for FormAllFieldsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormAllFieldsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        backend.draw_form(
            Rect::new(0.0, 0.0, vp.width, vp.height),
            &Self::build_form(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
