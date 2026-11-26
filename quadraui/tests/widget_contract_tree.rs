#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box tests for `TreeView` on the widget contract: the tree is
//! painted through `DrawsPrimitives::draw_dyn`, registered in the app's
//! `FrameTree`, and driven through the real `UiEvent → preprocess_event →
//! Interactive::handle` routing engine. Every `TreeEvent` comes back out
//! as a `UiEvent::Tree` this test's `AppLogic::handle` applies onto the
//! owned descriptor (controlled mode), and the next render paints it.
//!
//! The TUI leg covers keyboard navigation, Left/Right expand-collapse,
//! typeahead, chevron and row clicks, the wheel, the reserved scrollbar
//! column and the inline rename session. The GTK and macOS legs drive the
//! same app on a pixel surface, where the scrollbar overlays the rows.

use quadraui::{
    AppLogic, Backend, DrawsPrimitives, FrameTree, Reaction, Rect, SelectionMode, StyledText,
    TreeEvent, TreePath, TreeRow, TreeRowEditState, TreeStyle, TreeView, UiEvent, WidgetId,
};

/// One node of the app's own tree model.
struct Node {
    label: &'static str,
    children: Vec<Node>,
}

fn leaf(label: &'static str) -> Node {
    Node {
        label,
        children: Vec::new(),
    }
}

fn branch(label: &'static str, children: Vec<Node>) -> Node {
    Node { label, children }
}

/// The app that owns a tree model, flattens it into a `TreeView` every
/// frame and applies each `TreeEvent` back onto its own state.
struct TreeApp {
    id: WidgetId,
    rect: Rect,
    nodes: Vec<Node>,
    expanded: Vec<TreePath>,
    vim_keys: bool,
    selected: Option<TreePath>,
    scroll_offset: usize,
    editing: Option<(TreePath, TreeRowEditState)>,
    events: Vec<TreeEvent>,
    renamed: Vec<(TreePath, String)>,
}

impl TreeApp {
    fn new(rect: Rect) -> Self {
        Self {
            id: WidgetId::new("tree:explorer"),
            rect,
            nodes: vec![
                branch("src", vec![leaf("main.rs"), leaf("lib.rs")]),
                leaf("alpha"),
                leaf("beta"),
                leaf("gamma"),
                leaf("delta"),
            ],
            expanded: Vec::new(),
            vim_keys: false,
            selected: None,
            scroll_offset: 0,
            editing: None,
            events: Vec::new(),
            renamed: Vec::new(),
        }
    }

