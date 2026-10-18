//! Search panel demo — adapted from
//! `quadraui/examples/common/search_panel.rs`.
//!
//! Validates that [`MultiSectionView`] + [`TreeView`] express a
//! file-search-results UI: file names are [`Decoration::Header`] rows
//! with a chevron, match lines are plain leaves. Click a match to
//! "jump" (logged in the status line); click a header (or its
//! chevron) to collapse/expand. Two variants: results found, and an
//! empty query with no matches.

use quadraui::primitives::multi_section_view::{AuxHit, InlineInput};
use quadraui::{
    Backend, BackendCaps, Color, Decoration, InteractionState, MultiSectionView,
    MultiSectionViewHit, Reaction, Rect, Section, SectionAux, SectionBody, SectionHeader,
    SectionSize, StatusBar, StatusBarSegment, StyledSpan, StyledText, TreeRow, TreeView,
    TreeViewHit, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("search_panel.rs");

// gallery:begin
struct SearchMatch {
    line: usize,
    text: &'static str,
}

struct FileResult {
    path: &'static str,
    matches: &'static [SearchMatch],
    expanded: bool,
}

pub struct SearchPanelDemo {
    results: Vec<Vec<FileResult>>,
    selected_path: Vec<Option<Vec<u16>>>,
    last_message: Vec<String>,
}

impl SearchPanelDemo {
    pub fn new() -> Self {
        Self {
            results: vec![fake_results(), Vec::new()],
            selected_path: vec![None, None],
            last_message: vec![
                "Click a result to jump".into(),
                "No matches for this query".into(),
            ],
        }
    }

    fn build_tree_rows(&self, variant: usize) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for (fi, file) in self.results[variant].iter().enumerate() {
            rows.push(TreeRow {
                path: vec![fi as u16],
                indent: 0,
                icon: None,
                text: StyledText {
                    spans: vec![
                        StyledSpan::plain(file.path),
                        StyledSpan {
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
                decoration: Decoration::Header,
                edit: None,
            });
            if file.expanded {
                for (mi, m) in file.matches.iter().enumerate() {
                    rows.push(TreeRow {
                        path: vec![fi as u16, mi as u16],
                        indent: 1,
                        icon: None,
                        text: StyledText {
                            spans: vec![
                                StyledSpan {
                                    text: format!("{:>4}: ", m.line),
                                    fg: Some(Color::rgb(100, 100, 100)),
                                    bg: None,
                                    bold: false,
                                    italic: false,
                                    underline: false,
                                },
                                StyledSpan::plain(m.text),
                            ],
                        },
                        badge: None,
                        is_expanded: None,
                        decoration: Decoration::Normal,
                        edit: None,
                    });
                }
            }
        }
        rows
    }

    fn build_view(&self, variant: usize) -> MultiSectionView {
        let rows = self.build_tree_rows(variant);
        let tree = TreeView {
            id: WidgetId::new("gallery:search-panel:results"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: self.selected_path[variant].clone(),
            scroll_offset: 0,
            style: quadraui::TreeStyle::default(),
            has_focus: true,
        };
        MultiSectionView {
            id: WidgetId::new("gallery:search-panel"),
            sections: vec![Section {
                id: "results".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("SEARCH RESULTS"),
                    badge: None,
                    actions: vec![],
                    show_chevron: false,
                },
                body: SectionBody::Tree(tree),
                aux: Some(SectionAux::Search(InlineInput {
                    id: WidgetId::new("gallery:search-panel:input"),
                    text: String::new(),
                    caret: 0,
                    placeholder: Some("Search".into()),
                    has_focus: false,
                })),
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: quadraui::MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: quadraui::ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn status(&self, variant: usize) -> StatusBar {
        let match_count: usize = self.results[variant].iter().map(|f| f.matches.len()).sum();
        let file_count = self.results[variant].len();
        StatusBar {
            id: WidgetId::new("gallery:search-panel:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message[variant]),
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

    fn panel_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for SearchPanelDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for SearchPanelDemo {
    fn name(&self) -> &'static str {
        "Search Panel"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Results found", "No matches"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let panel_rect = Self::panel_rect(area, backend);
        backend.draw_multi_section_view(panel_rect, &self.build_view(variant));

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = event {
            let panel_rect = Self::panel_rect(area, backend);
            let view = self.build_view(variant);
            let layout = backend.msv_layout(panel_rect, &view);
            match layout.hit_test(position.x, position.y) {
                MultiSectionViewHit::Aux {
                    kind: AuxHit::Input,
                    ..
                } => {
                    self.last_message[variant] = "Input focused".into();
                }
                MultiSectionViewHit::Body { section, .. } => {
                    if let Some(sl) = layout.sections.get(section) {
                        if let SectionBody::Tree(ref tree) = view.sections[section].body {
                            let tree_layout = backend.tree_layout(sl.body_bounds, tree);
                            let local_x = position.x - sl.body_bounds.x;
                            let local_y = position.y - sl.body_bounds.y;
                            match tree_layout.hit_test(local_x, local_y) {
                                TreeViewHit::Chevron(row_idx) | TreeViewHit::Row(row_idx)
                                    if tree.rows[row_idx].is_expanded.is_some() =>
                                {
                                    let fi = tree.rows[row_idx].path[0] as usize;
                                    if let Some(file) = self.results[variant].get_mut(fi) {
                                        file.expanded = !file.expanded;
                                        self.last_message[variant] = format!(
                                            "{} {}",
                                            if file.expanded {
                                                "Expanded"
                                            } else {
                                                "Collapsed"
                                            },
                                            file.path
                                        );
                                    }
                                }
                                TreeViewHit::Row(row_idx) => {
                                    let row = &tree.rows[row_idx];
                                    self.selected_path[variant] = Some(row.path.clone());
                                    if row.path.len() >= 2 {
                                        let fi = row.path[0] as usize;
                                        let mi = row.path[1] as usize;
                                        if let Some(file) = self.results[variant].get(fi) {
                                            if let Some(m) = file.matches.get(mi) {
                                                self.last_message[variant] =
                                                    format!("Jump: {}:{}", file.path, m.line);
                                            }
                                        }
                                    }
                                }
                                TreeViewHit::Chevron(_) | TreeViewHit::Empty => {
                                    self.last_message[variant] = "Empty area".into();
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            return Reaction::Redraw;
        }
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "file_count": self.results[variant].len(),
            "match_count": self.results[variant].iter().map(|f| f.matches.len()).sum::<usize>(),
            "last_message": self.last_message[variant],
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

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
            ],
            expanded: true,
        },
        FileResult {
            path: "src/config.rs",
            matches: &[SearchMatch {
                line: 5,
                text: "pub struct Config {",
            }],
            expanded: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_matches_variant_has_no_results() {
        let demo = SearchPanelDemo::new();
        assert!(demo.results[1].is_empty());
    }

    #[test]
    fn results_variant_starts_with_two_expanded_files() {
        let demo = SearchPanelDemo::new();
        assert_eq!(demo.results[0].len(), 2);
        assert!(demo.results[0].iter().all(|f| f.expanded));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_a_file_header_collapses_it() {
        let mut demo = SearchPanelDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let panel_rect = SearchPanelDemo::panel_rect(area, &backend);
        let view = demo.build_view(0);
        let layout = backend.msv_layout(panel_rect, &view);
        let sl = &layout.sections[0];
        let tree_rect = sl.body_bounds;
        // First row of the tree body is the first file header.
        let event = UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: quadraui::Point::new(tree_rect.x + 1.0, tree_rect.y + 0.0),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(!demo.results[0][0].expanded);
    }
}
