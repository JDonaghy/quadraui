//! `TreeTable` demo — a small file tree with Name / Date Modified / Size
//! / Kind columns, built from [`quadraui::TreeTable`] (a [`DataTable`]
//! plus per-row tree metadata) rather than a parallel primitive.
//!
//! Chevron click, Left/Right expand-or-select-parent / collapse-or-
//! select-first-child, and Up/Down selection all go through
//! [`TreeNavCore::horizontal_nav`] — the same navigation core
//! `TreeController` already uses for `TreeView` — rather than
//! re-implementing the rule. Header click toggles a sort indicator
//! (cosmetic only: a tree's row order is its hierarchy, not a sortable
//! column). Narrower than ~56 cells drops `Kind`, then `Size`, then
//! `Date Modified`, from the right, keeping `Name` (and therefore the
//! tree cell) always visible.

use quadraui::compose::tree_nav::{HorizontalDir, HorizontalNav, TreeNavCore};
use quadraui::{
    Backend, BackendCaps, Color, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Icon,
    InteractionState, Key, NamedKey, Reaction, Rect, SortDirection, StatusBar, StatusBarSegment,
    Style, StyledText, TreeTable, TreeTableEvent, TreeTableHit, TreeTableLayout, TreeTableNode,
    UiEvent, WidgetId,
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

/// One file/directory in the demo's static hierarchy. Directories own
/// their own `expanded` flag — the app, not the primitive, decides which
/// children are currently flattened into `TreeTable::tree`.
struct FileNode {
    name: &'static str,
    date: &'static str,
    size: &'static str,
    kind: &'static str,
    is_dir: bool,
    expanded: bool,
    children: Vec<FileNode>,
}

impl FileNode {
    fn dir(
        name: &'static str,
        date: &'static str,
        expanded: bool,
        children: Vec<FileNode>,
    ) -> Self {
        Self {
            name,
            date,
            size: "--",
            kind: "Folder",
            is_dir: true,
            expanded,
            children,
        }
    }

    fn file(
        name: &'static str,
        date: &'static str,
        size: &'static str,
        kind: &'static str,
    ) -> Self {
        Self {
            name,
            date,
            size,
            kind,
            is_dir: false,
            expanded: false,
            children: Vec::new(),
        }
    }
}

fn demo_tree() -> Vec<FileNode> {
    vec![
        FileNode::dir(
            "src",
            "2026-02-01",
            true,
            vec![
                FileNode::file("main.rs", "2026-02-01", "1.8 KB", "Rust"),
                FileNode::file("lib.rs", "2026-01-28", "4.1 KB", "Rust"),
                FileNode::dir(
                    "util",
                    "2026-01-20",
                    false,
                    vec![FileNode::file("helpers.rs", "2026-01-20", "0.9 KB", "Rust")],
                ),
            ],
        ),
        FileNode::dir(
            "docs",
            "2025-12-15",
            false,
            vec![FileNode::file(
                "README.md",
                "2025-12-15",
                "2.2 KB",
                "Markdown",
            )],
        ),
        FileNode::file("Cargo.toml", "2026-01-28", "0.5 KB", "TOML"),
    ]
}

/// Recursively flatten `nodes` (depth-first, children immediately after
/// their parent — the order [`TreeNavCore::horizontal_nav`]'s
/// first-child lookup requires) into parallel `DataRow`/`TreeTableNode`
/// pairs, descending into a directory only when it's `expanded`.
fn flatten(
    nodes: &[FileNode],
    indent: u16,
    prefix: &[u16],
    out: &mut Vec<(DataRow, TreeTableNode)>,
) {
    for (i, node) in nodes.iter().enumerate() {
        let mut path = prefix.to_vec();
        path.push(i as u16);

        let mut tnode = TreeTableNode::new(path.clone(), indent);
        if node.is_dir {
            tnode = tnode.with_expanded(node.expanded);
            let name = if node.expanded {
                "folder-opened"
            } else {
                "folder"
            };
            tnode =
                tnode.with_icon(Icon::codicon(name, "DIR").unwrap_or_else(|| Icon::new("", "DIR")));
        } else {
            tnode = tnode.with_icon(Icon::codicon("file", "").unwrap_or_else(|| Icon::new("", "")));
        }

        out.push((
            DataRow {
                cells: vec![
                    StyledText::plain(node.name),
                    StyledText::plain(node.date),
                    StyledText::plain(node.size),
                    StyledText::plain(node.kind),
                ],
                decoration: Default::default(),
            },
            tnode,
        ));

        if node.is_dir && node.expanded {
            flatten(&node.children, indent + 1, &path, out);
        }
    }
}

/// Find the node at `path` (a sequence of child indices from the root),
/// for toggling its `expanded` flag.
fn node_at_path_mut<'a>(nodes: &'a mut [FileNode], path: &[u16]) -> Option<&'a mut FileNode> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get_mut(*first as usize)?;
    if rest.is_empty() {
        Some(node)
    } else {
        node_at_path_mut(&mut node.children, rest)
    }
}

