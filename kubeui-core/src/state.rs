//! Pure application state — no rendering, no event types.
//!
//! [`crate::KubeApp`] owns a single [`AppState`] instance and mutates
//! it directly from `quadraui::AppLogic::handle`. Builders in
//! [`crate::view`] turn the state into `quadraui` primitives that
//! [`crate::KubeApp::render`] draws through the generic
//! `quadraui::Backend` trait, so the same code runs on every backend.

use quadraui::{
    fuzzy_score, DualModePaletteController, Key, Modifiers, NamedKey, PaletteItem, StyledSpan,
    StyledText, UiEvent,
};

use crate::k8s::ResourceItem;

/// Top-level app state. Cloning is cheap-ish (Vec<ResourceItem> can
/// be large but is rarely cloned outside tests). Mutation happens
/// through `quadraui::AppLogic::handle`; nothing in the rest of the
/// crate holds a long-lived borrow.
pub struct AppState {
    /// Kubeconfig context name (informational only — kube uses the
    /// default context until we add a switcher).
    pub context: String,
    /// Namespaces fetched from the cluster on startup. Falls back to
    /// `["default"]` if the list call fails so the UI is still usable.
    pub namespaces: Vec<String>,
    pub current_ns: usize,
    /// Resource kind currently displayed.
    pub kind: ResourceKind,
    pub resources: Vec<ResourceItem>,
    pub selected: usize,
    pub yaml_scroll: usize,
    /// Which pane consumes j/k. Tab cycles.
    pub focus: Focus,
    /// Modal namespace/kind picker. `Some` while open.
    pub picker: Option<PickerOverlay>,
    /// Last status / error message — surfaced in the bottom bar.
    pub status: String,
    /// Set from `quadraui::AppLogic::handle` when the user hits the
    /// quit action. The runner's event loop exits once this is true.
    pub should_quit: bool,
}

impl AppState {
    pub fn new(context: String, namespaces: Vec<String>) -> Self {
        let namespaces = if namespaces.is_empty() {
            vec!["default".to_string()]
        } else {
            namespaces
        };
        let current_ns = namespaces.iter().position(|n| n == "default").unwrap_or(0);
        Self {
            context,
            namespaces,
            current_ns,
            kind: ResourceKind::Pods,
            resources: Vec::new(),
            selected: 0,
            yaml_scroll: 0,
            focus: Focus::Resources,
            picker: None,
            status: "Press r to load.".to_string(),
            should_quit: false,
        }
    }

    pub fn current_namespace(&self) -> &str {
        &self.namespaces[self.current_ns]
    }

    pub fn yaml_for_selected(&self) -> &str {
        self.resources
            .get(self.selected)
            .map(|r| r.yaml.as_str())
            .unwrap_or("")
    }
}

/// Kubernetes resource types the dashboard knows how to list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceKind {
    Pods,
    Deployments,
    Services,
    ConfigMaps,
    Secrets,
}

impl ResourceKind {
    pub const ALL: &'static [ResourceKind] = &[
        ResourceKind::Pods,
        ResourceKind::Deployments,
        ResourceKind::Services,
        ResourceKind::ConfigMaps,
        ResourceKind::Secrets,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ResourceKind::Pods => "Pods",
            ResourceKind::Deployments => "Deployments",
            ResourceKind::Services => "Services",
            ResourceKind::ConfigMaps => "ConfigMaps",
            ResourceKind::Secrets => "Secrets",
        }
    }
}

/// Which pane consumes j/k input. Tab cycles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Resources,
    Yaml,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerPurpose {
    Namespace,
    ResourceKind,
}

/// Modal namespace/kind picker — a thin wrapper around
/// `quadraui::DualModePaletteController` that keeps the palette's
/// filtered item list in sync with [`fuzzy_score`] matches against
/// `source`.
///
/// `DualModePaletteController` filters nothing on its own (see its
/// module doc: the app refilters and calls `set_items` in response to
/// `QueryChanged`) — that refiltering is this type's whole job, so
/// `source`'s original index survives query changes via `filtered`
/// (`filtered[row]` is the index into `source` the controller's row
/// `row` currently shows).
pub struct PickerOverlay {
    pub purpose: PickerPurpose,
    /// Full, unfiltered candidate list (namespace names or resource
    /// kind labels) in original order.
    source: Vec<String>,
    /// Index into `source` of the item the app currently has active —
    /// marked with a `●` prefix so the user can see what they're
    /// changing away from.
    current_orig: Option<usize>,
    /// `filtered[row]` is the `source` index the controller's row
    /// `row` currently displays. Rebuilt by [`Self::refilter`].
    filtered: Vec<usize>,
    controller: DualModePaletteController,
}

/// Large enough that `DualModePaletteController::sync_scroll` never
/// clamps `scroll_top` while a `PickerOverlay` seeds its initial
/// selection — the real viewport-derived row count is only known at
/// render/handle time, not at construction.
const UNBOUNDED_ROWS: usize = usize::MAX;

