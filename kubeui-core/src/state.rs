//! Pure application state — no rendering, no event types.
//!
//! Backends own a single [`AppState`] instance and mutate it through
//! [`crate::Action`] dispatched to [`crate::apply_action`]. Builders
//! in [`crate::view`] turn the state into `quadraui` primitives that
//! the backend rasterises.

use crate::k8s::ResourceItem;

/// Top-level app state. Cloning is cheap-ish (Vec<ResourceItem> can
/// be large but is rarely cloned outside tests). Mutation is through
/// the [`crate::Action`] reducer; nothing in the rest of the crate
/// holds a long-lived borrow.
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
    /// Modal picker (namespace or resource kind). `Some` while open.
    pub picker: Option<Picker>,
    /// Last status / error message — surfaced in the bottom bar.
    pub status: String,
    /// Set by [`crate::apply_action`] when the user hits the quit
    /// action. Backends check this in their event loop and exit.
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

/// Modal list-picker state — filterable. See [`Self::visible_indices`]
/// for the filter semantics.
pub struct Picker {
    pub title: String,
    pub purpose: PickerPurpose,
    pub items: Vec<String>,
    pub query: String,
    /// Index into the filtered view (`visible_indices()`), not the
    /// original `items` Vec.
    pub selected: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerPurpose {
    Namespace,
    ResourceKind,
}

impl Picker {
    /// Indices of `items` that match the current query, in original
    /// order. Empty query → all indices. Match is case-insensitive
    /// substring.
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.items.len()).collect();
        }
        let q = self.query.to_ascii_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, name)| name.to_ascii_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Original index of the currently highlighted row, if any.
    pub fn selected_orig_index(&self) -> Option<usize> {
        self.visible_indices().get(self.selected).copied()
    }

    pub fn move_down(&mut self) {
        let n = self.visible_indices().len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1).min(n - 1);
    }
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
    pub fn type_char(&mut self, ch: char) {
        self.query.push(ch);
        self.selected = 0;
    }
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }
}
