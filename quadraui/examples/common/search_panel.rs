//! Backend-agnostic app code for the search-panel spike
//! ([`tui_search_panel`] / [`gtk_search_panel`]).
//!
//! Validates that `MultiSectionView` + `TreeView` can express a
//! file-search-results UI (VSCode's "Search" sidebar shape).
//!
//! Structure:
//! - Section 0: aux=Search (query input), body=TreeView (results
//!   grouped by file). File names are `Decoration::Header` rows;
//!   match lines are `Decoration::Normal` leaves with line numbers.
//!
//! Controls:
//! - type in the search input to filter (mock — cycles fake results)
//! - click a match row to "jump" (logged in status bar)
//! - click a file header to collapse/expand
//! - ↑/↓ to scroll results
//! - q / Esc to quit

use quadraui::{
    AppLogic, Backend, Color, Key, Modifiers, MultiSectionView, MultiSectionViewHit, NamedKey,
    Reaction, Rect, Section, SectionAux, SectionBody, SectionHeader, SectionSize, StatusBar,
    StatusBarSegment, TreeRow, TreeView, TreeViewHit, UiEvent, WidgetId,
};

use quadraui::primitives::multi_section_view::{AuxHit, InlineInput};

struct SearchMatch {
    line: usize,
    text: &'static str,
}

struct FileResult {
    path: &'static str,
    matches: &'static [SearchMatch],
    expanded: bool,
}

pub struct SearchPanelApp {
    query: String,
    caret: usize,
    input_active: bool,
    results: Vec<FileResult>,
    scroll_offset: usize,
    selected_path: Option<Vec<u16>>,
    last_message: String,
}