impl PickerOverlay {
    pub fn new(
        purpose: PickerPurpose,
        title: impl Into<String>,
        source: Vec<String>,
        current_orig: Option<usize>,
    ) -> Self {
        let filtered: Vec<usize> = (0..source.len()).collect();
        let items = build_items(&source, &filtered, current_orig);
        let mut controller =
            DualModePaletteController::new(title, None, items).with_id("kubeui_picker");
        // Pre-select the currently active row so the user can press
        // Enter immediately to close without changing anything —
        // matches the old substring picker's pre-selected "current"
        // marker.
        if let Some(cur) = current_orig {
            if let Some(row) = filtered.iter().position(|&i| i == cur) {
                for _ in 0..row {
                    controller.handle(&down_key_event(), UNBOUNDED_ROWS);
                }
            }
        }
        Self {
            purpose,
            source,
            current_orig,
            filtered,
            controller,
        }
    }

    pub fn controller(&self) -> &DualModePaletteController {
        &self.controller
    }

    pub fn controller_mut(&mut self) -> &mut DualModePaletteController {
        &mut self.controller
    }

    /// Recompute the filtered item list from the controller's current
    /// query text via `fuzzy_score`, then push the result into the
    /// controller with `set_items`. Call after every
    /// `DualModePaletteEvent::QueryChanged`.
    ///
    /// An empty query shows every candidate, unranked, in original
    /// order — the fast path `fuzzy_score`'s own doc recommends for
    /// "nothing typed yet" (scoring would just report everything as a
    /// trivial match, so skip the sort instead of paying for one that
    /// changes nothing).
    pub fn refilter(&mut self) {
        let query = self.controller.query().to_string();
        let (filtered, positions): (Vec<usize>, Vec<Vec<usize>>) = if query.is_empty() {
            (
                (0..self.source.len()).collect(),
                vec![Vec::new(); self.source.len()],
            )
        } else {
            let q = query.to_lowercase();
            let mut scored: Vec<(i32, usize, Vec<usize>)> = self
                .source
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    fuzzy_score(&s.to_lowercase(), &q).map(|(score, pos)| (score, i, pos))
                })
                .collect();
            scored.sort_by_key(|&(score, _, _)| std::cmp::Reverse(score));
            let filtered = scored.iter().map(|&(_, i, _)| i).collect();
            let positions = scored.into_iter().map(|(_, _, p)| p).collect();
            (filtered, positions)
        };
        let items =
            build_items_with_positions(&self.source, &filtered, &positions, self.current_orig);
        self.filtered = filtered;
        self.controller.set_items(items);
    }

    /// Original `source` index the controller's currently highlighted
    /// row maps to, if any (empty filter result → `None`).
    pub fn selected_orig_index(&self) -> Option<usize> {
        self.filtered.get(self.controller.selected()).copied()
    }

    /// Map a palette row index (as carried by
    /// `DualModePaletteEvent::ItemConfirmed`) back to its `source`
    /// index.
    pub fn orig_index_of_row(&self, row: usize) -> Option<usize> {
        self.filtered.get(row).copied()
    }
}

fn down_key_event() -> UiEvent {
    UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Down),
        modifiers: Modifiers::default(),
        repeat: false,
    }
}

fn build_items(
    source: &[String],
    filtered: &[usize],
    current_orig: Option<usize>,
) -> Vec<PaletteItem> {
    let empty = vec![Vec::new(); filtered.len()];
    build_items_with_positions(source, filtered, &empty, current_orig)
}

/// Build the `PaletteItem` list for `filtered` rows. `positions[row]`
/// is forwarded to `PaletteItem::match_positions` so backends can
/// highlight why each row matched; the row mapping to `current_orig`
/// (if any) gets a `●` marker prefix.
fn build_items_with_positions(
    source: &[String],
    filtered: &[usize],
    positions: &[Vec<usize>],
    current_orig: Option<usize>,
) -> Vec<PaletteItem> {
    filtered
        .iter()
        .enumerate()
        .map(|(row, &orig)| {
            let is_current = current_orig == Some(orig);
            let prefix = if is_current { "● " } else { "  " };
            let text = format!("{prefix}{}", source[orig]);
            // Match positions are byte offsets into `source[orig]`;
            // shift them by the prefix's byte length so they still
            // land on the right characters in the prefixed text.
            let shift = prefix.len();
            let match_positions = positions
                .get(row)
                .map(|p| p.iter().map(|&b| b + shift).collect())
                .unwrap_or_default();
            PaletteItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain(text)],
                },
                detail: None,
                icon: None,
                match_positions,
                depth: 0,
                expandable: false,
                expanded: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespaces() -> Vec<String> {
        vec![
            "default".into(),
            "kube-system".into(),
            "kube-public".into(),
            "monitoring".into(),
        ]
    }

    #[test]
    fn fuzzy_query_matches_subsequence() {
        let mut picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(0));
        for ch in "kbs".chars() {
            picker.controller_mut().handle(
                &UiEvent::KeyPressed {
                    key: Key::Char(ch),
                    modifiers: Modifiers::default(),
                    repeat: false,
                },
                UNBOUNDED_ROWS,
            );
            picker.refilter();
        }
        let top = picker.selected_orig_index().expect("a match for 'kbs'");
        assert_eq!(picker.source[top], "kube-system");
    }

    #[test]
    fn empty_query_preserves_original_order() {
        let picker = PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), None);
        assert_eq!(picker.filtered, vec![0, 1, 2, 3]);
    }

    #[test]
    fn construction_preselects_current_item() {
        let picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(2));
        assert_eq!(picker.selected_orig_index(), Some(2));
    }
}