    /// Only the top-level leaves `labels`.
    fn with_leaves(rect: Rect, labels: &[&'static str]) -> Self {
        let mut app = Self::new(rect);
        app.nodes = labels.iter().map(|l| leaf(l)).collect();
        app
    }

    /// Many top-level leaves, so the rows overflow a short viewport.
    fn long(rect: Rect) -> Self {
        const LABELS: [&str; 20] = [
            "row00", "row01", "row02", "row03", "row04", "row05", "row06", "row07", "row08",
            "row09", "row10", "row11", "row12", "row13", "row14", "row15", "row16", "row17",
            "row18", "row19",
        ];
        Self::with_leaves(rect, &LABELS)
    }

    fn flatten(&self) -> Vec<TreeRow> {
        fn walk(
            nodes: &[Node],
            prefix: &TreePath,
            expanded: &[TreePath],
            editing: &Option<(TreePath, TreeRowEditState)>,
            out: &mut Vec<TreeRow>,
        ) {
            for (i, node) in nodes.iter().enumerate() {
                let mut path = prefix.clone();
                path.push(i as u16);
                let is_expanded = if node.children.is_empty() {
                    None
                } else {
                    Some(expanded.contains(&path))
                };
                out.push(TreeRow {
                    path: path.clone(),
                    indent: prefix.len() as u16,
                    icon: None,
                    text: StyledText::plain(node.label),
                    badge: None,
                    is_expanded,
                    decoration: Default::default(),
                    edit: editing
                        .as_ref()
                        .filter(|(p, _)| *p == path)
                        .map(|(_, e)| e.clone()),
                });
                if is_expanded == Some(true) {
                    walk(&node.children, &path, expanded, editing, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &self.nodes,
            &Vec::new(),
            &self.expanded,
            &self.editing,
            &mut out,
        );
        out
    }

    fn tree(&self) -> TreeView {
        TreeView {
            id: self.id,
            rows: self.flatten(),
            selection_mode: SelectionMode::Single,
            selected_path: self.selected.clone(),
            scroll_offset: self.scroll_offset,
            style: TreeStyle {
                vim_keys: self.vim_keys,
                ..TreeStyle::default()
            },
            has_focus: true,
        }
    }

    fn label_of(&self, path: &TreePath) -> Option<String> {
        self.flatten()
            .into_iter()
            .find(|r| &r.path == path)
            .map(|r| r.text.spans.iter().map(|s| s.text.as_str()).collect())
    }

    fn selected_label(&self) -> Option<String> {
        self.selected.as_ref().and_then(|p| self.label_of(p))
    }

    fn toggle(&mut self, path: &TreePath) {
        if let Some(i) = self.expanded.iter().position(|p| p == path) {
            self.expanded.remove(i);
        } else {
            self.expanded.push(path.clone());
        }
    }
}

impl AppLogic for TreeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(self.rect, &self.tree(), &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, self.rect, Box::new(self.tree()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::Tree(id, tree_event) = event else {
            return Reaction::Continue;
        };
        if id != self.id {
            return Reaction::Continue;
        }
        match &tree_event {
            TreeEvent::RowClicked { path, .. } | TreeEvent::SelectionChanged { path } => {
                self.selected = Some(path.clone());
            }
            TreeEvent::RowToggleExpand { path } => self.toggle(path),
            TreeEvent::ScrollChanged { scroll_offset } => self.scroll_offset = *scroll_offset,
            TreeEvent::RowRenameChanged {
                path,
                text,
                cursor,
                selection_anchor,
            } => {
                if let Some((p, e)) = &mut self.editing {
                    if p == path {
                        e.text = text.clone();
                        e.cursor = *cursor;
                        e.selection_anchor = *selection_anchor;
                    }
                }
            }
            TreeEvent::RowRenameCommitted { path, text } => {
                self.renamed.push((path.clone(), text.clone()));
                self.editing = None;
            }
            TreeEvent::RowRenameCancelled { .. } => self.editing = None,
            _ => {}
        }
        self.events.push(tree_event);
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{Modifiers, MouseButton, NamedKey, Point, ScrollDelta};

    const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 8.0);

    fn driver() -> TuiDriver<TreeApp> {
        TuiDriver::new(TreeApp::new(RECT), 40, 10)
    }

    /// Click a row by its label, which also gives the tree focus.
    fn click_label(driver: &mut TuiDriver<TreeApp>, label: &str) {
        let (x, y) = driver
            .find(label)
            .unwrap_or_else(|| panic!("{label} visible"));
        driver.click(x, y);
    }

    #[test]
    fn renders_every_top_level_row() {
        let driver = driver();
        for label in ["src", "alpha", "beta", "gamma", "delta"] {
            assert!(driver.screen_contains(label), "{label} painted");
        }
        assert!(
            !driver.screen_contains("main.rs"),
            "a collapsed branch hides its children"
        );
    }

    #[test]
    fn clicking_a_row_selects_it() {
        let mut driver = driver();
        click_label(&mut driver, "beta");
        assert_eq!(driver.app().selected_label().as_deref(), Some("beta"));
        assert!(matches!(
            driver.app().events.last(),
            Some(TreeEvent::RowClicked { .. })
        ));
    }

    #[test]
    fn arrow_and_vim_keys_move_the_selection() {
        let mut driver = driver();
        driver.app_mut().vim_keys = true;
        click_label(&mut driver, "alpha");
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().selected_label().as_deref(), Some("beta"));
        driver.type_char('j');
        assert_eq!(driver.app().selected_label().as_deref(), Some("gamma"));
        driver.type_char('k');
        assert_eq!(driver.app().selected_label().as_deref(), Some("beta"));
        driver.press_named(NamedKey::Up);
        assert_eq!(driver.app().selected_label().as_deref(), Some("alpha"));
        driver.press_named(NamedKey::End);
        assert_eq!(driver.app().selected_label().as_deref(), Some("delta"));
        driver.press_named(NamedKey::Home);
        assert_eq!(driver.app().selected_label().as_deref(), Some("src"));
    }

    #[test]
    fn enter_activates_the_selected_row() {
        let mut driver = driver();
        click_label(&mut driver, "gamma");
        driver.press_named(NamedKey::Enter);
        let path = driver.app().selected.clone().expect("gamma selected");
        assert_eq!(
            driver.app().events.last(),
            Some(&TreeEvent::RowActivated { path })
        );
    }

    #[test]
    fn right_expands_then_enters_and_left_returns_to_the_parent() {
        let mut driver = driver();
        click_label(&mut driver, "src");

        driver.press_named(NamedKey::Right);
        assert!(driver.screen_contains("main.rs"), "Right expands src");

        driver.press_named(NamedKey::Right);
        assert_eq!(
            driver.app().selected_label().as_deref(),
            Some("main.rs"),
            "Right on an expanded branch selects its first child"
        );

        driver.press_named(NamedKey::Left);
        assert_eq!(
            driver.app().selected_label().as_deref(),
            Some("src"),
            "Left on a leaf selects its parent"
        );

        driver.press_named(NamedKey::Left);
        assert!(!driver.screen_contains("main.rs"), "Left collapses src");
    }

    #[test]
    fn space_toggles_the_selected_branch() {
        let mut driver = driver();
        click_label(&mut driver, "src");
        driver.type_char(' ');
        assert!(driver.screen_contains("lib.rs"));
        driver.type_char(' ');
        assert!(!driver.screen_contains("lib.rs"));
    }

    #[test]
    fn clicking_the_chevron_toggles_without_selecting() {
        let mut driver = driver();
        let chevron = TreeStyle::default().chevron_collapsed;
        let (x, y) = driver.find(&chevron).expect("collapsed chevron painted");
        driver.click(x, y);
        assert!(driver.screen_contains("main.rs"), "chevron click expands");
        assert!(matches!(
            driver.app().events.last(),
            Some(TreeEvent::RowToggleExpand { .. })
        ));
        assert_eq!(driver.app().selected, None);
    }

    #[test]
    fn typeahead_jumps_to_the_first_matching_row() {
        let mut driver = driver();
        click_label(&mut driver, "src");
        driver.type_char('g');
        assert_eq!(driver.app().selected_label().as_deref(), Some("gamma"));
    }

    #[test]
    fn typeahead_accumulates_a_prefix_within_the_debounce_window() {
        let mut driver = driver();
        click_label(&mut driver, "src");
        driver.type_char('d');
        assert_eq!(driver.app().selected_label().as_deref(), Some("delta"));
        driver.type_char('e');
        assert_eq!(
            driver.app().selected_label().as_deref(),
            Some("delta"),
            "\"de\" still matches delta"
        );
    }

    const JK_LABELS: [&str; 4] = ["back", "bake", "jar", "kube"];

    #[test]
    fn typeahead_reaches_labels_with_j_and_k_when_vim_keys_are_off() {
        let mut driver = TuiDriver::new(TreeApp::with_leaves(RECT, &JK_LABELS), 40, 10);
        click_label(&mut driver, "back");
        driver.type_char('k');
        assert_eq!(driver.app().selected_label().as_deref(), Some("kube"));
        driver.type_char('u');
        assert_eq!(driver.app().selected_label().as_deref(), Some("kube"));
    }

    #[test]
    fn typeahead_accumulates_a_k_inside_a_prefix() {
        let mut driver = TuiDriver::new(TreeApp::with_leaves(RECT, &JK_LABELS), 40, 10);
        click_label(&mut driver, "jar");
        for c in "bak".chars() {
            driver.type_char(c);
        }
        assert_eq!(driver.app().selected_label().as_deref(), Some("bake"));
    }

    #[test]
    fn right_click_requests_a_context_menu_for_the_row() {
        let mut driver = driver();
        let (x, y) = driver.find("beta").expect("beta visible");
        driver.right_click(x, y);
        match driver.app().events.last() {
            Some(TreeEvent::ContextMenuRequested { path, position }) => {
                assert_eq!(driver.app().label_of(path).as_deref(), Some("beta"));
                assert_eq!(*position, Point::new(x, y));
            }
            other => panic!("expected ContextMenuRequested, got {other:?}"),
        }
    }

    #[test]
    fn overflowing_rows_reserve_and_paint_a_scrollbar_column() {
        let driver = TuiDriver::new(TreeApp::long(RECT), 40, 10);
        let col = RECT.width as u16 - 1;
        let glyphs: String = (0..RECT.height as u16)
            .map(|y| {
                driver
                    .styled_row(y)
                    .get(col as usize)
                    .map(|(c, _)| *c)
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            glyphs.contains('█'),
            "the last column carries the scrollbar thumb, got {glyphs:?}"
        );
    }

    #[test]
    fn wheel_scrolls_the_viewport() {
        let mut driver = TuiDriver::new(TreeApp::long(RECT), 40, 10);
        assert!(driver.screen_contains("row00"));
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 0.0, y: -3.0 },
            position: Point::new(5.0, 2.0),
        });
        assert_eq!(driver.app().scroll_offset, 3);
        assert!(!driver.screen_contains("row00"));
        assert!(driver.screen_contains("row03"));
    }

    #[test]
    fn keyboard_moves_scroll_the_selection_into_view() {
        let mut driver = TuiDriver::new(TreeApp::long(RECT), 40, 10);
        click_label(&mut driver, "row00");
        driver.press_named(NamedKey::End);
        assert_eq!(driver.app().selected_label().as_deref(), Some("row19"));
        assert!(
            driver.screen_contains("row19"),
            "End scrolls the last row into view"
        );
    }

    #[test]
    fn rename_session_edits_through_the_shared_editor() {
        let mut driver = driver();
        click_label(&mut driver, "beta");
        let path = driver.app().selected.clone().expect("beta selected");
        driver.app_mut().editing = Some((
            path.clone(),
            TreeRowEditState {
                text: "beta".into(),
                cursor: 4,
                selection_anchor: None,
                placeholder: None,
            },
        ));
        driver.render();

        driver.type_char('2');
        assert_eq!(
            driver.app().editing.as_ref().map(|(_, e)| e.text.as_str()),
            Some("beta2")
        );
        driver.press_named(NamedKey::Backspace);
        driver.press_named(NamedKey::Backspace);
        assert_eq!(
            driver.app().editing.as_ref().map(|(_, e)| e.text.as_str()),
            Some("bet")
        );
        driver.press_named(NamedKey::Down);
        assert_eq!(
            driver.app().selected_label().as_deref(),
            Some("beta"),
            "arrow keys stay inside the rename session"
        );
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().renamed, vec![(path, "bet".to_string())]);
        assert!(driver.app().editing.is_none());
    }

