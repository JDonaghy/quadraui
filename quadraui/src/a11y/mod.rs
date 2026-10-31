//! Accessibility data fields and the tree they feed.
//!
//! This module provides the data-field half of the accessibility plan:
//! a typed [`A11yRole`] plus a typed state bag, carried on [`A11yInfo`].
//! [`tree`] turns a captured [`crate::frame::OwnedFrame`] plus a
//! [`crate::FocusManager`] into a backend-independent accessibility tree
//! (and, behind the `a11y` cargo feature, an `accesskit::TreeUpdate`).
//! Per-platform AT backend wiring (UI Automation / NSAccessibility /
//! AT-SPI) consumes that update and is separate.
//!
//! # Why this is a standalone type, not a field on every primitive
//!
//! Adding new required fields directly to [`crate::TabBar`],
//! [`crate::ActivityBar`], [`crate::Spinner`], etc. would break every
//! call site: these are exhaustive structs with all-`pub` fields built
//! via exhaustive literals without `..Default::default()` spread.
//!
//! The preferred shape for additive data that cannot be a defaulted field
//! is a **sidecar type** — the same pattern this crate uses for
//! [`crate::ActivityBarStyle`], [`crate::TabChrome`], and
//! [`crate::TooltipChrome`]. [`A11yInfo`] follows that precedent.
//! It is marked `#[non_exhaustive]` so later additions (focus order,
//! `described_by`, additional state) remain additive rather than
//! breaking. It is not yet wired into any primitive's struct or the
//! `Backend` trait — each integration is a breaking change requiring
//! its own PR and migration.
//!
//! Field names match `docs/UI_CRATE_DESIGN.md` decision #6 (`a11y_role`,
//! `a11y_label`) rather than the shorter `role`/`label`, so the audit
//! command that identified this gap (`grep -rn a11y_role quadraui/src`)
//! still finds hits.
//!
//! # Typed role, typed state
//!
//! [`A11yRole`] replaces a free-form role string: a typo in `"buton"`
//! is invisible to the compiler, but an unknown [`A11yRole`] variant
//! isn't representable at all. Each variant maps 1:1 onto an
//! `accesskit::Role` — see [`A11yRole`]'s doc for the exact mapping —
//! so wiring a real AT backend later is a lookup table, not a
//! redesign.
//!
//! [`A11yRole`], [`A11yChecked`] and [`A11yLive`] serialise with
//! `#[serde(rename_all = "snake_case")]` (e.g. `A11yRole::TextInput` as
//! `"text_input"`) rather than serde's PascalCase default — chosen now,
//! before any serialized `A11yInfo` exists in the wild (`frame.rs`
//! documents `OwnedNode.a11y` as always `None` from `from_layout`),
//! since changing the wire format later would silently break any JSON
//! already written under the default.
//!
//! The data model (`A11yRole`, `A11yChecked`, `A11yLive`,
//! `A11yValueRange`, `A11yInfo`) is always compiled, regardless of
//! feature flags, so a descriptor that embeds `A11yInfo` never changes
//! shape depending on which backend features a consumer enables. Only
//! the `accesskit`-facing `From` conversions are gated behind the
//! optional `a11y` cargo feature, which pulls in the `accesskit`
//! dependency.
//!
//! # Field naming: `a11y_role`/`a11y_label` keep the prefix, state
//! fields don't
//!
//! `A11yInfo`'s two oldest fields, `a11y_role` and `a11y_label`, keep the
//! `a11y_` prefix to match `docs/UI_CRATE_DESIGN.md` decision #6 (see
//! above). The state/structure fields alongside the typed role
//! (`selected`, `expanded`, `checked`, `disabled`, `read_only`,
//! `value`, `value_range`, `level`, `position_in_set`, `set_size`,
//! `live`, `hidden`, `modal`) deliberately drop it: they live only on this sidecar struct,
//! so the field name alone (`info.selected`, not `info.a11y_selected`)
//! is already unambiguous, and decision #6's own rationale was to
//! disambiguate `a11y_role`/`a11y_label` from a primitive's own
//! same-named fields — a concern that doesn't apply to fields that only
//! ever exist here.

