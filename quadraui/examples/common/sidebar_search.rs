//! SidebarSystem-based search panel — manual smoke test for Form
//! sections with ToggleGroup/ButtonRow items (#105, #112) and
//! Header-decorated tree rows (#110).
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
//! - q / Esc to quit

use quadraui::primitives::form::{FieldKind, FormField, ToggleGroupItem};
use quadraui::{
    AppLogic, Backend, Color, Decoration, Form, FormEvent, Key, NamedKey, NavigationMode, Reaction,
    Rect, SectionKind, SectionSize, SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBar,
    StatusBarSegment, StyledText, TreeRow, UiEvent, WidgetId,
};

const STATUS_BAR_LINES: f32 = 1.5;

pub struct SidebarSearchApp {
    sidebar: SidebarSystem,
    case_sensitive: bool,
    regex: bool,
    whole_word: bool,
    query: String,
    expanded: Vec<bool>,
    last_event: String,
}

impl SidebarSearchApp {
    pub fn new() -> Self {
        let mut sidebar = SidebarSystem::new(vec![
            SidebarSectionDef::form("search", "SEARCH"),
            SidebarSectionDef::new("results", "RESULTS"),
        ]);
        sidebar.set_navigation_mode(NavigationMode::Selection);
        sidebar.set_active_section(Some(0));

        let expanded = vec![true; 3];
        let mut app = Self {
            sidebar,
            case_sensitive: false,
            regex: false,
            whole_word: false,
            query: String::new(),
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
                        value: self.query.clone(),
                        placeholder: "Search...".into(),
                        cursor: Some(self.query.len()),
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
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
                },
            ],
            focused_field: Some(WidgetId::new("query")),
            scroll_offset: 0,
            has_focus: self.sidebar.active_section() == Some(0),
        };
        self.sidebar.set_form(0, form);
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
        let flags = format!(
            "Aa:{} .*:{} W:{}",
            if self.case_sensitive { "on" } else { "off" },
            if self.regex { "on" } else { "off" },
            if self.whole_word { "on" } else { "off" },
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
}

impl Default for SidebarSearchApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SidebarSearchApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let sidebar_rect = Self::sidebar_rect(backend);
        let status_rect = Self::status_rect(backend);
        self.sidebar.render(backend, sidebar_rect);
        let _ = backend.draw_status_bar(status_rect, &self.build_status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let rect = Self::sidebar_rect(backend);
        match self.sidebar.handle(&event, backend, rect) {
            SidebarEvent::FormEvent { event, .. } => {
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
                    FormEvent::FocusChanged { id } => {
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
                Reaction::Redraw
            }
            SidebarEvent::RowSelected { section, path } => {
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
                Reaction::Redraw
            }
            SidebarEvent::HeaderActivated { section } => {
                self.last_event = format!("header→{section}");
                Reaction::Redraw
            }
            SidebarEvent::StateChanged
            | SidebarEvent::Consumed
            | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
            SidebarEvent::Ignored => match event {
                UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                }
                | UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => Reaction::Exit,
                UiEvent::WindowResized { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            },
            _ => Reaction::Redraw,
        }
    }
}