    #[test]
    fn escape_cancels_a_rename() {
        let mut driver = driver();
        click_label(&mut driver, "alpha");
        let path = driver.app().selected.clone().expect("alpha selected");
        driver.app_mut().editing = Some((
            path.clone(),
            TreeRowEditState {
                text: "alpha".into(),
                cursor: 5,
                selection_anchor: Some(0),
                placeholder: None,
            },
        ));
        driver.render();
        driver.type_char('x');
        driver.press_named(NamedKey::Escape);
        assert_eq!(
            driver.app().events.last(),
            Some(&TreeEvent::RowRenameCancelled { path })
        );
        assert!(driver.app().renamed.is_empty());
    }

    #[test]
    fn modified_click_in_multi_mode_reports_the_selection_set() {
        struct Multi(TreeApp);
        impl AppLogic for Multi {
            type AreaId = ();
            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let mut tree = self.0.tree();
                tree.selection_mode = SelectionMode::Multi;
                backend.draw_dyn(self.0.rect, &tree, &());
            }
            fn frame_tree(&self, _area: ()) -> FrameTree {
                let mut tree = self.0.tree();
                tree.selection_mode = SelectionMode::Multi;
                let mut frame = FrameTree::new();
                frame.push(self.0.id, self.0.rect, Box::new(tree));
                frame
            }
            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.0.handle(event, backend)
            }
        }
        let mut driver = TuiDriver::new(Multi(TreeApp::new(RECT)), 40, 10);
        let (x, y) = driver.find("alpha").expect("alpha visible");
        driver.click(x, y);
        let (x, y) = driver.find("gamma").expect("gamma visible");
        driver.click_with(
            MouseButton::Left,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            x,
            y,
        );
        match driver.app().0.events.last() {
            Some(TreeEvent::SelectionSet { paths }) => {
                let labels: Vec<_> = paths
                    .iter()
                    .filter_map(|p| driver.app().0.label_of(p))
                    .collect();
                assert_eq!(labels, vec!["alpha", "gamma"]);
            }
            other => panic!("expected SelectionSet, got {other:?}"),
        }
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;
    use quadraui::NamedKey;

    const RECT: Rect = Rect::new(0.0, 0.0, 300.0, 240.0);

    #[test]
    fn click_keys_and_chevron_route_through_the_painted_layout_on_gtk() {
        let mut driver = GtkDriver::new(TreeApp::new(RECT), 320, 260);
        let (x, y) = driver.find("beta").expect("beta painted");
        driver.click(x, y);
        assert_eq!(driver.app().selected_label().as_deref(), Some("beta"));

        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().selected_label().as_deref(), Some("gamma"));

        driver.press_named(NamedKey::Home);
        driver.press_named(NamedKey::Right);
        assert!(driver.screen_contains("main.rs"), "Right expands src");

        driver.type_char('d');
        assert_eq!(driver.app().selected_label().as_deref(), Some("delta"));
    }

    #[test]
    fn dragging_the_overlay_thumb_scrolls_on_gtk() {
        let rect = Rect::new(0.0, 0.0, 300.0, 120.0);
        let mut driver = GtkDriver::new(TreeApp::long(rect), 320, 140);
        assert!(driver.screen_contains("row00"));
        let x = rect.width - 3.0;
        driver.drag(x, 3.0, x, 110.0);
        assert!(
            driver.app().scroll_offset > 0,
            "a thumb drag reports a new scroll offset"
        );
        assert!(!driver.screen_contains("row00"));
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;
    use quadraui::NamedKey;

    const RECT: Rect = Rect::new(0.0, 0.0, 300.0, 240.0);

    #[test]
    fn click_keys_and_chevron_route_through_the_painted_layout_on_macos() {
        let mut driver = MacDriver::new(TreeApp::new(RECT), 320, 260);
        let (x, y) = driver.find("beta").expect("beta painted");
        driver.click(x, y);
        assert_eq!(driver.app().selected_label().as_deref(), Some("beta"));

        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().selected_label().as_deref(), Some("gamma"));

        driver.press_named(NamedKey::Home);
        driver.press_named(NamedKey::Right);
        assert!(driver.screen_contains("main.rs"), "Right expands src");

        driver.type_char('d');
        assert_eq!(driver.app().selected_label().as_deref(), Some("delta"));
    }

    #[test]
    fn dragging_the_overlay_thumb_scrolls_on_macos() {
        let rect = Rect::new(0.0, 0.0, 300.0, 120.0);
        let mut driver = MacDriver::new(TreeApp::long(rect), 320, 140);
        assert!(driver.screen_contains("row00"));
        let x = rect.width - 3.0;
        driver.drag(x, 3.0, x, 110.0);
        assert!(
            driver.app().scroll_offset > 0,
            "a thumb drag reports a new scroll offset"
        );
        assert!(!driver.screen_contains("row00"));
    }
}