use serde::{Deserialize, Serialize};

mod action;
mod chrome;
mod content;
pub mod tree;

pub use action::{route_action, A11yAction, A11yActionValue};
pub use content::markdown_a11y_children;
pub use tree::{
    frame_to_tree, frame_to_tree_with_modal, A11yChild, A11yNodeId, A11yPrimitive, A11yTree,
    A11yTreeBuilder, A11yTreeNode, A11yUpdate,
};

/// A typed accessibility role, replacing a free-form role string.
///
/// Each variant maps 1:1 onto an [`accesskit::Role`] (see the `a11y`
/// cargo feature's `From<A11yRole> for accesskit::Role` impl below) —
/// this enum is deliberately a subset of `accesskit::Role`'s much larger
/// vocabulary, scoped to what this crate's own catalogue of primitives
/// actually needs. `Editor` and `Document` both map onto
/// `accesskit::Role::Document`: AccessKit has no separate "code editor"
/// role, and a multi-line editable document is what every AT actually
/// cares about. `Presentation` maps onto `accesskit::Role::GenericContainer`,
/// which is AccessKit's equivalent of the ARIA `none`/`presentation` role —
/// an element an AT should skip over entirely.
///
/// `#[non_exhaustive]`: new roles are additive, not breaking. Construct
/// with one of the named variants; there is no catch-all "custom string"
/// escape hatch, deliberately — see the module doc.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A11yRole {
    /// A top-level window or root application surface.
    Window,
    /// A toolbar grouping related actions.
    Toolbar,
    /// A clickable button.
    Button,
    /// A single tab within a [`Self::TabList`].
    Tab,
    /// A container of [`Self::Tab`]s.
    TabList,
    /// A menu (menu bar entry, context menu, or submenu).
    Menu,
    /// A single selectable entry within a [`Self::Menu`].
    MenuItem,
    /// The application's top-level menu bar, containing [`Self::Menu`]s.
    MenuBar,
    /// A hierarchical tree of [`Self::TreeItem`]s.
    Tree,
    /// A single node within a [`Self::Tree`].
    TreeItem,
    /// A flat list of [`Self::ListItem`]s.
    List,
    /// A single entry within a [`Self::List`].
    ListItem,
    /// A table of [`Self::Row`]s/[`Self::Cell`]s.
    Table,
    /// A row within a [`Self::Table`].
    Row,
    /// A single cell within a [`Self::Table`] [`Self::Row`].
    Cell,
    /// A single-line (or multi-line) editable text field.
    TextInput,
    /// A two- or tri-state checkbox or toggle switch.
    CheckBox,
    /// A container of mutually exclusive [`Self::RadioButton`]s, such as a
    /// segmented control.
    RadioGroup,
    /// One mutually exclusive option within a [`Self::RadioGroup`].
    RadioButton,
    /// A continuous numeric control dragged along a track.
    Slider,
    /// A collapsed list of options with one currently selected.
    ComboBox,
    /// Static text naming another element, such as a form field caption.
    Label,
    /// A group of related input fields submitted together.
    Form,
    /// A read-mostly document surface (e.g. a rendered preview pane).
    Document,
    /// A multi-line editable document, such as a code editor. Maps onto
    /// the same `accesskit::Role::Document` as [`Self::Document`] — see
    /// this enum's own doc for why AccessKit has no separate role.
    Editor,
    /// A VT-100-style terminal surface.
    Terminal,
    /// A modal or non-modal dialog window.
    Dialog,
    /// A status message region (e.g. a status bar).
    Status,
    /// An important, time-sensitive message such as a toast notification.
    Alert,
    /// Transient explanatory text attached to another element.
    Tooltip,
    /// A scrollbar driving a scrollable region.
    ScrollBar,
    /// A draggable divider between two resizable panes.
    Splitter,
    /// A progress indicator.
    Progress,
    /// A non-interactive image.
    Image,
    /// A data visualization such as a line, bar, or scatter chart. Maps
    /// onto `accesskit::Role::GraphicsDocument`. A chart's visual series
    /// data has no textual equivalent, so the accessible name falls back,
    /// in order, to the app's own label override, the chart's axis labels,
    /// then (only when the legend is shown) its series labels; a chart with
    /// none of those is announced as unlabelled.
    Chart,
    /// An arbitrary immediate-mode drawing surface. Maps onto
    /// `accesskit::Role::Canvas`. Carries no synthesized label, for the
    /// same reason as [`Self::Chart`]: the app must supply one.
    Canvas,
    /// A generic grouping container with no more specific role.
    Group,
    /// Decorative content an AT should skip — ARIA's `none`/`presentation`.
    Presentation,
    /// A section heading. Carries its level (1 for an H1, 2 for an H2, …)
    /// in [`A11yInfo::level`].
    Heading,
    /// A hyperlink to another resource. Carries its target in
    /// [`A11yInfo::value`], since this enum has no dedicated href field.
    Link,
}

