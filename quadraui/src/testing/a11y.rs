//! Headless accessibility-tree test seam.
//!
//! [`A11yScenario`] is the black-box harness for accessibility work:
//! describe a screen as a list of [`Surface`]s plus a focused
//! [`WidgetId`], then ask for an [`A11ySnapshot`] — a stable text
//! rendering of the resulting accessibility tree (`role "label"
//! [state]`, one line per node, two spaces of indent per level) with
//! the live [`A11yTree`] alongside it for structural assertions.
//!
//! Tests assert through this seam rather than against a platform
//! accessibility API, for the same reason the rest of the harness is
//! backend-agnostic: a tree built from an owned frame is identical on
//! every backend, so one assertion covers all of them. No backend
//! feature is required and no window, display, or screen reader is
//! involved — the scenario captures an [`OwnedFrame`] directly from a
//! [`ScreenLayout`], never painting anything.
//!
//! ```no_run
//! use quadraui::testing::a11y::A11yScenario;
//! use quadraui::WidgetId;
//! # fn my_surface() -> quadraui::frame::Surface { unimplemented!() }
//!
//! let snapshot = A11yScenario::new()
//!     .with_surface(my_surface())
//!     .with_focus(WidgetId::new("files"))
//!     .snapshot();
//!
//! assert!(snapshot.contains("list \"Files\""));
//! ```
//!
//! ## Snapshots are text, geometry is not
//!
//! [`A11ySnapshot::text`] deliberately carries no bounds: node
//! rectangles depend on the frame's layout and would make every
//! snapshot brittle. Assert on [`A11ySnapshot::tree`] when a test is
//! actually about geometry.
//!
//! ## Incremental updates
//!
//! [`A11yScenario::update`] drives the same incremental path a backend
//! would: the first call carries the full tree, later calls carry only
//! the nodes that changed. Mutate the scenario's surfaces through
//! [`A11yScenario::surfaces_mut`] (or move focus) between calls to test
//! what a frame-to-frame delta emits.

use std::fmt;

use crate::a11y::{A11yInfo, A11yRole, A11yTree, A11yTreeBuilder, A11yTreeNode, A11yUpdate};
use crate::focus::FocusManager;
use crate::frame::{OwnedFrame, ScreenLayout, Surface};
use crate::WidgetId;

/// A headless screen description that can be turned into an
/// accessibility tree.
///
/// Surfaces are pushed in the same back-to-front z-order a real frame
/// uses, so the tree's child order matches what a backend would paint.
#[derive(Default)]
pub struct A11yScenario {
    layout: ScreenLayout,
    overrides: Vec<(usize, A11yInfo)>,
    focus: FocusManager,
    builder: A11yTreeBuilder,
}

