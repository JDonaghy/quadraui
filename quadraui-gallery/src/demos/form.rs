//! `Form` demo — adapted from `quadraui/examples/common/form_all_fields.rs`,
//! `form_groups.rs` and `form_scroll.rs`.
//!
//! Three variants cover the `Form` primitive's range: every `FieldKind`
//! laid out statically, a small interactive search/replace panel
//! (toggle group, button row, an embedded toolbar field, and
//! [`FocusRing`]-driven Tab/Shift+Tab cycling), and a 20-field settings
//! panel long enough to need [`FormController`]'s built-in scrollbar.

use quadraui::{
    Backend, BackendCaps, ButtonRowItem, Color, FieldKind, FocusRing, Form, FormController,
    FormControllerEvent, FormEvent, FormField, FormHit, InteractionState, Key, MouseButton,
    NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, ToggleGroupItem, Toolbar,
    ToolbarButton, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("form.rs");

// gallery:begin
/// Variant 1 ("Search & replace") state: a mini find/replace panel.
struct SearchReplaceState {
    search_query: String,
    replace_query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    focus: FocusRing,
    last_action: String,
}

impl SearchReplaceState {
    fn new() -> Self {
        Self {
            search_query: "hello".into(),
            replace_query: String::new(),
            case_sensitive: true,
            whole_word: false,
            regex: false,
            focus: FocusRing::new(vec!["search", "toggles", "replace", "buttons"]),
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        Form {
            id: WidgetId::new("gallery:form:search-replace"),
            fields: vec![
                FormField {
                    id: WidgetId::new("search"),
                    label: StyledText::plain("Find"),
                    kind: FieldKind::TextInput {
                        value: self.search_query.clone(),
                        placeholder: "Search…".into(),
                        cursor: Some(self.search_query.len()),
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("toggles"),
                    label: StyledText::default(),
                    kind: FieldKind::ToggleGroup {
                        toggles: vec![
                            ToggleGroupItem {
                                id: WidgetId::new("case"),
                                label: "Aa".into(),
                                value: self.case_sensitive,
                            },
                            ToggleGroupItem {
                                id: WidgetId::new("word"),
                                label: "Ab|".into(),
                                value: self.whole_word,
                            },
                            ToggleGroupItem {
                                id: WidgetId::new("regex"),
                                label: ".*".into(),
                                value: self.regex,
                            },
                        ],
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("replace"),
                    label: StyledText::plain("Replace"),
                    kind: FieldKind::TextInput {
                        value: self.replace_query.clone(),
                        placeholder: "Replace…".into(),
                        cursor: Some(self.replace_query.len()),
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("buttons"),
                    label: StyledText::default(),
                    kind: FieldKind::ButtonRow {
                        buttons: vec![
                            ButtonRowItem {
                                id: WidgetId::new("find-next"),
                                label: "Find Next".into(),
                                disabled: false,
                                icon: None,
                            },
                            ButtonRowItem {
                                id: WidgetId::new("replace-one"),
                                label: "Replace".into(),
                                disabled: false,
                                icon: None,
                            },
                            ButtonRowItem {
                                id: WidgetId::new("replace-all"),
                                label: "Replace All".into(),
                                disabled: self.search_query.is_empty(),
                                icon: None,
                            },
                        ],
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
            ],
            focused_field: self.focus.current().cloned(),
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn click(&mut self, id: &WidgetId) {
        match id.as_str() {
            "case" => {
                self.case_sensitive = !self.case_sensitive;
                self.last_action = format!("case={}", self.case_sensitive);
            }
            "word" => {
                self.whole_word = !self.whole_word;
                self.last_action = format!("word={}", self.whole_word);
            }
            "regex" => {
                self.regex = !self.regex;
                self.last_action = format!("regex={}", self.regex);
            }
            "find-next" => self.last_action = "Find Next".into(),
            "replace-one" => self.last_action = "Replace".into(),
            "replace-all" => {
                if !self.search_query.is_empty() {
                    self.last_action = "Replace All".into();
                }
            }
            other => {
                self.focus.set(id);
                self.last_action = format!("focus → {other}");
            }
        }
    }
}

/// Variant 2 ("Scrolling") state: 20 toggles, enough to overflow a
/// typical gallery viewport and exercise `FormController`'s scrollbar.
struct ScrollState {
    fc: FormController,
    toggles: Vec<bool>,
    last_action: String,
}

fn scroll_form_from_toggles(toggles: &[bool]) -> Form {
    let fields: Vec<FormField> = toggles
        .iter()
        .enumerate()
        .map(|(i, &val)| FormField {
            id: WidgetId::new(format!("toggle-{i}")),
            label: StyledText::plain(format!("Setting {}", i + 1)),
            kind: FieldKind::Toggle { value: val },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        })
        .collect();
    Form {
        id: WidgetId::new("gallery:form:settings-fields"),
        fields,
        focused_field: None,
        scroll_offset: 0,
        has_focus: true,
    }
}

impl ScrollState {
    fn new() -> Self {
        let toggles = vec![false; 20];
        let mut fc = FormController::new("gallery:form:settings".to_string());
        // Seed the form immediately so the very first render (before any
        // `handle()` event reaches `FormController`) isn't blank —
        // `FormController::render` reads back whatever `set_form` last
        // stored, it never builds from scratch.
        fc.set_form(scroll_form_from_toggles(&toggles));
        Self {
            fc,
            toggles,
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        scroll_form_from_toggles(&self.toggles)
    }
}

/// All `FieldKind` variants, laid out statically — variant 0 ("All field
/// kinds"). No interactive state: this is a reference sheet, not a live
/// control surface.
fn build_all_fields_form() -> Form {
    Form {
        id: WidgetId::new("gallery:form:all-fields"),
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
                    value: StyledText::plain("v0.1.2"),
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            },
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
                        StyledText::plain("Solarized Light"),
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
                    value: "Release notes go here.".into(),
                    placeholder: String::new(),
                    cursor: None,
                    visible_rows: 3,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            },
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
                    options: vec!["File".into(), "Project".into()],
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
                        label: "Case sensitive".into(),
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
                        label: "Run action".into(),
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
                kind: FieldKind::Toolbar(
                    Toolbar::new(WidgetId::new("toolbar-field")).with_buttons(vec![
                        ToolbarButton::Action {
                            id: WidgetId::new("build"),
                            label: "Build".into(),
                            icon: None,
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: "Build the project".into(),
                        },
                    ]),
                ),
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

pub struct FormDemo {
    search_replace: SearchReplaceState,
    scroll: ScrollState,
}

impl FormDemo {
    pub fn new() -> Self {
        Self {
            search_replace: SearchReplaceState::new(),
            scroll: ScrollState::new(),
        }
    }

    fn status_bar(text: &str, hint: &str) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("gallery:form:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {text} "),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {hint} "),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }

    fn form_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for FormDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FormDemo {
    fn name(&self) -> &'static str {
        "Form"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["All field kinds", "Search & replace", "Scrolling"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let form_rect = Self::form_rect(area, backend);
        match variant {
            0 => backend.draw_form(area, &build_all_fields_form()),
            1 => {
                backend.draw_form(form_rect, &self.search_replace.build_form());
                let bar = Self::status_bar(
                    &format!("last: {}", self.search_replace.last_action),
                    "click / Tab / Shift+Tab",
                );
                let _ = backend.draw_status_bar_interactive(
                    Self::status_rect(area, backend),
                    &bar,
                    &InteractionState::new(),
                );
            }
            _ => {
                self.scroll.fc.render(backend, form_rect);
                let bar = Self::status_bar(
                    &format!(
                        "scroll={} | {}",
                        self.scroll.fc.scroll_offset(),
                        self.scroll.last_action
                    ),
                    "scroll / click / drag scrollbar",
                );
                let _ = backend.draw_status_bar_interactive(
                    Self::status_rect(area, backend),
                    &bar,
                    &InteractionState::new(),
                );
            }
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match variant {
            0 => Reaction::Continue,
            1 => match event {
                UiEvent::MouseDown {
                    button: MouseButton::Left,
                    position,
                    ..
                } => {
                    let form_rect = Self::form_rect(area, backend);
                    let layout = backend.form_layout(form_rect, &self.search_replace.build_form());
                    match layout.hit_test(position.x, position.y) {
                        FormHit::Field(id) => {
                            self.search_replace.click(&id);
                            Reaction::Redraw
                        }
                        FormHit::Empty => Reaction::Continue,
                    }
                }
                UiEvent::KeyPressed { key, .. } => match key {
                    Key::Named(NamedKey::Tab) => {
                        self.search_replace.focus.advance();
                        Reaction::Redraw
                    }
                    Key::Named(NamedKey::BackTab) => {
                        self.search_replace.focus.retreat();
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                },
                _ => Reaction::Continue,
            },
            _ => {
                self.scroll.fc.set_form(self.scroll.build_form());
                self.scroll.fc.set_backend_info(backend.line_height());
                let rect = Self::form_rect(area, backend);
                match self.scroll.fc.handle_cached(event, rect) {
                    FormControllerEvent::FormAction(FormEvent::ToggleChanged { id, value }) => {
                        if let Ok(idx) = id
                            .as_str()
                            .strip_prefix("toggle-")
                            .unwrap_or("")
                            .parse::<usize>()
                        {
                            if idx < self.scroll.toggles.len() {
                                self.scroll.toggles[idx] = value;
                            }
                        }
                        self.scroll.last_action = format!("{} = {value}", id.as_str());
                        Reaction::Redraw
                    }
                    FormControllerEvent::FormAction(other) => {
                        self.scroll.last_action = format!("{other:?}");
                        Reaction::Redraw
                    }
                    FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => {
                        Reaction::Redraw
                    }
                    FormControllerEvent::Ignored => Reaction::Continue,
                }
            }
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            0 => serde_json::to_value(build_all_fields_form()).unwrap_or(serde_json::Value::Null),
            1 => serde_json::json!({
                "form": self.search_replace.build_form(),
                "last_action": self.search_replace.last_action,
            }),
            _ => serde_json::json!({
                "scroll_offset": self.scroll.fc.scroll_offset(),
                "toggles_on": self.scroll.toggles.iter().filter(|&&v| v).count(),
                "last_action": self.scroll.last_action,
            }),
        }
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_replace_click_toggles_case_sensitivity() {
        let mut state = SearchReplaceState::new();
        assert!(state.case_sensitive);
        state.click(&WidgetId::new("case"));
        assert!(!state.case_sensitive);
    }

    #[test]
    fn search_replace_click_on_a_field_id_moves_focus() {
        let mut state = SearchReplaceState::new();
        state.click(&WidgetId::new("replace"));
        assert_eq!(state.focus.current(), Some(&WidgetId::new("replace")));
    }

    #[test]
    fn scroll_state_starts_with_twenty_untoggled_settings() {
        let state = ScrollState::new();
        assert_eq!(state.toggles.len(), 20);
        assert!(state.toggles.iter().all(|&v| !v));
    }

    #[test]
    fn all_fields_form_has_every_field_kind_represented() {
        let form = build_all_fields_form();
        assert_eq!(form.fields.len(), 14);
    }
}