/// A tri-state "checked" value for checkboxes, radio buttons, and
/// tri-state toggles. Mirrors `accesskit::Toggled`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A11yChecked {
    /// Unchecked.
    False,
    /// Checked.
    True,
    /// Partially checked (e.g. a parent checkbox with mixed children).
    Mixed,
}

/// Live-region politeness, controlling how urgently an AT should
/// announce a change to this element's content. Mirrors `accesskit::Live`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A11yLive {
    /// Not a live region — no announcement on change.
    #[default]
    Off,
    /// Announce changes, but only once the AT is idle.
    Polite,
    /// Announce changes immediately, interrupting other speech.
    Assertive,
}

/// A numeric value plus its range, for sliders, progress bars, and
/// spin buttons. All three fields are the same unit (the caller's); this
/// type carries no notion of percentage vs. absolute.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct A11yValueRange {
    /// The current value.
    pub current: f64,
    /// The minimum value.
    pub min: f64,
    /// The maximum value.
    pub max: f64,
}

impl A11yValueRange {
    /// Builds a value range from its three components.
    pub fn new(current: f64, min: f64, max: f64) -> Self {
        Self { current, min, max }
    }
}

/// Accessibility metadata a future per-primitive field or `Backend`
/// sidecar parameter would carry: a typed AT role, an accessible name,
/// and typed state (selected, expanded, checked, disabled, read-only,
/// value, level, position-in-set, live-region politeness).
///
/// `#[non_exhaustive]`: later additions (focus order, `described_by`)
/// are additive rather than breaking. Construct with [`A11yInfo::new`] /
/// [`A11yInfo::default`] and the `with_*` builders.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct A11yInfo {
    /// Typed AT role hint. `None` (the default) means "no role asserted
    /// yet" — today's behaviour for every primitive, since none carry
    /// this data at all.
    #[serde(default)]
    pub a11y_role: Option<A11yRole>,
    /// Accessible name an AT would announce for this element. `None`
    /// (the default) means "no explicit label" — a future backend
    /// would need to fall back to visible text, same as today's
    /// AT-less behaviour.
    #[serde(default)]
    pub a11y_label: Option<String>,
    /// Whether this element is the selected item within its container
    /// (e.g. a selected tab or list item). `None` means "not
    /// applicable" rather than "not selected".
    #[serde(default)]
    pub selected: Option<bool>,
    /// Whether this element's children are currently visible (e.g. an
    /// expanded tree item or disclosure triangle). `None` means "not
    /// applicable" rather than "collapsed".
    #[serde(default)]
    pub expanded: Option<bool>,
    /// Tri-state checked value for checkboxes/radio buttons/toggles.
    #[serde(default)]
    pub checked: Option<A11yChecked>,
    /// Whether this element is disabled (present but not interactive).
    /// `None` means "disabledness not asserted" rather than "enabled" —
    /// the same "not applicable" vs. "explicitly false" distinction
    /// [`Self::selected`] documents, so a backend can tell a decorative
    /// or [`A11yRole::Presentation`] node (no assertion either way) apart
    /// from an element explicitly marked enabled.
    #[serde(default)]
    pub disabled: Option<bool>,
    /// Whether this element's value can be read but not edited. `None`
    /// means "read-only-ness not asserted", for the same reason as
    /// [`Self::disabled`].
    #[serde(default)]
    pub read_only: Option<bool>,
    /// The element's current value as text (e.g. a text input's
    /// content, or a combo box's selected option).
    #[serde(default)]
    pub value: Option<String>,
    /// The element's current numeric value and range (e.g. a slider or
    /// progress bar). Independent of [`Self::value`] — a slider can
    /// carry both a numeric range and a formatted text value.
    #[serde(default)]
    pub value_range: Option<A11yValueRange>,
    /// Depth within a hierarchical structure (e.g. a tree item's nesting
    /// level, or a heading's level). 1-based, matching ARIA's
    /// `aria-level`.
    #[serde(default)]
    pub level: Option<u32>,
    /// 1-based position of this element within its parent set (ARIA's
    /// `aria-posinset`), for sets whose full membership isn't otherwise
    /// derivable (e.g. a virtualised list).
    #[serde(default)]
    pub position_in_set: Option<u32>,
    /// Total number of elements in this element's parent set (ARIA's
    /// `aria-setsize`), paired with [`Self::position_in_set`].
    #[serde(default)]
    pub set_size: Option<u32>,
    /// Live-region politeness. `None` means "not a live region" —
    /// equivalent to [`A11yLive::Off`] but distinguishable from an
    /// explicit `Off` for backends that care about the difference.
    #[serde(default)]
    pub live: Option<A11yLive>,
    /// Whether this element and its subtree are withheld from assistive
    /// technology while still painted on screen — ARIA's `aria-hidden`.
    /// Set on every top-level surface behind an open modal dialog so the
    /// AT cursor cannot escape the dialog. `None` means "not asserted"
    /// rather than "visible".
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Whether this element is a modal dialog: content outside it is
    /// inert while it is open — ARIA's `aria-modal`. `None` means "not
    /// asserted" rather than "non-modal".
    #[serde(default)]
    pub modal: Option<bool>,
}

