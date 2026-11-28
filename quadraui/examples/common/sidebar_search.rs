//! `Sidebar`-based search panel — manual smoke test for Form
//! sections with ToggleGroup/ButtonRow items and
//! Header-decorated tree rows.
//!
//! Structure:
//! - Section 0 (Form): query TextInput + toggle flags (Aa, .*, W)
//! - Section 1 (Tree): search results grouped by file
//!   (`Decoration::Header` file rows + `Normal` match rows)
//!
//! Controls:
//! - Click toggles to flip (status bar shows the event)
//! - Click file headers to collapse/expand
//! - Click match rows to "jump"
//! - Tab cycles sections
//! - Arrow keys / Home / End move cursor within text fields
//! - Shift+Arrow / Shift+Home / Shift+End extend selection
//! - Ctrl+A select all, Ctrl+C copy (not password), Ctrl+V paste
//! - q / Esc to quit (when not typing)

use quadraui::behaviour::text_edit::flat_selected_text;
use quadraui::primitives::form::{FieldKind, FormField, ToggleGroupItem, ValidationState};
use quadraui::{
    AppLogic, Backend, Color, Decoration, Form, FormEvent, Handled, Key, Modifiers, NamedKey,
    NavigationMode, Reaction, Rect, Sidebar, SidebarAction, SidebarSectionDef, SidebarState,
    StatusBar, StatusBarSegment, StyledText, TreeRow, UiEvent, WidgetId,
};
use quadraui::{DrawsPrimitives, EditOp, TextField};

const STATUS_BAR_LINES: f32 = 1.5;

// ─── App ─────────────────────────��────────────────────────��────────────────

pub struct SidebarSearchApp {
    sidebar: Sidebar,
    sidebar_state: SidebarState,
    case_sensitive: bool,
    regex: bool,
    whole_word: bool,
    query: TextField,
    password: TextField,
    scope_idx: usize,
    focused_field: Option<String>,
    expanded: Vec<bool>,
    last_event: String,
}