impl SearchPanelApp {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            caret: 0,
            input_active: true,
            results: fake_results(),
            scroll_offset: 0,
            selected_path: None,
            last_message: "Type to search, click results to jump".into(),
        }
    }

    fn build_tree_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for (fi, file) in self.results.iter().enumerate() {
            rows.push(TreeRow {
                path: vec![fi as u16],
                indent: 0,
                icon: None,
                text: quadraui::StyledText {
                    spans: vec![
                        quadraui::StyledSpan::plain(file.path),
                        quadraui::StyledSpan {
                            text: format!(" ({} matches)", file.matches.len()),
                            fg: Some(Color::rgb(120, 120, 120)),
                            bg: None,
                            bold: false,
                            italic: false,
                            underline: false,
                        },
                    ],
                },
                badge: None,
                is_expanded: Some(file.expanded),
                decoration: quadraui::Decoration::Header,
                edit: None,
            });
            if file.expanded {
                for (mi, m) in file.matches.iter().enumerate() {
                    let line_prefix = format!("{:>4}: ", m.line);
                    rows.push(TreeRow {
                        path: vec![fi as u16, mi as u16],
                        indent: 1,
                        icon: None,
                        text: quadraui::StyledText {
                            spans: vec![
                                quadraui::StyledSpan {
                                    text: line_prefix,
                                    fg: Some(Color::rgb(100, 100, 100)),
                                    bg: None,
                                    bold: false,
                                    italic: false,
                                    underline: false,
                                },
                                quadraui::StyledSpan::plain(m.text),
                            ],
                        },
                        badge: None,
                        is_expanded: None,
                        decoration: quadraui::Decoration::Normal,
                        edit: None,
                    });
                }
            }
        }
        rows
    }

    fn build_view(&self) -> MultiSectionView {
        let rows = self.build_tree_rows();
        let tree = TreeView {
            id: WidgetId::new("results"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: self.selected_path.clone(),
            scroll_offset: self.scroll_offset,
            style: quadraui::TreeStyle::default(),
            has_focus: !self.input_active,
        };

        MultiSectionView {
            id: WidgetId::new("search-panel"),
            sections: vec![Section {
                id: "results".into(),
                header: SectionHeader {
                    icon: None,
                    title: quadraui::StyledText {
                        spans: vec![quadraui::StyledSpan::plain("SEARCH RESULTS")],
                    },
                    badge: None,
                    actions: vec![],
                    show_chevron: false,
                },
                body: SectionBody::Tree(tree),
                aux: Some(SectionAux::Search(InlineInput {
                    id: WidgetId::new("search-input"),
                    text: self.query.clone(),
                    caret: self.caret,
                    placeholder: Some("Search".into()),
                    has_focus: self.input_active,
                })),
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: quadraui::primitives::multi_section_view::Axis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: quadraui::primitives::multi_section_view::ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let match_count: usize = self.results.iter().map(|f| f.matches.len()).sum();
        let file_count = self.results.len();
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {match_count} matches in {file_count} files "),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for SearchPanelApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SearchPanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
        let view = self.build_view();
        backend.draw_multi_section_view(panel_rect, &view);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.input_active {
                    self.input_active = false;
                    return Reaction::Redraw;
                }
                return Reaction::Exit;
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => return Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char(ch), ..
            } if self.input_active => {
                self.query.insert(self.caret, ch);
                self.caret += 1;
                self.last_message = format!("Searching: {}", self.query);
                return Reaction::Redraw;
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Backspace),
                ..
            } if self.input_active && self.caret > 0 => {
                self.caret -= 1;
                self.query.remove(self.caret);
                self.last_message = format!("Searching: {}", self.query);
                return Reaction::Redraw;
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } if !self.input_active => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                }
                return Reaction::Redraw;
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } if !self.input_active => {
                self.scroll_offset += 1;
                return Reaction::Redraw;
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let view = self.build_view();
                let layout = backend.msv_layout(panel_rect, &view);
                match layout.hit_test(position.x, position.y) {
                    MultiSectionViewHit::Aux {
                        kind: AuxHit::Input,
                        ..
                    } => {
                        self.input_active = true;
                        self.last_message = "Input active".into();
                    }
                    MultiSectionViewHit::Body { section, .. } => {
                        self.input_active = false;
                        if let Some(sl) = layout.sections.get(section) {
                            if let SectionBody::Tree(ref tree) = view.sections[section].body {
                                let tree_layout = backend.tree_layout(sl.body_bounds, tree);
                                let local_x = position.x - sl.body_bounds.x;
                                let local_y = position.y - sl.body_bounds.y;
                                match tree_layout.hit_test(local_x, local_y) {
                                    TreeViewHit::Row(row_idx) => {
                                        let row = &tree.rows[row_idx];
                                        if row.is_expanded.is_some() {
                                            // File header — toggle expand.
                                            let fi = row.path[0] as usize;
                                            if fi < self.results.len() {
                                                self.results[fi].expanded =
                                                    !self.results[fi].expanded;
                                                self.last_message = format!(
                                                    "{} {}",
                                                    if self.results[fi].expanded {
                                                        "Expanded"
                                                    } else {
                                                        "Collapsed"
                                                    },
                                                    self.results[fi].path
                                                );
                                            }
                                        } else {
                                            // Match row — jump.
                                            self.selected_path = Some(row.path.clone());
                                            if row.path.len() >= 2 {
                                                let fi = row.path[0] as usize;
                                                let mi = row.path[1] as usize;
                                                if fi < self.results.len()
                                                    && mi < self.results[fi].matches.len()
                                                {
                                                    let m = &self.results[fi].matches[mi];
                                                    self.last_message = format!(
                                                        "Jump: {}:{}",
                                                        self.results[fi].path, m.line
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    TreeViewHit::Empty => {
                                        self.last_message = "Empty area".into();
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        self.input_active = false;
                    }
                }
                return Reaction::Redraw;
            }
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }
        Reaction::Continue
    }
}

fn fake_results() -> Vec<FileResult> {
    vec![
        FileResult {
            path: "src/main.rs",
            matches: &[
                SearchMatch {
                    line: 12,
                    text: "fn main() {",
                },
                SearchMatch {
                    line: 45,
                    text: "    let config = Config::load();",
                },
                SearchMatch {
                    line: 78,
                    text: "    app.run(config)?;",
                },
            ],
            expanded: true,
        },
        FileResult {
            path: "src/config.rs",
            matches: &[
                SearchMatch {
                    line: 5,
                    text: "pub struct Config {",
                },
                SearchMatch {
                    line: 23,
                    text: "    pub fn load() -> Self {",
                },
            ],
            expanded: true,
        },
        FileResult {
            path: "src/app.rs",
            matches: &[
                SearchMatch {
                    line: 1,
                    text: "use crate::config::Config;",
                },
                SearchMatch {
                    line: 34,
                    text: "    pub fn run(&mut self, config: Config) -> Result<()> {",
                },
                SearchMatch {
                    line: 67,
                    text: "        self.config = config;",
                },
                SearchMatch {
                    line: 89,
                    text: "        Config::default()",
                },
            ],
            expanded: true,
        },
    ]
}