impl A11yInfo {
    /// An empty [`A11yInfo`] — equivalent to [`A11yInfo::default`],
    /// spelled as a constructor for symmetry with the `with_*`
    /// builders.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the typed AT role hint.
    pub fn with_role(mut self, role: A11yRole) -> Self {
        self.a11y_role = Some(role);
        self
    }

    /// Sets the accessible label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets whether this element is selected within its container.
    pub fn with_selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Sets whether this element's children are currently expanded.
    pub fn with_expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Sets the tri-state checked value.
    pub fn with_checked(mut self, checked: A11yChecked) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Sets whether this element is disabled.
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }

    /// Sets whether this element is read-only.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = Some(read_only);
        self
    }

    /// Sets the element's current value as text.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets the element's current numeric value and range.
    pub fn with_value_range(mut self, value_range: A11yValueRange) -> Self {
        self.value_range = Some(value_range);
        self
    }

    /// Sets the element's depth within a hierarchical structure.
    pub fn with_level(mut self, level: u32) -> Self {
        self.level = Some(level);
        self
    }

    /// Sets the element's 1-based position within its parent set.
    pub fn with_position_in_set(mut self, position_in_set: u32) -> Self {
        self.position_in_set = Some(position_in_set);
        self
    }

    /// Sets the total size of the element's parent set.
    pub fn with_set_size(mut self, set_size: u32) -> Self {
        self.set_size = Some(set_size);
        self
    }

    /// Sets the live-region politeness.
    pub fn with_live(mut self, live: A11yLive) -> Self {
        self.live = Some(live);
        self
    }

    /// Sets whether this element and its subtree are withheld from
    /// assistive technology.
    pub fn with_hidden(mut self, hidden: bool) -> Self {
        self.hidden = Some(hidden);
        self
    }

    /// Sets whether this element is a modal dialog.
    pub fn with_modal(mut self, modal: bool) -> Self {
        self.modal = Some(modal);
        self
    }
}

