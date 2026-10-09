//! `Tree` demo — adapted from `quadraui/examples/common/multi_tree.rs`.
//!
//! Two variants: a 4-section debugger sidebar (Variables / Watch / Call
//! Stack / Breakpoints) driven by [`SidebarSystem`] with `Tab` cycling
//! and click-to-activate, and a single expandable file tree that owns
//! its own `is_expanded` flips in response to
//! [`SidebarEvent::RowToggleExpand`].

use quadraui::{
    Backend, BackendCaps, Color, Decoration, InteractionState, Key, NavigationMode, Reaction, Rect,
    ScrollMode, SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBar, StatusBarSegment,
    StyledText, TreeRow, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("tree.rs");

// gallery:begin
fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("{prefix}{i}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        })
        .collect()
}

/// A tiny two-level file tree: `src/` expanded with two children,
/// `tests/` collapsed with one hidden child — the app (not the
/// primitive) owns which children are currently visible.
struct FileTree {
    src_expanded: bool,
    tests_expanded: bool,
}

impl FileTree {
    fn rows(&self) -> Vec<TreeRow> {
        let mut rows = vec![TreeRow {
            path: vec![0],
            indent: 0,
            icon: None,
            text: StyledText::plain("src/"),
            badge: None,
            is_expanded: Some(self.src_expanded),
            decoration: Decoration::Normal,
            edit: None,
        }];
        if self.src_expanded {
            rows.push(TreeRow {
                path: vec![0, 0],
                indent: 1,
                icon: None,
                text: StyledText::plain("main.rs"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            });
            rows.push(TreeRow {
                path: vec![0, 1],
                indent: 1,
                icon: None,
                text: StyledText::plain("lib.rs"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            });
        }
        rows.push(TreeRow {
            path: vec![1],
            indent: 0,
            icon: None,
            text: StyledText::plain("tests/"),
            badge: None,
            is_expanded: Some(self.tests_expanded),
            decoration: Decoration::Normal,
            edit: None,
        });
        if self.tests_expanded {
            rows.push(TreeRow {
                path: vec![1, 0],
                indent: 1,
                icon: None,
                text: StyledText::plain("smoke.rs"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            });
        }
        rows
    }

    /// Flip the expand state of the top-level row named by `path[0]`
    /// (`0` = `src/`, `1` = `tests/`) and return a label for the status
    /// line.
    fn toggle(&mut self, path: &[u16]) -> String {
        match path.first() {
            Some(0) => {
                self.src_expanded = !self.src_expanded;
                format!(
                    "src/ → {}",
                    if self.src_expanded {
                        "expanded"
                    } else {
                        "collapsed"
                    }
                )
            }
            Some(1) => {
                self.tests_expanded = !self.tests_expanded;
                format!(
                    "tests/ → {}",
                    if self.tests_expanded {
                        "expanded"
                    } else {
                        "collapsed"
                    }
                )
            }
            _ => "—".into(),
        }
    }
}

pub struct TreeDemo {
    // Variant 0: multi-section debug sidebar.
    debug: SidebarSystem,
    debug_last: String,
    // Variant 1: single expandable file tree.
    file_tree: FileTree,
    file_sidebar: SidebarSystem,
    file_last: String,
}

impl TreeDemo {
    pub fn new() -> Self {
        let mut debug = SidebarSystem::new(vec![
            SidebarSectionDef::new("variables", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("call-stack", "CALL STACK"),
            SidebarSectionDef::new("breakpoints", "BREAKPOINTS"),
        ]);
        debug.set_rows(0, fake_rows("v", 8));
        debug.set_rows(1, fake_rows("w", 4));
        debug.set_rows(2, fake_rows("frame", 3));
        debug.set_rows(3, fake_rows("bp", 0));
        debug.set_navigation_mode(NavigationMode::Selection);
        debug.set_scroll_mode(ScrollMode::WholePanel);
        debug.set_active_section(Some(0));

        let file_tree = FileTree {
            src_expanded: true,
            tests_expanded: false,
        };
        let mut file_sidebar =
            SidebarSystem::new(vec![SidebarSectionDef::new("files", "EXPLORER")]);
        file_sidebar.set_rows(0, file_tree.rows());
        file_sidebar.set_navigation_mode(NavigationMode::Selection);
        file_sidebar.set_active_section(Some(0));

        Self {
            debug,
            debug_last: "—".into(),
            file_tree,
            file_sidebar,
            file_last: "—".into(),
        }
    }

    fn content_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self, variant: usize) -> StatusBar {
        let text = if variant == 0 {
            format!(" Tab cycles sections — last: {} ", self.debug_last)
        } else {
            format!(
                " click a header (or Space on it) to expand/collapse — last: {} ",
                self.file_last
            )
        };
        StatusBar {
            id: WidgetId::new("gallery:tree:status"),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for TreeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TreeDemo {
    fn name(&self) -> &'static str {
        "Tree"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Debug panel (4 sections)", "File tree (expand/collapse)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let content = Self::content_rect(area, backend);
        if variant == 0 {
            self.debug.render(backend, content);
        } else {
            self.file_sidebar.render(backend, content);
        }
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
        let content = Self::content_rect(area, backend);
        if variant == 0 {
            match self.debug.handle(event, backend, content) {
                SidebarEvent::HeaderActivated { section } => {
                    self.debug_last = format!("header→{section}");
                    Reaction::Redraw
                }
                SidebarEvent::RowSelected { section, path } => {
                    self.debug_last = format!("sel→{section} {path:?}");
                    Reaction::Redraw
                }
                SidebarEvent::StateChanged
                | SidebarEvent::Consumed
                | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
                SidebarEvent::Ignored => Reaction::Continue,
                _ => Reaction::Redraw,
            }
        } else {
            match self.file_sidebar.handle(event, backend, content) {
                SidebarEvent::RowToggleExpand { path, .. } => {
                    self.file_last = self.file_tree.toggle(&path);
                    self.file_sidebar.set_rows(0, self.file_tree.rows());
                    Reaction::Redraw
                }
                SidebarEvent::RowSelected { path, .. } => {
                    self.file_last = format!("sel {path:?}");
                    Reaction::Redraw
                }
                SidebarEvent::StateChanged
                | SidebarEvent::Consumed
                | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
                SidebarEvent::Ignored => match event {
                    UiEvent::KeyPressed {
                        key: Key::Char(' '),
                        ..
                    } => {
                        let ev = self.file_sidebar.toggle_expand_selected();
                        if let SidebarEvent::RowToggleExpand { path, .. } = ev {
                            self.file_last = self.file_tree.toggle(&path);
                            self.file_sidebar.set_rows(0, self.file_tree.rows());
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    _ => Reaction::Continue,
                },
                _ => Reaction::Redraw,
            }
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        if variant == 0 {
            serde_json::json!({
                "active_section": self.debug.active_section(),
                "last": self.debug_last,
            })
        } else {
            serde_json::json!({
                "src_expanded": self.file_tree.src_expanded,
                "tests_expanded": self.file_tree.tests_expanded,
                "last": self.file_last,
            })
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
    fn file_tree_toggle_flips_src_expanded() {
        let mut tree = FileTree {
            src_expanded: true,
            tests_expanded: false,
        };
        tree.toggle(&[0]);
        assert!(!tree.src_expanded);
    }

    #[test]
    fn file_tree_rows_hide_children_when_collapsed() {
        let tree = FileTree {
            src_expanded: false,
            tests_expanded: false,
        };
        let rows = tree.rows();
        assert_eq!(rows.len(), 2, "only the two top-level headers should show");
    }

    #[test]
    fn new_starts_with_the_first_debug_section_active() {
        let demo = TreeDemo::new();
        assert_eq!(demo.debug.active_section(), Some(0));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn space_on_the_file_tree_toggles_expand_and_redraws() {
        let mut demo = TreeDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 20.0);
        // Select the first row (src/) so `toggle_expand_selected` has a
        // target.
        demo.file_sidebar.set_selected_path(0, Some(vec![0]));
        let event = UiEvent::KeyPressed {
            key: Key::Char(' '),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(1, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(!demo.file_tree.src_expanded);
    }
}
