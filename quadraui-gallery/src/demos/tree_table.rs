//! `TreeTable` demo — a small file tree with Name / Date Modified / Size
//! / Kind columns, driven entirely through
//! [`quadraui::compose::TreeTableController`]: the demo owns no hierarchy
//! walk, flatten or navigation logic of its own.
//!
//! `remote/` is a lazy branch
//! ([`quadraui::compose::TreeTableNodeData::lazy_branch`]): expanding it
//! for the first time emits
//! [`quadraui::compose::TreeTableControllerEvent::ChildrenRequested`],
//! which this demo answers synchronously via
//! [`quadraui::compose::TreeTableController::set_children`] — standing in
//! for the async fetch a real app (e.g. a remote filesystem browser)
//! would do instead.
//!
//! Narrower than ~56 cells drops `Kind`, then `Size`, then `Date
//! Modified`, from the right, keeping `Name` (and therefore the tree
//! cell) always visible.

use quadraui::compose::{TreeTableController, TreeTableControllerEvent, TreeTableNodeData};
use quadraui::{
    Backend, BackendCaps, Color, Column, ColumnAlign, ColumnWidth, InteractionState, Reaction,
    Rect, StatusBar, StatusBarSegment, Style, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("tree_table.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it, so a chrome font taller than
/// the design token isn't clipped — or the plain chrome line height on
/// TUI's fixed cell grid, where every `Style` chrome metric is a
/// documented no-op. Mirrors `AppShell`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin

/// One row's four cells.
fn row(label: &str, date: &str, size: &str, kind: &str) -> Vec<StyledText> {
    vec![
        StyledText::plain(label),
        StyledText::plain(date),
        StyledText::plain(size),
        StyledText::plain(kind),
    ]
}

/// The demo's static hierarchy — eagerly-loaded everywhere except
/// `remote/`, a [`TreeTableNodeData::lazy_branch`] whose children arrive
/// only once the user expands it (see [`TreeTableDemo::handle`]'s
/// [`TreeTableControllerEvent::ChildrenRequested`] arm).
fn demo_tree() -> Vec<TreeTableNodeData> {
    vec![
        TreeTableNodeData::branch(
            "src",
            row("src", "2026-02-01", "--", "Folder"),
            vec![
                TreeTableNodeData::leaf("main.rs", row("main.rs", "2026-02-01", "1.8 KB", "Rust")),
                TreeTableNodeData::leaf("lib.rs", row("lib.rs", "2026-01-28", "4.1 KB", "Rust")),
                TreeTableNodeData::branch(
                    "util",
                    row("util", "2026-01-20", "--", "Folder"),
                    vec![TreeTableNodeData::leaf(
                        "helpers.rs",
                        row("helpers.rs", "2026-01-20", "0.9 KB", "Rust"),
                    )],
                ),
            ],
        ),
        TreeTableNodeData::branch(
            "docs",
            row("docs", "2025-12-15", "--", "Folder"),
            vec![TreeTableNodeData::leaf(
                "README.md",
                row("README.md", "2025-12-15", "2.2 KB", "Markdown"),
            )],
        ),
        TreeTableNodeData::lazy_branch("remote", row("remote", "—", "--", "Folder")),
        TreeTableNodeData::leaf(
            "Cargo.toml",
            row("Cargo.toml", "2026-01-28", "0.5 KB", "TOML"),
        ),
    ]
}

/// The children [`TreeTableDemo::handle`] supplies in response to
/// `remote/`'s one [`TreeTableControllerEvent::ChildrenRequested`] —
/// standing in for an async fetch a real remote-filesystem browser would
/// do instead.
fn remote_children() -> Vec<TreeTableNodeData> {
    vec![
        TreeTableNodeData::leaf("a.log", row("a.log", "2026-02-10", "12 KB", "Log")),
        TreeTableNodeData::leaf("b.log", row("b.log", "2026-02-09", "9 KB", "Log")),
    ]
}

/// All four columns, narrowest-rect-safe: `Kind`, then `Size`, then
/// `Date Modified` drop from the right as `width` shrinks, keeping
/// `Name` — and therefore the tree cell itself — always visible.
fn columns_for(width: f32) -> Vec<Column> {
    let all = [
        Column {
            title: "Name".into(),
            width: ColumnWidth::Flex(3.0),
            align: ColumnAlign::Left,
        },
        Column {
            title: "Date Modified".into(),
            width: ColumnWidth::FixedCells(14.0),
            align: ColumnAlign::Left,
        },
        Column {
            title: "Size".into(),
            width: ColumnWidth::FixedCells(8.0),
            align: ColumnAlign::Right,
        },
        Column {
            title: "Kind".into(),
            width: ColumnWidth::FixedCells(10.0),
            align: ColumnAlign::Left,
        },
    ];
    // Each threshold is the dropped column's own fixed width plus the
    // narrowest `Name` the demo still wants visible at that step: 56 =
    // 32 (`Date Modified` + `Size` + `Kind`) + 24; 46 = 22 (`Date
    // Modified` + `Size`) + 24; 34 = 14 (`Date Modified`) + 20. `Name`
    // itself has no fixed width — it's `Flex` — so these are a floor the
    // demo picks for itself, not a hard minimum the shared `TreeTable`
    // layout enforces (see `tree_column_floor_width` in `tui::data_table`
    // for that).
    let n = if width >= 56.0 {
        4
    } else if width >= 46.0 {
        3
    } else if width >= 34.0 {
        2
    } else {
        1
    };
    all.into_iter().take(n).collect()
}

pub struct TreeTableDemo {
    controller: TreeTableController,
    last: String,
}

impl TreeTableDemo {
    pub fn new() -> Self {
        let mut controller = TreeTableController::new("gallery:tree-table:files");
        controller.set_nodes(demo_tree());
        controller.set_expanded("src".to_string(), true);
        controller.set_selected_id(Some("src".to_string()));
        Self {
            controller,
            last: "—".into(),
        }
    }

    fn table_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:tree-table:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " ↑↓ select, ←→ collapse/expand, click chevron — last: {} ",
                    self.last
                ),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    /// Apply one [`TreeTableControllerEvent`] to `self.last`'s status
    /// text, and — for [`TreeTableControllerEvent::ChildrenRequested`] —
    /// answer it on the spot with [`remote_children`]. The one place this
    /// demo reacts to a controller event; every mouse/keyboard arm in
    /// [`Self::handle`] funnels through [`TreeTableController::handle`]
    /// and lands here with the result.
    fn describe(&mut self, event: TreeTableControllerEvent) {
        match event {
            TreeTableControllerEvent::ChildrenRequested { id } => {
                self.last = format!("{id} children requested — loaded");
                self.controller.set_children(&id, remote_children());
            }
            TreeTableControllerEvent::ToggleExpand { id, expand } => {
                self.last = format!("{id} {}", if expand { "expanded" } else { "collapsed" });
            }
            TreeTableControllerEvent::RowSelected { id } => {
                self.last = format!("{id} selected");
            }
            TreeTableControllerEvent::RowActivated { id } => {
                self.last = format!("{id} activated");
            }
            TreeTableControllerEvent::SortChanged { col, ascending } => {
                self.last = format!(
                    "sorted by column {col} ({})",
                    if ascending { "asc" } else { "desc" }
                );
            }
            TreeTableControllerEvent::Consumed | TreeTableControllerEvent::Ignored => {}
        }
    }
}

impl Default for TreeTableDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TreeTableDemo {
    fn name(&self) -> &'static str {
        "Tree Table"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let table_rect = Self::table_rect(area, backend);
        self.controller
            .render(backend, table_rect, columns_for(table_rect.width));

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let table_rect = Self::table_rect(area, backend);
        let columns = columns_for(table_rect.width);
        match event {
            UiEvent::KeyPressed { .. }
            | UiEvent::MouseDown { .. }
            | UiEvent::DoubleClick { .. } => {
                let outcome = self.controller.handle(event, backend, table_rect, columns);
                let redraw = !matches!(outcome, TreeTableControllerEvent::Ignored);
                self.describe(outcome);
                if redraw {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::to_value(self.controller.flatten(columns_for(80.0)))
            .unwrap_or(serde_json::Value::Null)
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
    fn new_selects_src_and_starts_unsorted() {
        let demo = TreeTableDemo::new();
        assert_eq!(demo.controller.selected_id(), Some(&"src".to_string()));
        assert_eq!(demo.controller.flatten(columns_for(80.0)).table.sort, None);
    }

    #[test]
    fn src_starts_expanded_and_docs_starts_collapsed() {
        let demo = TreeTableDemo::new();
        assert!(demo.controller.is_expanded(&"src".to_string()));
        assert!(!demo.controller.is_expanded(&"docs".to_string()));
    }

    #[test]
    fn narrow_width_drops_columns_from_the_right() {
        assert_eq!(columns_for(80.0).len(), 4);
        assert_eq!(columns_for(50.0).len(), 3);
        assert_eq!(columns_for(40.0).len(), 2);
        assert_eq!(columns_for(10.0).len(), 1);
        assert_eq!(columns_for(10.0)[0].title, "Name");
    }

    #[test]
    fn expanding_the_lazy_remote_branch_requests_and_loads_its_children() {
        let mut demo = TreeTableDemo::new();
        let ev = demo.controller.set_expanded("remote".to_string(), true);
        assert_eq!(
            ev,
            TreeTableControllerEvent::ChildrenRequested {
                id: "remote".to_string()
            }
        );
        demo.describe(ev);
        assert_eq!(demo.last, "remote children requested — loaded");
        let table = demo.controller.flatten(columns_for(80.0));
        assert!(table
            .table
            .rows
            .iter()
            .any(|r| r.cells.first().map(|c| c.spans[0].text.as_str()) == Some("a.log")));
    }

    #[test]
    fn collapsing_src_hides_its_children() {
        let mut demo = TreeTableDemo::new();
        demo.controller.set_expanded("src".to_string(), false);
        let table = demo.controller.flatten(columns_for(80.0));
        assert!(!table.table.rows.iter().any(|r| r
            .cells
            .first()
            .map(|c| c.spans[0].text.as_str())
            == Some("main.rs")));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_the_chevron_collapses_the_expanded_src_row() {
        let mut demo = TreeTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let table_rect = TreeTableDemo::table_rect(area, &backend);
        let columns = columns_for(table_rect.width);
        let table = demo.controller.flatten(columns.clone());
        let layout = backend.tree_table_layout(table_rect, &table);
        let (idx, bounds) = layout.chevrons[0];
        assert_eq!(table.tree[idx].path, vec![0]);
        let event = UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: quadraui::Point::new(
                table_rect.x + bounds.x + 0.5,
                table_rect.y + bounds.y + 0.5,
            ),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(
            !demo.controller.is_expanded(&"src".to_string()),
            "clicking the chevron should collapse src/"
        );
    }

    #[test]
    fn clicking_a_header_sorts_by_it() {
        let mut demo = TreeTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let table_rect = TreeTableDemo::table_rect(area, &backend);
        let columns = columns_for(table_rect.width);
        let table = demo.controller.flatten(columns);
        let layout = backend.tree_table_layout(table_rect, &table);
        let col1 = layout.table.columns[1];
        let event = UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: quadraui::Point::new(table_rect.x + col1.x + col1.width / 2.0, table_rect.y),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(
            demo.controller
                .flatten(columns_for(table_rect.width))
                .table
                .sort,
            Some((1, quadraui::SortDirection::Ascending))
        );
    }

    #[test]
    fn left_right_up_down_follow_the_shared_navigation_rule() {
        let mut demo = TreeTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);

        // Down from `src/` lands on its first child.
        let down = UiEvent::KeyPressed {
            key: quadraui::Key::Named(quadraui::NamedKey::Down),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &down, &mut backend, area);
        assert_eq!(demo.controller.selected_id(), Some(&"main.rs".to_string()));

        // Left on a leaf row selects its parent.
        let left = UiEvent::KeyPressed {
            key: quadraui::Key::Named(quadraui::NamedKey::Left),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &left, &mut backend, area);
        assert_eq!(demo.controller.selected_id(), Some(&"src".to_string()));

        // Left again on the now-selected, still-expanded `src/` collapses
        // it (selection itself doesn't move).
        demo.handle(0, &left, &mut backend, area);
        assert!(!demo.controller.is_expanded(&"src".to_string()));
        assert_eq!(demo.controller.selected_id(), Some(&"src".to_string()));

        // Right re-expands it.
        let right = UiEvent::KeyPressed {
            key: quadraui::Key::Named(quadraui::NamedKey::Right),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &right, &mut backend, area);
        assert!(demo.controller.is_expanded(&"src".to_string()));
    }

    #[test]
    fn narrow_rect_drops_trailing_columns_but_keeps_the_tree_column() {
        let demo = TreeTableDemo::new();
        let backend = quadraui::tui::TuiBackend::new();
        let wide = TreeTableDemo::table_rect(Rect::new(0.0, 0.0, 80.0, 20.0), &backend);
        let narrow = TreeTableDemo::table_rect(Rect::new(0.0, 0.0, 20.0, 20.0), &backend);
        assert_eq!(
            demo.controller
                .flatten(columns_for(wide.width))
                .table
                .columns
                .len(),
            4
        );
        let narrow_table = demo.controller.flatten(columns_for(narrow.width));
        assert!(narrow_table.table.columns.len() < 4);
        assert_eq!(narrow_table.table.columns[0].title, "Name");
    }
}