/// `accesskit`-facing conversions. Gated on the optional `a11y` cargo
/// feature, which pulls in the `accesskit` dependency — the data model
/// above is always compiled so a descriptor embedding [`A11yInfo`]
/// never changes shape depending on which backend features a consumer
/// enables (see the module doc).
#[cfg(feature = "a11y")]
mod accesskit_convert {
    use super::{A11yChecked, A11yLive, A11yRole};

    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    impl From<A11yRole> for accesskit::Role {
        fn from(role: A11yRole) -> Self {
            match role {
                A11yRole::Window => accesskit::Role::Window,
                A11yRole::Toolbar => accesskit::Role::Toolbar,
                A11yRole::Button => accesskit::Role::Button,
                A11yRole::Tab => accesskit::Role::Tab,
                A11yRole::TabList => accesskit::Role::TabList,
                A11yRole::Menu => accesskit::Role::Menu,
                A11yRole::MenuItem => accesskit::Role::MenuItem,
                A11yRole::MenuBar => accesskit::Role::MenuBar,
                A11yRole::Tree => accesskit::Role::Tree,
                A11yRole::TreeItem => accesskit::Role::TreeItem,
                A11yRole::List => accesskit::Role::List,
                A11yRole::ListItem => accesskit::Role::ListItem,
                A11yRole::Table => accesskit::Role::Table,
                A11yRole::Row => accesskit::Role::Row,
                A11yRole::Cell => accesskit::Role::Cell,
                A11yRole::TextInput => accesskit::Role::TextInput,
                A11yRole::CheckBox => accesskit::Role::CheckBox,
                A11yRole::RadioGroup => accesskit::Role::RadioGroup,
                A11yRole::RadioButton => accesskit::Role::RadioButton,
                A11yRole::Slider => accesskit::Role::Slider,
                A11yRole::ComboBox => accesskit::Role::ComboBox,
                A11yRole::Label => accesskit::Role::Label,
                A11yRole::Form => accesskit::Role::Form,
                A11yRole::Document => accesskit::Role::Document,
                A11yRole::Editor => accesskit::Role::Document,
                A11yRole::Terminal => accesskit::Role::Terminal,
                A11yRole::Dialog => accesskit::Role::Dialog,
                A11yRole::Status => accesskit::Role::Status,
                A11yRole::Alert => accesskit::Role::Alert,
                A11yRole::Tooltip => accesskit::Role::Tooltip,
                A11yRole::ScrollBar => accesskit::Role::ScrollBar,
                A11yRole::Splitter => accesskit::Role::Splitter,
                A11yRole::Progress => accesskit::Role::ProgressIndicator,
                A11yRole::Image => accesskit::Role::Image,
                A11yRole::Chart => accesskit::Role::GraphicsDocument,
                A11yRole::Canvas => accesskit::Role::Canvas,
                A11yRole::Group => accesskit::Role::Group,
                A11yRole::Presentation => accesskit::Role::GenericContainer,
                A11yRole::Heading => accesskit::Role::Heading,
                A11yRole::Link => accesskit::Role::Link,
            }
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    impl From<A11yChecked> for accesskit::Toggled {
        fn from(checked: A11yChecked) -> Self {
            match checked {
                A11yChecked::False => accesskit::Toggled::False,
                A11yChecked::True => accesskit::Toggled::True,
                A11yChecked::Mixed => accesskit::Toggled::Mixed,
            }
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    impl From<A11yLive> for accesskit::Live {
        fn from(live: A11yLive) -> Self {
            match live {
                A11yLive::Off => accesskit::Live::Off,
                A11yLive::Polite => accesskit::Live::Polite,
                A11yLive::Assertive => accesskit::Live::Assertive,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{A11yChecked, A11yLive, A11yRole};

        #[test]
        fn role_maps_to_accesskit() {
            assert_eq!(
                accesskit::Role::from(A11yRole::Button),
                accesskit::Role::Button
            );
            assert_eq!(
                accesskit::Role::from(A11yRole::Document),
                accesskit::Role::Document
            );
            assert_eq!(
                accesskit::Role::from(A11yRole::Editor),
                accesskit::Role::Document
            );
            assert_eq!(
                accesskit::Role::from(A11yRole::Presentation),
                accesskit::Role::GenericContainer
            );
            assert_eq!(
                accesskit::Role::from(A11yRole::Heading),
                accesskit::Role::Heading
            );
            assert_eq!(accesskit::Role::from(A11yRole::Link), accesskit::Role::Link);
        }

        #[test]
        fn checked_maps_to_accesskit() {
            assert_eq!(
                accesskit::Toggled::from(A11yChecked::Mixed),
                accesskit::Toggled::Mixed
            );
        }

        #[test]
        fn live_maps_to_accesskit() {
            assert_eq!(
                accesskit::Live::from(A11yLive::Assertive),
                accesskit::Live::Assertive
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let info = A11yInfo::default();
        assert_eq!(info.a11y_role, None);
        assert_eq!(info.a11y_label, None);
        assert_eq!(info.selected, None);
        assert_eq!(info.expanded, None);
        assert_eq!(info.checked, None);
        assert_eq!(info.disabled, None);
        assert_eq!(info.read_only, None);
        assert_eq!(info.value, None);
        assert_eq!(info.value_range, None);
        assert_eq!(info.level, None);
        assert_eq!(info.position_in_set, None);
        assert_eq!(info.set_size, None);
        assert_eq!(info.live, None);
        assert_eq!(info.hidden, None);
        assert_eq!(info.modal, None);
    }

    #[test]
    fn builders_set_fields() {
        let info = A11yInfo::new()
            .with_role(A11yRole::Button)
            .with_label("Save")
            .with_selected(true)
            .with_expanded(false)
            .with_checked(A11yChecked::Mixed)
            .with_disabled(true)
            .with_read_only(true)
            .with_value("42")
            .with_value_range(A11yValueRange::new(42.0, 0.0, 100.0))
            .with_level(2)
            .with_position_in_set(3)
            .with_set_size(10)
            .with_live(A11yLive::Polite)
            .with_hidden(true)
            .with_modal(true);

        assert_eq!(info.a11y_role, Some(A11yRole::Button));
        assert_eq!(info.a11y_label.as_deref(), Some("Save"));
        assert_eq!(info.selected, Some(true));
        assert_eq!(info.expanded, Some(false));
        assert_eq!(info.checked, Some(A11yChecked::Mixed));
        assert_eq!(info.disabled, Some(true));
        assert_eq!(info.read_only, Some(true));
        assert_eq!(info.value.as_deref(), Some("42"));
        assert_eq!(
            info.value_range,
            Some(A11yValueRange::new(42.0, 0.0, 100.0))
        );
        assert_eq!(info.level, Some(2));
        assert_eq!(info.position_in_set, Some(3));
        assert_eq!(info.set_size, Some(10));
        assert_eq!(info.live, Some(A11yLive::Polite));
        assert_eq!(info.hidden, Some(true));
        assert_eq!(info.modal, Some(true));
    }

    /// `#[serde(default)]` allows JSON missing these fields (or `{}`) to
    /// deserialize successfully, preserving backward compatibility with
    /// data serialized before these fields existed.
    #[test]
    fn deserializes_from_empty_json() {
        let info: A11yInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(info, A11yInfo::default());
    }

    #[test]
    fn round_trips_through_json() {
        let info = A11yInfo::new()
            .with_role(A11yRole::Tab)
            .with_label("Untitled-1")
            .with_selected(true)
            .with_level(1);
        let json = serde_json::to_string(&info).unwrap();
        let back: A11yInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn checked_default_is_distinct_from_false() {
        // "No checkbox state at all" (`A11yInfo::new()`, `checked: None`)
        // must stay distinguishable from "unchecked"
        // (`with_checked(A11yChecked::False)`) through a JSON round trip,
        // not just in memory — collapsing them on the wire would make an
        // unchecked checkbox indistinguishable from a button with no
        // checked state at all to anything that only sees the JSON.
        let unchecked = A11yInfo::new().with_checked(A11yChecked::False);
        let absent = A11yInfo::new();

        let unchecked_json = serde_json::to_string(&unchecked).unwrap();
        let absent_json = serde_json::to_string(&absent).unwrap();
        assert_ne!(unchecked_json, absent_json);

        let unchecked_back: A11yInfo = serde_json::from_str(&unchecked_json).unwrap();
        let absent_back: A11yInfo = serde_json::from_str(&absent_json).unwrap();
        assert_eq!(unchecked_back.checked, Some(A11yChecked::False));
        assert_eq!(absent_back.checked, None);
    }
}