impl SidebarSearchApp {
    pub fn new() -> Self {
        let mut sidebar = Sidebar::from_defs(
            WidgetId::new("search-sidebar"),
            &[
                SidebarSectionDef::form("search", "SEARCH"),
                SidebarSectionDef::new("results", "RESULTS"),
            ],
        );
        sidebar.navigation_mode = NavigationMode::Selection;
        sidebar.active_section = Some(0);

        let expanded = vec![true; 3];
        let mut app = Self {
            sidebar,
            sidebar_state: SidebarState::default(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            query: TextField::new(WidgetId::new("query")).with_cursor(Some(0)),
            password: TextField::new(WidgetId::new("password")).with_cursor(Some(0)),
            scope_idx: 0,
            focused_field: Some("query".into()),
            expanded,
            last_event: "Click toggles, headers, or match rows".into(),
        };
        app.update_form();
        app.update_results();
        app
    }

    fn update_form(&mut self) {
        let form = Form {
            id: WidgetId::new("search-form"),
            fields: vec![
                FormField {
                    id: WidgetId::new("query"),
                    label: StyledText::plain("Find"),
                    kind: FieldKind::TextInput {
                        value: self.query.value.clone(),
                        placeholder: "Search...".into(),
                        cursor: self.query.cursor,
                        selection_anchor: self.query.selection_anchor,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: if self.query.value.is_empty() {
                        Some(ValidationState::Error("Query required".into()))
                    } else {
                        None
                    },
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("toggles"),
                    label: StyledText::plain(""),
                    kind: FieldKind::ToggleGroup {
                        toggles: vec![
                            ToggleGroupItem {
                                id: WidgetId::new("case"),
                                label: "Aa".into(),
                                value: self.case_sensitive,
                            },
                            ToggleGroupItem {
                                id: WidgetId::new("regex"),
                                label: ".*".into(),
                                value: self.regex,
                            },
                            ToggleGroupItem {
                                id: WidgetId::new("word"),
                                label: "W".into(),
                                value: self.whole_word,
                            },
                        ],
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
                },
                FormField {
                    id: WidgetId::new("scope"),
                    label: StyledText::plain("Scope"),
                    kind: FieldKind::SegmentedControl {
                        options: vec!["Workspace".into(), "File".into(), "Selection".into()],
                        selected_idx: self.scope_idx,
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
                },
                FormField {
                    id: WidgetId::new("password"),
                    label: StyledText::plain("Token"),
                    kind: FieldKind::PasswordInput {
                        value: self.password.value.clone(),
                        placeholder: "API key...".into(),
                        cursor: self.password.cursor,
                        mask_char: '•',
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: if !self.password.value.is_empty() && self.password.value.len() < 4
                    {
                        Some(ValidationState::Warning("Token too short".into()))
                    } else {
                        None
                    },
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            focused_field: self
                .focused_field
                .as_ref()
                .map(|f| WidgetId::new(f.clone())),
            scroll_offset: 0,
            has_focus: self.sidebar.active_section == Some(0),
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        if let Some(f) = self.sidebar.form_mut(0) {
            *f = form;
        }
    }

    fn update_results(&mut self) {
        let files: &[(&str, &[&str])] = &[
            (
                "src/main.rs",
                &["fn main() {", "    let config = Config::load();"],
            ),
            (
                "src/config.rs",
                &["pub struct Config {", "    pub fn load() -> Self {"],
            ),
            (
                "src/app.rs",
                &[
                    "use crate::config::Config;",
                    "    pub fn run(&mut self) {",
                    "        Config::default()",
                ],
            ),
        ];

        let mut rows = Vec::new();
        for (fi, (path, matches)) in files.iter().enumerate() {
            let expanded = self.expanded.get(fi).copied().unwrap_or(true);
            rows.push(TreeRow {
                path: vec![fi as u16],
                indent: 0,
                icon: None,
                text: StyledText {
                    spans: vec![
                        quadraui::StyledSpan::plain(*path),
                        quadraui::StyledSpan {
                            text: format!(" ({})", matches.len()),
                            fg: Some(Color::rgb(120, 120, 120)),
                            bg: None,
                            bold: false,
                            italic: false,
                            underline: false,
                        },
                    ],
                },
                badge: None,
                is_expanded: Some(expanded),
                decoration: Decoration::Header,
                edit: None,
            });
            if expanded {
                for (mi, text) in matches.iter().enumerate() {
                    rows.push(TreeRow {
                        path: vec![fi as u16, mi as u16],
                        indent: 1,
                        icon: None,
                        text: StyledText::plain((*text).to_string()),
                        badge: None,
                        is_expanded: None,
                        decoration: Decoration::Normal,
                        edit: None,
                    });
                }
            }
        }
        self.sidebar.set_rows(1, rows);
    }

    fn sidebar_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(0.0, 0.0, vp.width, (vp.height - status_h).max(0.0))
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(0.0, (vp.height - status_h).max(0.0), vp.width, status_h)
    }

    fn build_status_bar(&self) -> StatusBar {
        let scope = ["Workspace", "File", "Selection"][self.scope_idx.min(2)];
        let flags = format!(
            "Aa:{} .*:{} W:{} scope:{}",
            if self.case_sensitive { "on" } else { "off" },
            if self.regex { "on" } else { "off" },
            if self.whole_word { "on" } else { "off" },
            scope,
        );
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_event),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {flags} "),
                fg: Color::rgb(180, 180, 180),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn handle_ctrl_key(&mut self, ch: char, backend: &mut dyn Backend) -> Option<Reaction> {
        match ch.to_ascii_lowercase() {
            'a' => {
                match self.focused_field.as_deref() {
                    Some("query") => self.query.apply_edit_op(EditOp::SelectAll),
                    Some("password") => self.password.apply_edit_op(EditOp::SelectAll),
                    _ => return None,
                };
                self.update_form();
                Some(Reaction::Redraw)
            }
            'c' => {
                // Copy — skip for password fields (don't expose masked text)
                if self.focused_field.as_deref() == Some("query") {
                    if let Some(text) = flat_selected_text(
                        &self.query.value,
                        self.query.cursor.unwrap_or(0),
                        self.query.selection_anchor,
                    ) {
                        backend.services().clipboard().write_text(&text);
                        self.last_event = "Copied to clipboard".into();
                        return Some(Reaction::Redraw);
                    }
                }
                None
            }
            'v' => {
                let text = backend.services().clipboard().read_text();
                if let Some(text) = text {
                    match self.focused_field.as_deref() {
                        Some("query") => self.query.apply_edit_op(EditOp::InsertText(text)),
                        Some("password") => self.password.apply_edit_op(EditOp::InsertText(text)),
                        _ => return None,
                    };
                    self.update_form();
                    Some(Reaction::Redraw)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn handle_named_key(&mut self, named: NamedKey, modifiers: Modifiers) -> Reaction {
        if self.sidebar.active_section == Some(0) {
            let field = match self.focused_field.as_deref() {
                Some("query") => Some(&mut self.query),
                Some("password") => Some(&mut self.password),
                _ => None,
            };
            if let Some(field) = field {
                let extend = modifiers.shift;
                let op = match named {
                    NamedKey::Left => Some(EditOp::MoveLeft { extend }),
                    NamedKey::Right => Some(EditOp::MoveRight { extend }),
                    NamedKey::Home => Some(EditOp::MoveLineStart { extend }),
                    NamedKey::End => Some(EditOp::MoveLineEnd { extend }),
                    NamedKey::Backspace => Some(EditOp::DeleteBackward),
                    NamedKey::Delete => Some(EditOp::DeleteForward),
                    _ => None,
                };
                let handled = op.is_some_and(|op| {
                    field.apply_edit_op(op);
                    true
                });
                if handled {
                    self.update_form();
                    return Reaction::Redraw;
                }
            }
        }
        match named {
            NamedKey::Escape => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }
}

impl Default for SidebarSearchApp {
    fn default() -> Self {
        Self::new()
    }
}

impl SidebarSearchApp {
    /// React to one action the sidebar answered, after it is applied.
    fn on_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::Form { event, .. } => {
                match &event {
                    FormEvent::ToggleChanged { id, value } => {
                        let name = id.as_str();
                        match name {
                            "case" => self.case_sensitive = *value,
                            "regex" => self.regex = *value,
                            "word" => self.whole_word = *value,
                            _ => {}
                        }
                        self.last_event = format!("ToggleChanged {name}={value}");
                    }
                    FormEvent::SegmentedControlChanged { id, selected_idx } => {
                        if id.as_str() == "scope" {
                            self.scope_idx = *selected_idx;
                        }
                        self.last_event =
                            format!("SegmentedControl {}={selected_idx}", id.as_str());
                    }
                    FormEvent::FocusChanged { id } => {
                        self.focused_field = Some(id.as_str().to_string());
                        self.last_event = format!("FocusChanged {}", id.as_str());
                    }
                    FormEvent::ButtonClicked { id } => {
                        self.last_event = format!("ButtonClicked {}", id.as_str());
                    }
                    other => {
                        self.last_event = format!("{other:?}");
                    }
                }
                self.update_form();
            }
            SidebarAction::RowSelected { section, path } => {
                if section == 1 && path.len() == 1 {
                    let fi = path[0] as usize;
                    if fi < self.expanded.len() {
                        self.expanded[fi] = !self.expanded[fi];
                        self.last_event = format!(
                            "{} file {}",
                            if self.expanded[fi] {
                                "Expanded"
                            } else {
                                "Collapsed"
                            },
                            fi
                        );
                        self.update_results();
                    }
                } else {
                    self.last_event = format!("row→{section} {path:?}");
                }
            }
            SidebarAction::HeaderActivated { section } => {
                self.last_event = format!("header→{section}");
            }
            _ => {}
        }
    }
}

impl AppLogic for SidebarSearchApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let sidebar_rect = Self::sidebar_rect(backend);
        let status_rect = Self::status_rect(backend);
        let _ = backend.draw_dyn(sidebar_rect, &self.sidebar, &());
        let _ = backend.draw_dyn(status_rect, &self.build_status_bar(), &());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let rect = Self::sidebar_rect(backend);
        match self
            .sidebar
            .handle_event(&mut self.sidebar_state, &event, backend, rect)
        {
            Handled::Event(actions) => {
                for action in actions {
                    self.sidebar.apply(&action);
                    self.on_action(action);
                }
                Reaction::Redraw
            }
            Handled::Consumed => Reaction::Redraw,
            Handled::Ignored => match event {
                UiEvent::KeyPressed {
                    key: Key::Char(ch),
                    modifiers,
                    ..
                } => {
                    if self.sidebar.active_section == Some(0) && modifiers.ctrl {
                        if let Some(reaction) = self.handle_ctrl_key(ch, backend) {
                            return reaction;
                        }
                    }
                    if self.sidebar.active_section == Some(0) && !modifiers.ctrl {
                        match self.focused_field.as_deref() {
                            Some("query") => {
                                self.query.apply_edit_op(EditOp::InsertChar(ch));
                                self.update_form();
                                return Reaction::Redraw;
                            }
                            Some("password") if !ch.is_control() => {
                                self.password.apply_edit_op(EditOp::InsertChar(ch));
                                self.update_form();
                                return Reaction::Redraw;
                            }
                            _ => {}
                        }
                    }
                    if ch == 'q' {
                        Reaction::Exit
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::CharTyped(ch) => {
                    if self.sidebar.active_section == Some(0) {
                        match self.focused_field.as_deref() {
                            Some("query") => {
                                self.query.apply_edit_op(EditOp::InsertChar(ch));
                                self.update_form();
                                return Reaction::Redraw;
                            }
                            Some("password") if !ch.is_control() => {
                                self.password.apply_edit_op(EditOp::InsertChar(ch));
                                self.update_form();
                                return Reaction::Redraw;
                            }
                            _ => {}
                        }
                    }
                    if ch == 'q' {
                        Reaction::Exit
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::KeyPressed {
                    key: Key::Named(named),
                    modifiers,
                    ..
                } => self.handle_named_key(named, modifiers),
                UiEvent::ClipboardPaste(text) => {
                    if self.sidebar.active_section == Some(0) {
                        match self.focused_field.as_deref() {
                            Some("query") => {
                                self.query.apply_edit_op(EditOp::InsertText(text));
                                self.update_form();
                                Reaction::Redraw
                            }
                            Some("password") => {
                                self.password.apply_edit_op(EditOp::InsertText(text));
                                self.update_form();
                                Reaction::Redraw
                            }
                            _ => Reaction::Continue,
                        }
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::WindowResized { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            },
        }
    }
}