impl fmt::Debug for A11yScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Surface` is not `Debug` (it owns whole primitives, several of
        // which carry backend handles), so the surface list is reported
        // by count.
        f.debug_struct("A11yScenario")
            .field("surfaces", &self.layout.surfaces().len())
            .field("overrides", &self.overrides)
            .field("focus", &self.focus)
            .field("builder", &self.builder)
            .finish()
    }
}

impl A11yScenario {
    /// An empty scenario: no surfaces, nothing focused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Renames the root window node, as an application would.
    pub fn with_root_label(mut self, label: impl Into<String>) -> Self {
        self.builder = std::mem::take(&mut self.builder).with_root_label(label);
        self
    }

    /// Appends a surface, chainable.
    pub fn with_surface(mut self, surface: Surface) -> Self {
        self.push(surface);
        self
    }

    /// Focuses `id`, chainable. The id does not have to be in any tab
    /// order — the scenario sets focus directly, the way the runtime's
    /// event preprocessing would have left it.
    pub fn with_focus(mut self, id: WidgetId) -> Self {
        self.set_focus(Some(id));
        self
    }

    /// Attaches a consumer accessibility override to the surface at
    /// `idx`, chainable — the same per-node escape hatch an application
    /// uses to relabel a primitive it does not control. Fields set here
    /// win over the primitive's own; unset fields fall through.
    pub fn with_a11y_override(mut self, idx: usize, info: A11yInfo) -> Self {
        self.overrides.push((idx, info));
        self
    }

    /// Names the surface a backend's modal stack holds on top,
    /// chainable. That surface is marked modal and every other
    /// top-level surface is hidden from assistive technology; without
    /// it the topmost dialog surface in the frame is used.
    pub fn with_modal(mut self, id: WidgetId) -> Self {
        self.set_modal(Some(id));
        self
    }

    /// Appends a surface.
    pub fn push(&mut self, surface: Surface) {
        self.layout.push(surface);
    }

    /// Sets (or clears) the modal top between snapshots — the stateful
    /// form of [`Self::with_modal`].
    pub fn set_modal(&mut self, id: Option<WidgetId>) {
        self.builder.set_modal_top(id);
    }

    /// The pushed surfaces, mutably — replace one and re-snapshot to
    /// test what a frame-to-frame change emits.
    pub fn surfaces_mut(&mut self) -> &mut [Surface] {
        self.layout.surfaces_mut()
    }

    /// Sets (or clears) keyboard focus.
    pub fn set_focus(&mut self, id: Option<WidgetId>) {
        self.focus.set_focus(id);
    }

    /// The frame this scenario captures, with any
    /// [`Self::with_a11y_override`] applied.
    pub fn frame(&self) -> OwnedFrame {
        let mut frame = OwnedFrame::from_layout(&self.layout);
        for (idx, info) in &self.overrides {
            if let Some(node) = frame.nodes_mut().get_mut(*idx) {
                node.a11y = Some(info.clone());
            }
        }
        frame
    }

    /// Builds the tree and renders it as an assertable snapshot. Does
    /// not disturb [`Self::update`]'s incremental state.
    pub fn snapshot(&self) -> A11ySnapshot {
        A11ySnapshot::new(self.builder.tree(&self.frame(), &self.focus))
    }

    /// Builds the next incremental update: full the first time, then
    /// only the nodes that changed since the previous call.
    pub fn update(&mut self) -> A11yUpdate {
        let frame = self.frame();
        self.builder.update(&frame, &self.focus)
    }

    /// Forgets what [`Self::update`] has already emitted, so the next
    /// one is full again.
    pub fn reset(&mut self) {
        self.builder.reset();
    }
}

/// A rendered accessibility tree: the structural tree plus its stable
/// text form.
#[derive(Debug, Clone)]
pub struct A11ySnapshot {
    tree: A11yTree,
    text: String,
}

impl A11ySnapshot {
    fn new(tree: A11yTree) -> Self {
        let text = tree.to_snapshot();
        Self { tree, text }
    }

    /// The tree itself — for assertions about ids, bounds, or node
    /// relationships that the text form deliberately omits.
    pub fn tree(&self) -> &A11yTree {
        &self.tree
    }

    /// The snapshot text: one line per node, `role "label" [state]`,
    /// two spaces of indent per level, trailing newline.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the snapshot text contains `needle` anywhere — the
    /// loose assertion, for when a test cares about one node and not
    /// the whole screen.
    pub fn contains(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }

    /// The snapshot line describing the node whose accessible name is
    /// exactly `label`, indentation trimmed.
    pub fn line(&self, label: &str) -> Option<&str> {
        let quoted = format!("\"{label}\"");
        self.text
            .lines()
            .map(str::trim_start)
            .find(|line| line.contains(&quoted))
    }

    /// The first node whose accessible name is exactly `label`.
    pub fn find(&self, label: &str) -> Option<&A11yTreeNode> {
        self.tree.find(label)
    }

    /// Every node's role, in document order, root first.
    pub fn roles(&self) -> Vec<A11yRole> {
        self.tree.nodes().iter().map(A11yTreeNode::role).collect()
    }

    /// Every node's accessible name, in document order; unnamed nodes
    /// are skipped.
    pub fn labels(&self) -> Vec<&str> {
        self.tree
            .nodes()
            .iter()
            .filter_map(A11yTreeNode::label)
            .collect()
    }

    /// The node holding accessibility focus.
    pub fn focused(&self) -> Option<&A11yTreeNode> {
        self.tree.focused()
    }

    /// Asserts the whole snapshot equals `expected`, ignoring leading
    /// and trailing blank lines so a test can write the expectation as
    /// an indented raw string. Panics with both trees side by side.
    #[track_caller]
    pub fn assert_matches(&self, expected: &str) {
        let actual = self.text.trim_matches('\n');
        let expected = expected.trim_matches('\n');
        assert!(
            actual == expected,
            "accessibility tree did not match.\n--- expected ---\n{expected}\n--- actual ---\n{actual}\n"
        );
    }
}

impl fmt::Display for A11ySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::{frame_to_tree, A11yNodeId};
    use crate::primitives::list::{ListItem, ListView};
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::types::{Decoration, StyledText};
    use crate::Rect;

    fn list(id: &str, title: &str, items: &[&str], selected: usize) -> ListView {
        ListView {
            id: WidgetId::new(id),
            title: Some(StyledText::plain(title)),
            items: items
                .iter()
                .map(|text| ListItem {
                    text: StyledText::plain(*text),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: selected,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn list_surface(id: &str, title: &str, items: &[&str], selected: usize) -> Surface {
        Surface::List {
            rect: Rect::new(0.0, 1.0, 30.0, 11.0),
            list: list(id, title, items, selected),
        }
    }

    fn button(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn toolbar_surface() -> Surface {
        let bar = Toolbar::new(WidgetId::new("tools")).with_buttons(vec![
            button("run", "Run"),
            ToolbarButton::Separator,
            button("stop", "Stop"),
        ]);
        Surface::Toolbar {
            rect: Rect::new(0.0, 0.0, 30.0, 1.0),
            bar,
            hovered: None,
            pressed: None,
        }
    }

    #[test]
    fn scenario_snapshots_a_list_screen() {
        let snapshot = A11yScenario::new()
            .with_surface(toolbar_surface())
            .with_surface(list_surface("files", "Files", &["a.rs", "b.rs"], 1))
            .snapshot();

        snapshot.assert_matches(
            "\
window \"quadraui\" [focused]
  toolbar
    button \"Run\"
    presentation
    button \"Stop\"
  list \"Files\" [size=2]
    list_item \"a.rs\" [pos=1/2]
    list_item \"b.rs\" [selected pos=2/2]",
        );
    }

    #[test]
    fn snapshot_text_matches_the_tree_it_carries() {
        let scenario =
            A11yScenario::new().with_surface(list_surface("files", "Files", &["a.rs"], 0));
        let snapshot = scenario.snapshot();
        let direct = frame_to_tree(&scenario.frame(), &FocusManager::new());
        assert_eq!(snapshot.text(), direct.to_snapshot());
        assert_eq!(snapshot.to_string(), direct.to_snapshot());
    }

    #[test]
    fn focus_descends_into_the_focused_primitive() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", "Files", &["a.rs", "b.rs"], 1))
            .with_focus(WidgetId::new("files"))
            .snapshot();

        assert_eq!(
            snapshot.focused().and_then(A11yTreeNode::label),
            Some("b.rs")
        );
        assert_eq!(
            snapshot.line("b.rs"),
            Some("list_item \"b.rs\" [focused selected pos=2/2]")
        );
    }

    #[test]
    fn roles_and_labels_read_in_document_order() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", "Files", &["a.rs"], 0))
            .snapshot();

        assert_eq!(
            snapshot.roles(),
            vec![A11yRole::Window, A11yRole::List, A11yRole::ListItem]
        );
        assert_eq!(snapshot.labels(), vec!["quadraui", "Files", "a.rs"]);
        assert!(snapshot.contains("list \"Files\""));
        assert!(snapshot.find("Files").is_some());
        assert!(snapshot.find("nope").is_none());
    }

    #[test]
    fn node_ids_survive_a_re_snapshot() {
        let mut scenario =
            A11yScenario::new().with_surface(list_surface("files", "Files", &["a.rs", "b.rs"], 0));
        let first = scenario.snapshot();
        let before = first.find("a.rs").expect("row").id;

        scenario.surfaces_mut()[0] = list_surface("files", "Files", &["a.rs", "b.rs"], 1);
        let second = scenario.snapshot();

        assert_eq!(second.find("a.rs").expect("row").id, before);
        assert_eq!(
            second.tree().root(),
            first.tree().root(),
            "the root id is fixed"
        );
        assert_eq!(
            before,
            A11yNodeId::for_widget(&WidgetId::new("files")).child("item.0")
        );
    }

    #[test]
    fn first_update_is_full_then_incremental() {
        let mut scenario =
            A11yScenario::new().with_surface(list_surface("files", "Files", &["a.rs", "b.rs"], 0));

        let full = scenario.update();
        assert!(full.full);
        assert_eq!(full.nodes.len(), scenario.snapshot().tree().nodes().len());

        let unchanged = scenario.update();
        assert!(!unchanged.full);
        assert!(
            unchanged.nodes.is_empty(),
            "an identical frame emits nothing"
        );

        scenario.surfaces_mut()[0] = list_surface("files", "Files", &["a.rs", "b.rs"], 1);
        let changed = scenario.update();
        assert!(!changed.full);
        assert_eq!(changed.nodes.len(), 2, "only the two rows that moved");

        scenario.reset();
        assert!(scenario.update().full, "reset forces a full tree again");
    }

    #[test]
    fn consumer_override_relabels_a_primitive() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", "Files", &["a.rs"], 0))
            .with_a11y_override(0, A11yInfo::new().with_label("Open editors"))
            .snapshot();

        assert_eq!(
            snapshot.line("Open editors"),
            Some("list \"Open editors\" [size=1]")
        );
        assert!(snapshot.find("Files").is_none());
    }

    #[test]
    fn root_label_is_overridable() {
        let snapshot = A11yScenario::new()
            .with_root_label("kubeui")
            .with_surface(list_surface("files", "Files", &["a.rs"], 0))
            .snapshot();

        assert!(snapshot.contains("window \"kubeui\""));
    }

    #[test]
    #[should_panic(expected = "accessibility tree did not match")]
    fn assert_matches_reports_a_mismatch() {
        A11yScenario::new()
            .snapshot()
            .assert_matches("list \"nope\"");
    }
}