/// All four columns, narrowest-rect-safe: `Kind`, then `Size`, then
/// `Date Modified` drop from the right as `width` shrinks, keeping
/// `Name` — and therefore the tree cell itself — always visible. A
/// dropped column's cells are simply never reached by the painter; rows
/// still carry all four (see [`flatten`]).
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
    roots: Vec<FileNode>,
    nav: TreeNavCore,
    sort_col: Option<usize>,
    sort_asc: bool,
    last: String,
}

impl TreeTableDemo {
    pub fn new() -> Self {
        let mut nav = TreeNavCore::new();
        nav.set_selected_path(Some(vec![0]));
        Self {
            roots: demo_tree(),
            nav,
            sort_col: None,
            sort_asc: true,
            last: "—".into(),
        }
    }

    /// Flatten the current tree and build the `TreeTable` for a table
    /// area `width` wide — the one place column-dropping and row
    /// flattening happen, shared by `render` and `handle`.
    fn table(&self, width: f32) -> TreeTable {
        let mut out = Vec::new();
        flatten(&self.roots, 0, &[], &mut out);
        let (rows, tree): (Vec<DataRow>, Vec<TreeTableNode>) = out.into_iter().unzip();
        TreeTable {
            table: DataTable {
                id: WidgetId::new("gallery:tree-table:files"),
                columns: columns_for(width),
                rows,
                selected_idx: self.nav.selected_row_index(&tree),
                scroll_offset: self.nav.scroll_offset(),
                sort: self.sort_col.map(|c| {
                    (
                        c,
                        if self.sort_asc {
                            SortDirection::Ascending
                        } else {
                            SortDirection::Descending
                        },
                    )
                }),
                has_focus: true,
                show_scrollbar: true,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
                bordered: false,
            },
            tree,
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

    /// Apply a [`HorizontalNav`] classification: collapse/expand go
    /// through [`TreeTableEvent::from_horizontal_nav`] and
    /// [`Self::apply_event`] — the same path a chevron click's
    /// [`TreeTableLayout::chevron_toggle`] event takes — select-parent/
    /// select-first-child just move `nav`'s selection, exactly the rule
    /// [`TreeNavCore::horizontal_nav`] documents, applied rather than
    /// re-derived.
    fn apply_horizontal(&mut self, tree: &[TreeTableNode], dir: HorizontalDir) {
        match self.nav.horizontal_nav(tree, dir) {
            nav @ (HorizontalNav::Collapse(_) | HorizontalNav::Expand(_)) => {
                if let Some(event) = TreeTableEvent::from_horizontal_nav(nav) {
                    self.apply_event(event);
                }
            }
            HorizontalNav::SelectParent(path) | HorizontalNav::SelectFirstChild(path) => {
                self.nav.set_selected_path(Some(path));
            }
            HorizontalNav::None => {}
        }
    }

    fn set_expanded(&mut self, path: &[u16], expanded: bool) {
        if let Some(node) = node_at_path_mut(&mut self.roots, path) {
            node.expanded = expanded;
        }
    }

    /// Apply a [`TreeTableEvent`] the demo received from either
    /// [`TreeTableEvent::from_horizontal_nav`] (Left/Right) or
    /// [`TreeTableLayout::chevron_toggle`] (chevron click) — the one
    /// place `roots` is mutated for an expand/collapse, so both paths
    /// stay in sync.
    fn apply_event(&mut self, event: TreeTableEvent) {
        if let TreeTableEvent::ToggleExpanded { path, expand } = event {
            self.set_expanded(&path, expand);
            self.last = format!("{path:?} {}", if expand { "expanded" } else { "collapsed" });
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
        let table = self.table(table_rect.width);
        let _ = backend.draw_tree_table(table_rect, &table, None);

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
        let table = self.table(table_rect.width);
        match event {
            UiEvent::KeyPressed { key, .. } => {
                match key {
                    Key::Named(NamedKey::Down) | Key::Char('j') => {
                        let layout = backend.tree_table_layout(table_rect, &table);
                        self.nav
                            .move_selection_by(&table.tree, 1, layout.table.visible_rows);
                    }
                    Key::Named(NamedKey::Up) | Key::Char('k') => {
                        let layout = backend.tree_table_layout(table_rect, &table);
                        self.nav
                            .move_selection_by(&table.tree, -1, layout.table.visible_rows);
                    }
                    Key::Named(NamedKey::Left) | Key::Char('h') => {
                        self.apply_horizontal(&table.tree, HorizontalDir::Left);
                    }
                    Key::Named(NamedKey::Right) | Key::Char('l') => {
                        self.apply_horizontal(&table.tree, HorizontalDir::Right);
                    }
                    Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                        if let Some(path) = self.nav.activate() {
                            self.last = format!("{path:?} activated");
                        }
                    }
                    _ => return Reaction::Continue,
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let (lx, ly) = (position.x - table_rect.x, position.y - table_rect.y);
                let layout = backend.tree_table_layout(table_rect, &table);
                match layout.hit_test(lx, ly, self.nav.scroll_offset(), table.table.rows.len()) {
                    TreeTableHit::Header { col } => {
                        if self.sort_col == Some(col) {
                            self.sort_asc = !self.sort_asc;
                        } else {
                            self.sort_col = Some(col);
                            self.sort_asc = true;
                        }
                        self.last = format!("header {col} clicked");
                        Reaction::Redraw
                    }
                    TreeTableHit::Chevron { idx } => {
                        if let Some(event) = TreeTableLayout::chevron_toggle(idx, &table.tree) {
                            self.apply_event(event);
                        }
                        Reaction::Redraw
                    }
                    TreeTableHit::Row { idx } => {
                        self.nav
                            .set_selected_path(Some(table.tree[idx].path.clone()));
                        Reaction::Redraw
                    }
                    TreeTableHit::HeaderDivider { .. }
                    | TreeTableHit::Footer
                    | TreeTableHit::Empty => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::to_value(self.table(80.0)).unwrap_or(serde_json::Value::Null)
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
    fn new_selects_the_first_row_and_starts_unsorted() {
        let demo = TreeTableDemo::new();
        assert_eq!(demo.nav.selected_path(), Some(&vec![0]));
        assert_eq!(demo.sort_col, None);
    }

    #[test]
    fn src_starts_expanded_and_docs_starts_collapsed() {
        let demo = TreeTableDemo::new();
        let table = demo.table(80.0);
        assert!(table
            .tree
            .iter()
            .any(|n| n.path == vec![0, 0] || n.path == vec![0, 1]));
        assert!(!table.tree.iter().any(|n| n.path == vec![1, 0]));
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
    fn chevron_toggle_emits_toggle_expanded_and_apply_event_flips_a_directorys_state() {
        let mut demo = TreeTableDemo::new();
        let tree = demo.table(80.0).tree;
        let event = TreeTableLayout::chevron_toggle(0, &tree);
        assert_eq!(
            event,
            Some(TreeTableEvent::ToggleExpanded {
                path: vec![0],
                expand: false,
            })
        );
        demo.apply_event(event.unwrap());
        assert!(!demo.roots[0].expanded, "src/ should collapse");

        let tree = demo.table(80.0).tree;
        let event = TreeTableLayout::chevron_toggle(0, &tree).unwrap();
        demo.apply_event(event);
        assert!(demo.roots[0].expanded, "src/ should re-expand");
    }

    #[test]
    fn left_on_the_expanded_src_row_collapses_it() {
        let mut demo = TreeTableDemo::new();
        let tree = demo.table(80.0).tree;
        demo.apply_horizontal(&tree, HorizontalDir::Left);
        assert!(!demo.roots[0].expanded);
    }

    #[test]
    fn right_on_the_collapsed_docs_row_expands_it_without_moving_selection() {
        let mut demo = TreeTableDemo::new();
        demo.nav.set_selected_path(Some(vec![1]));
        let tree = demo.table(80.0).tree;
        demo.apply_horizontal(&tree, HorizontalDir::Right);
        assert!(demo.roots[1].expanded);
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
        let table = demo.table(table_rect.width);
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
            !demo.roots[0].expanded,
            "clicking the chevron should collapse src/"
        );
    }

    #[test]
    fn clicking_a_header_sorts_by_it() {
        let mut demo = TreeTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let table_rect = TreeTableDemo::table_rect(area, &backend);
        let table = demo.table(table_rect.width);
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
        assert_eq!(demo.sort_col, Some(1));
    }

    #[test]
    fn left_right_up_down_follow_the_shared_navigation_rule() {
        let mut demo = TreeTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);

        // Down from `src/` (path [0]) lands on its first child.
        let down = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &down, &mut backend, area);
        assert_eq!(demo.nav.selected_path(), Some(&vec![0, 0]));

        // Left on a leaf row selects its parent.
        let left = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Left),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &left, &mut backend, area);
        assert_eq!(demo.nav.selected_path(), Some(&vec![0]));

        // Left again on the now-selected, still-expanded `src/` collapses it
        // (selection itself doesn't move).
        demo.handle(0, &left, &mut backend, area);
        assert!(!demo.roots[0].expanded);
        assert_eq!(demo.nav.selected_path(), Some(&vec![0]));

        // Right re-expands it.
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &right, &mut backend, area);
        assert!(demo.roots[0].expanded);
    }

    #[test]
    fn narrow_rect_drops_trailing_columns_but_keeps_the_tree_column() {
        let demo = TreeTableDemo::new();
        let backend = quadraui::tui::TuiBackend::new();
        let wide = TreeTableDemo::table_rect(Rect::new(0.0, 0.0, 80.0, 20.0), &backend);
        let narrow = TreeTableDemo::table_rect(Rect::new(0.0, 0.0, 20.0, 20.0), &backend);
        assert_eq!(demo.table(wide.width).table.columns.len(), 4);
        let narrow_table = demo.table(narrow.width);
        assert!(narrow_table.table.columns.len() < 4);
        assert_eq!(narrow_table.table.columns[0].title, "Name");
    }
}
