//! Backend-agnostic Form demo showcasing `FieldKind::ToggleGroup`,
//! `FieldKind::ButtonRow`, and [`FocusRing`] for Tab/Shift+Tab cycling.
//!
//! Single [`AppLogic`] impl drives both the TUI runner and the GTK
//! runner. The thin shells in `examples/{tui,gtk}_form_groups.rs` are
//! each ~10 lines.
//!
//! Shape: a mini search/replace panel with:
//! - TextInput: search query
//! - ToggleGroup: Aa (case) / Ab| (word) / .* (regex)
//! - TextInput: replace string
//! - ButtonRow: Find Next / Replace / Replace All
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - mouse click on button  → log to status bar
//! - `Tab` / `Shift+Tab`    → cycle focused field (via FocusRing)
//! - `q` / `Esc`            → quit

use quadraui::{
    AppLogic, Backend, ButtonRowItem, Color, FieldKind, FocusRing, Form, FormField, FormHit, Key,
    MouseButton, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText,
    ToggleGroupItem, UiEvent, WidgetId,
};

pub struct FormGroupsApp {
    search_query: String,
    replace_query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    focus: FocusRing,
    last_action: String,
}

impl FormGroupsApp {
    pub fn new() -> Self {
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
            id: WidgetId::new("find-replace"),
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
                            },
                            ButtonRowItem {
                                id: WidgetId::new("replace-one"),
                                label: "Replace".into(),
                                disabled: false,
                            },
                            ButtonRowItem {
                                id: WidgetId::new("replace-all"),
                                label: "Replace All".into(),
                                disabled: self.search_query.is_empty(),
                            },
                        ],
                    },
                    hint: StyledText::default(),
                    disabled: false,
                },
            ],
            focused_field: self.focus.current().cloned(),
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn form_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, 0.0, vp.width, (vp.height - status_h).max(0.0))
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, (vp.height - status_h).max(0.0), vp.width, status_h)
    }

    fn click(&mut self, backend: &mut dyn Backend, x: f32, y: f32) {
        let form = self.build_form();
        let area = Self::form_rect(backend);
        let layout = backend.form_layout(area, &form);
        let hit = layout.hit_test(x, y);
        match hit {
            FormHit::Field(ref id) => {
                if id.as_str() == "case" {
                    self.case_sensitive = !self.case_sensitive;
                    self.last_action = format!("case={}", self.case_sensitive);
                } else if id.as_str() == "word" {
                    self.whole_word = !self.whole_word;
                    self.last_action = format!("word={}", self.whole_word);
                } else if id.as_str() == "regex" {
                    self.regex = !self.regex;
                    self.last_action = format!("regex={}", self.regex);
                } else if id.as_str() == "find-next" {
                    self.last_action = "Find Next".into();
                } else if id.as_str() == "replace-one" {
                    self.last_action = "Replace".into();
                } else if id.as_str() == "replace-all" {
                    if !self.search_query.is_empty() {
                        self.last_action = "Replace All".into();
                    }
                } else {
                    self.focus.set(id);
                    self.last_action = format!("focus → {}", id.as_str());
                }
            }
            FormHit::Empty => {}
        }
    }

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" last: {} ", self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " click / Tab / Shift+Tab / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormGroupsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormGroupsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let form_rect = Self::form_rect(backend);
        let status_rect = Self::status_rect(backend);
        backend.draw_form(form_rect, &self.build_form());
        let _hits = backend.draw_status_bar(status_rect, &self.build_status_bar());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                self.click(backend, position.x, position.y);
                Reaction::Redraw
            }
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Named(NamedKey::Tab) => {
                    self.focus.advance();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::BackTab) => {
                    self.focus.retreat();
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
