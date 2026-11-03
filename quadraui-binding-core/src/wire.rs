//! The neutral end of a binding's descriptor wire decode: a render
//! callback's returned layout tree / flat [`RenderFrame`] shape
//! ([`decode_render_output`]), one layout tree node
//! ([`decode_layout_node`]), and `run_shell`'s `config` dict
//! ([`decode_shell_config`]).
//!
//! Every decoder here is generic over `serde::Deserializer`, not tied to
//! any host language's own wire format — a binding supplies its own
//! `Deserializer` implementation over its own object model (a Python dict
//! today, a JS object next) and maps the returned [`crate::DecodeError`]
//! to its own exception type. Internally, every entry point first
//! deserializes the whole input into one [`serde_json::Value`] tree (the
//! same merge medium [`crate::host::merge_overrides`] already uses), then
//! dispatches against that uniform shape — the dynamically-keyed,
//! "which of these N keys is present" dict shapes a layout tree node or
//! `ShellConfig` decodes from don't map onto a single `#[derive(Deserialize)]`
//! struct, but a `serde_json::Value` is cheap to re-inspect by key as many
//! times as the recursive decode needs.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use quadraui::{
    Backend, BackendWidget, BottomPanelConfig, BottomPanelTab, Color, Panel, PanelDefinition, Rect,
    ShellConfig, ShellPosition, SidebarPanel, Split, SplitDirection, SplitTree, TabBar,
    TabBarSegment, TabItem, WidgetId,
};

use crate::extra_paint::{decode_extra_primitive, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
use crate::host::text_display_from_lines;
use crate::layout::{
    validate_unique_leaf_ids, LayoutLeaf, LayoutNode, LayoutPanel, LayoutSidebarPanel, LayoutSplit,
    LayoutSplitTree, LayoutTabEntry, LayoutTabs, LeafDescriptor, LEAF_KIND_KEYS,
};
use crate::render::RenderFrame;
use crate::{decode_value, DecodeError};

/// Minimal [`BackendWidget`] painted into a `run_shell` bottom-panel tab's
/// body: a single line naming the tab's own id. Bottom-panel tabs built
/// from `run_shell`'s `"bottom_panel"` config have no binding-registered
/// per-tab content callback (out of scope for this first cut), but
/// [`BottomPanelTab::content`] still requires *some* [`BackendWidget`], and
/// painting the active tab's id makes "which tab is active" visible on the
/// rendered screen — useful for a driver test to assert on, instead of an
/// invisible no-op.
pub struct StaticTabContent(pub String);

impl BackendWidget for StaticTabContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let td = text_display_from_lines(
            WidgetId::new(format!("shell:bottom_panel:{}", self.0)),
            vec![self.0.clone()],
        );
        backend.draw_text_display(rect, &td);
    }
}

// ─── Value helpers ──────────────────────────────────────────────────────────

/// Reads `value[key]`, treating a `null` entry the same as a missing one —
/// every decoder below has its own binding's "`None`-vs-missing both mean
/// empty" quirk collapsed by this one shared accessor.
fn obj_get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object()?.get(key).filter(|v| !v.is_null())
}

/// Checks that `value` is a dict, raising a [`DecodeError`] that names
/// `suffix` (already carrying whatever label/id prefix the caller has in
/// hand) otherwise.
fn must_be_dict<'a>(what: &str, value: &'a Value, suffix: &str) -> Result<&'a Value, DecodeError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(DecodeError(format!("{what}: {suffix} must be a dict")))
    }
}

/// Reads `value[key]` as a required sub-dict, raising a [`DecodeError`]
/// from `context` (a caller-supplied `"{what}: <label>"` prefix) if it is
/// missing or not a dict.
fn required_dict<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a Value, DecodeError> {
    match obj_get(value, key) {
        Some(v) if v.is_object() => Ok(v),
        Some(_) => Err(DecodeError(format!("{context} {key:?} must be a dict"))),
        None => Err(DecodeError(format!(
            "{context} is missing required {key:?}"
        ))),
    }
}

/// Reads `value[key]` as a required `list`, raising a [`DecodeError`] from
/// `context` if it is missing or not a list.
fn required_list<'a>(
    value: &'a Value,
    key: &str,
    context: &str,
) -> Result<&'a Vec<Value>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_array()
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be a list"))),
        None => Err(DecodeError(format!(
            "{context} is missing required {key:?}"
        ))),
    }
}

/// Reads `value[key]` as a required string, raising a [`DecodeError`] from
/// `context` if it is missing or not a string.
fn required_str(value: &Value, key: &str, context: &str) -> Result<String, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be a string"))),
        None => Err(DecodeError(format!(
            "{context} is missing required {key:?}"
        ))),
    }
}

/// Reads `value[key]` as a required `f32`, raising a [`DecodeError`] from
/// `context` if it is missing or not a number.
fn required_f32(value: &Value, key: &str, context: &str) -> Result<f32, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be a number"))),
        None => Err(DecodeError(format!(
            "{context} is missing required {key:?}"
        ))),
    }
}

/// Reads `value[key]` as an optional `f32`, raising a [`DecodeError`] from
/// `context` if present but not a number. `None` when absent.
fn opt_f32(value: &Value, key: &str, context: &str) -> Result<Option<f32>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be a number"))),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an optional `bool`, raising a [`DecodeError`] from
/// `context` if present but not a bool. `None` when absent.
fn opt_bool(value: &Value, key: &str, context: &str) -> Result<Option<bool>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be a bool"))),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an optional non-negative integer, raising a
/// [`DecodeError`] from `context` if present but not one. `None` when
/// absent.
fn opt_usize(value: &Value, key: &str, context: &str) -> Result<Option<usize>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be an integer"))),
        None => Ok(None),
    }
}

/// `"horizontal"`/`"vertical"` → [`SplitDirection`], shared by a top-level
/// `"split"` node and a `"split_tree"` node's own nested `"split"` shape —
/// the lowercase, Python-idiomatic spelling this wire format accepts,
/// independent of [`SplitDirection`]'s own (PascalCase, serde-default)
/// wire form.
const SPLIT_ORIENTATIONS: [&str; 2] = ["horizontal", "vertical"];

fn split_direction(context: &str, orientation: &str) -> Result<SplitDirection, DecodeError> {
    match orientation {
        "horizontal" => Ok(SplitDirection::Horizontal),
        "vertical" => Ok(SplitDirection::Vertical),
        _ => Err(DecodeError(format!(
            "{context} \"orientation\" must be one of {SPLIT_ORIENTATIONS:?}, got {orientation:?}"
        ))),
    }
}

// ─── Layout tree decode ─────────────────────────────────────────────────────

/// The dispatch keys [`decode_layout_node`] checks, in check order — a
/// `"row"`/`"column"`/`"split"`/`"tabs"`/`"panel"`/`"sidebar_panel"`/
/// `"split_tree"` node, else a leaf. Shared with [`is_layout_tree`]'s
/// flat-vs-tree sniff so the two can't drift onto different key lists.
pub const LAYOUT_NODE_KEYS: [&str; 7] = [
    "row",
    "column",
    "split",
    "tabs",
    "panel",
    "sidebar_panel",
    "split_tree",
];

/// Decodes one layout tree node from `deserializer`: a `"row"` or `"column"`
/// key holds the container's children, a `"split"` key holds a two-pane
/// divider, a `"tabs"` key holds a tab strip plus per-tab content, a
/// `"panel"`/`"sidebar_panel"` key holds one chrome descriptor plus a
/// single content subtree, a `"split_tree"` key holds an arbitrarily
/// nested divider tree; any other shape is decoded as a leaf.
pub fn decode_layout_node<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
) -> Result<LayoutNode, DecodeError> {
    let value = Value::deserialize(deserializer)
        .map_err(|err| DecodeError(format!("{what}: invalid descriptor: {err}")))?;
    layout_node_from_value(what, &value)
}

fn layout_node_from_value(what: &str, value: &Value) -> Result<LayoutNode, DecodeError> {
    if let Some(v) = obj_get(value, "row") {
        return Ok(LayoutNode::Row(layout_children_from_value(what, v)?));
    }
    if let Some(v) = obj_get(value, "column") {
        return Ok(LayoutNode::Column(layout_children_from_value(what, v)?));
    }
    if let Some(v) = obj_get(value, "split") {
        let split_value = must_be_dict(what, v, "\"split\"")?;
        return Ok(LayoutNode::Split(layout_split_from_value(
            what,
            split_value,
        )?));
    }
    if let Some(v) = obj_get(value, "tabs") {
        let tabs_value = must_be_dict(what, v, "\"tabs\"")?;
        return Ok(LayoutNode::Tabs(layout_tabs_from_value(what, tabs_value)?));
    }
    if let Some(v) = obj_get(value, "panel") {
        let panel_value = must_be_dict(what, v, "\"panel\"")?;
        return Ok(LayoutNode::Panel(layout_panel_from_value(
            what,
            panel_value,
        )?));
    }
    if let Some(v) = obj_get(value, "sidebar_panel") {
        let sidebar_value = must_be_dict(what, v, "\"sidebar_panel\"")?;
        return Ok(LayoutNode::SidebarPanel(layout_sidebar_panel_from_value(
            what,
            sidebar_value,
        )?));
    }
    if let Some(v) = obj_get(value, "split_tree") {
        let split_tree_value = must_be_dict(what, v, "\"split_tree\"")?;
        return Ok(LayoutNode::SplitTree(layout_split_tree_from_value(
            what,
            split_tree_value,
        )?));
    }
    Ok(LayoutNode::Leaf(layout_leaf_from_value(what, value)?))
}

/// Decodes a `"row"`/`"column"` container's children: a `list` of child
/// dicts, each recursively decoded by [`layout_node_from_value`].
fn layout_children_from_value(what: &str, value: &Value) -> Result<Vec<LayoutNode>, DecodeError> {
    let list = value
        .as_array()
        .ok_or_else(|| DecodeError(format!("{what}: row/column children must be a list")))?;
    list.iter()
        .map(|child| {
            if !child.is_object() {
                return Err(DecodeError(format!(
                    "{what}: each row/column child must be a dict"
                )));
            }
            layout_node_from_value(what, child)
        })
        .collect()
}

/// Decodes a `"split"` node: `{"id", "orientation", "ratio", "first",
/// "second"}`, with `"first_min"`/`"second_min"` optional (defaulting to
/// `0.0`, a portable line count — [`crate::render::paint_layout`]'s
/// `LayoutNode::Split` arm scales them by the backend's line height before
/// painting). `"first"`/`"second"` are full child node dicts, recursively
/// decoded by [`layout_node_from_value`].
fn layout_split_from_value(what: &str, value: &Value) -> Result<LayoutSplit, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError(format!("{what}: split \"id\" must be a string")))?,
        None => {
            return Err(DecodeError(format!(
                "{what}: split is missing required \"id\""
            )))
        }
    };
    let context = format!("{what}: split {id:?}");

    let orientation = required_str(value, "orientation", &context)?;
    let direction = split_direction(&context, &orientation)?;
    let ratio = required_f32(value, "ratio", &context)?;
    let first_min = opt_f32(value, "first_min", &context)?.unwrap_or(0.0);
    let second_min = opt_f32(value, "second_min", &context)?.unwrap_or(0.0);

    let first_value = required_dict(value, "first", &context)?;
    let first = layout_node_from_value(what, first_value)?;
    let second_value = required_dict(value, "second", &context)?;
    let second = layout_node_from_value(what, second_value)?;

    Ok(LayoutSplit {
        split: Split {
            id: WidgetId::new(id),
            direction,
            ratio,
            first_min,
            second_min,
        },
        first: Box::new(first),
        second: Box::new(second),
    })
}

/// Decodes a `"panel"` node: the chrome dict decoded generically into a
/// [`Panel`] (`title`/`actions`/`accent`/`collapsed`, each `#[serde(default)]`,
/// so a minimal `{"id": ...}` is enough) plus a required `"content"` child
/// node, recursively decoded by [`layout_node_from_value`]. The `"content"`
/// key is harmless noise to the `Panel` decode itself — `Panel` has no
/// `deny_unknown_fields`.
fn layout_panel_from_value(what: &str, value: &Value) -> Result<LayoutPanel, DecodeError> {
    let panel: Panel = decode_value(what, value)?;
    let id = panel.id.as_str().to_owned();
    let context = format!("{what}: panel {id:?}");
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(what, content_value)?;
    Ok(LayoutPanel {
        panel,
        content: Box::new(content),
    })
}

/// Decodes a `"sidebar_panel"` node: the chrome dict decoded generically
/// into a [`SidebarPanel`] (optional `toolbar`/`toolbar_height`) plus a
/// required `"content"` child node, recursively decoded by
/// [`layout_node_from_value`].
fn layout_sidebar_panel_from_value(
    what: &str,
    value: &Value,
) -> Result<LayoutSidebarPanel, DecodeError> {
    let sidebar_panel: SidebarPanel = decode_value(what, value)?;
    let id = sidebar_panel.id.as_str().to_owned();
    let context = format!("{what}: sidebar_panel {id:?}");
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(what, content_value)?;
    Ok(LayoutSidebarPanel {
        sidebar_panel,
        content: Box::new(content),
    })
}

/// Decodes a `"split_tree"` node: `{"id", "root"}`, where `"root"` is
/// recursively either `{"leaf": <child node dict>}` or `{"split":
/// {"orientation", "ratio", "first", "second"}}` (the same `"orientation"`
/// spelling and values as a top-level `"split"` node). Each `"leaf"`
/// becomes one position in the `quadraui::SplitTree` geometry built
/// alongside it, addressed by a synthetic, internal-only id; its decoded
/// content node is pushed onto the matching [`LayoutSplitTree::leaves`] in
/// the same pre-order, for [`crate::render::paint_layout`] to zip back
/// against the resolved rects at paint time.
fn layout_split_tree_from_value(what: &str, value: &Value) -> Result<LayoutSplitTree, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError(format!("{what}: split_tree \"id\" must be a string")))?,
        None => {
            return Err(DecodeError(format!(
                "{what}: split_tree is missing required \"id\""
            )));
        }
    };
    let context = format!("{what}: split_tree {id:?}");
    let root_value = required_dict(value, "root", &context)?;

    let mut leaves = Vec::new();
    let tree = split_tree_node_from_value(what, &id, root_value, &mut leaves)?;
    Ok(LayoutSplitTree { id, tree, leaves })
}

/// Recursively decodes one node of a `"split_tree"`'s `"root"` tree. A
/// `{"leaf": <node dict>}` decodes its content via [`layout_node_from_value`],
/// pushes it onto `leaves`, and returns a `SplitTree::Leaf` carrying a
/// synthetic id addressing its position (`"{tree_id}::leaf::{index}"`,
/// never user-facing, never checked by [`validate_unique_leaf_ids`]). A
/// `{"split": {...}}` recurses into `"first"`/`"second"` in pre-order,
/// matching the order the resolved split-tree layout's leaves are in.
fn split_tree_node_from_value(
    what: &str,
    tree_id: &str,
    value: &Value,
    leaves: &mut Vec<LayoutNode>,
) -> Result<SplitTree, DecodeError> {
    if let Some(v) = obj_get(value, "leaf") {
        let leaf_value = must_be_dict(what, v, &format!("split_tree {tree_id:?} \"leaf\""))?;
        let content = layout_node_from_value(what, leaf_value)?;
        let placeholder = WidgetId::new(format!("{tree_id}::leaf::{}", leaves.len()));
        leaves.push(content);
        return Ok(SplitTree::leaf(placeholder));
    }
    if let Some(v) = obj_get(value, "split") {
        let split_value = must_be_dict(what, v, &format!("split_tree {tree_id:?} \"split\""))?;
        let context = format!("{what}: split_tree {tree_id:?} split");

        let orientation = required_str(split_value, "orientation", &context)?;
        let direction = split_direction(&context, &orientation)?;
        let ratio = required_f32(split_value, "ratio", &context)?;

        let first_value = required_dict(split_value, "first", &context)?;
        let first = split_tree_node_from_value(what, tree_id, first_value, leaves)?;
        let second_value = required_dict(split_value, "second", &context)?;
        let second = split_tree_node_from_value(what, tree_id, second_value, leaves)?;

        return Ok(SplitTree::split(direction, ratio, first, second));
    }
    Err(DecodeError(format!(
        "{what}: split_tree {tree_id:?} node must have a \"leaf\" or \"split\" key"
    )))
}

/// Decodes a `"tabs"` node: `{"id", "tabs", "selected", "closable",
/// "scroll_offset", "right_segments", "active_accent", "compact"}`, where
/// `"tabs"` is a non-empty `list` of `{"id", "title", "content", "closable",
/// "is_dirty", "is_preview"}` entries and `"selected"` is the index of the
/// entry whose `"content"` paints beneath the strip this frame. `"content"`
/// is a full child node dict, recursively decoded by
/// [`layout_node_from_value`]. An empty `"tabs"` list is a valid empty
/// state rather than an error — a render callback that starts with zero
/// tabs can still return a `tabs` node instead of special-casing the empty
/// case itself, and `"selected"` is only validated against the tab count
/// when there are tabs to select.
fn layout_tabs_from_value(what: &str, value: &Value) -> Result<LayoutTabs, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError(format!("{what}: tabs \"id\" must be a string")))?,
        None => {
            return Err(DecodeError(format!(
                "{what}: tabs is missing required \"id\""
            )))
        }
    };
    let context = format!("{what}: tabs {id:?}");

    let tabs_list = required_list(value, "tabs", &context)?;
    let mut items = Vec::with_capacity(tabs_list.len());
    let mut entries = Vec::with_capacity(tabs_list.len());
    for entry in tabs_list {
        if !entry.is_object() {
            return Err(DecodeError(format!("{context} each tab must be a dict")));
        }
        let tab_id = required_str(entry, "id", &format!("{context} tab"))?;
        let tab_context = format!("{context} tab {tab_id:?}");
        let title = required_str(entry, "title", &tab_context)?;
        let content_value = required_dict(entry, "content", &tab_context)?;
        let content = layout_node_from_value(what, content_value)?;
        let is_closable = opt_bool(entry, "closable", &tab_context)?.unwrap_or(true);
        let is_dirty = opt_bool(entry, "is_dirty", &tab_context)?.unwrap_or(false);
        let is_preview = opt_bool(entry, "is_preview", &tab_context)?.unwrap_or(false);

        items.push(TabItem {
            label: title,
            is_closable,
            is_dirty,
            is_preview,
            ..Default::default()
        });
        entries.push(LayoutTabEntry {
            id: tab_id,
            content: Box::new(content),
        });
    }

    let selected = opt_usize(value, "selected", &context)?.unwrap_or(0);
    if !items.is_empty() && selected >= items.len() {
        return Err(DecodeError(format!(
            "{context} \"selected\" index {selected} is out of range for {len} tab(s)",
            len = items.len()
        )));
    }
    if let Some(item) = items.get_mut(selected) {
        item.is_active = true;
    }

    let show_tab_close = opt_bool(value, "closable", &context)?.unwrap_or(true);
    let scroll_offset = opt_usize(value, "scroll_offset", &context)?.unwrap_or(0);
    let right_segments: Vec<TabBarSegment> = match obj_get(value, "right_segments") {
        Some(v) => {
            if !v.is_array() {
                return Err(DecodeError(format!(
                    "{context} \"right_segments\" must be a list"
                )));
            }
            serde_json::from_value(v.clone()).map_err(|err| {
                DecodeError(format!("{context} invalid \"right_segments\": {err}"))
            })?
        }
        None => Vec::new(),
    };
    let active_accent: Option<Color> = match obj_get(value, "active_accent") {
        Some(v) => {
            let accent_value = must_be_dict(what, v, &format!("tabs {id:?} \"active_accent\""))?;
            Some(decode_value(what, accent_value)?)
        }
        None => None,
    };
    let compact = opt_bool(value, "compact", &context)?.unwrap_or(false);

    Ok(LayoutTabs {
        tab_bar: TabBar {
            id: WidgetId::new(id),
            tabs: items,
            scroll_offset,
            right_segments,
            active_accent,
            show_tab_close,
            compact,
        },
        entries,
        selected,
    })
}

/// Decodes one `{"id": ..., "<kind>": <descriptor>}` leaf dict. `"id"`
/// must be present and a string; exactly one of [`LEAF_KIND_KEYS`] or an
/// [`EXTRA_PRIMITIVE_KINDS`] kind must be present and hold a `dict` — a
/// leaf with two keys from either set (fixed+fixed, fixed+extra, or
/// extra+extra) is rejected by the single `found` duplicate check shared
/// across both loops below.
fn layout_leaf_from_value(what: &str, value: &Value) -> Result<LayoutLeaf, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError(format!("{what}: leaf \"id\" must be a string")))?,
        None => {
            return Err(DecodeError(format!(
                "{what}: leaf is missing required \"id\""
            )))
        }
    };

    let mut found: Option<LeafDescriptor> = None;
    for key in LEAF_KIND_KEYS {
        let Some(v) = obj_get(value, key) else {
            continue;
        };
        let value_dict = must_be_dict(what, v, &format!("leaf {id:?} {key:?}"))?;
        if found.is_some() {
            return Err(DecodeError(format!(
                "{what}: leaf {id:?} has more than one descriptor kind"
            )));
        }
        found = Some(match key {
            "button" => LeafDescriptor::Button(decode_value(what, value_dict)?),
            "text_display" => LeafDescriptor::TextDisplay(decode_value(what, value_dict)?),
            "list" => LeafDescriptor::List(decode_value(what, value_dict)?),
            "tree" => LeafDescriptor::Tree(decode_value(what, value_dict)?),
            "data_table" => LeafDescriptor::DataTable(decode_value(what, value_dict)?),
            "tab_bar" => LeafDescriptor::TabBar(decode_value(what, value_dict)?),
            _ => unreachable!("LEAF_KIND_KEYS is exhaustive"),
        });
    }
    // Additive primitive kinds share the same leaf dict shape and the same
    // `found` duplicate-detection as the six fixed kinds above; a new
    // additive primitive only ever adds a line to `extra_paint`'s
    // `extra_primitives!` table, never a new loop here.
    for kind in EXTRA_PRIMITIVE_KINDS {
        let Some(v) = obj_get(value, kind) else {
            continue;
        };
        let value_dict = must_be_dict(what, v, &format!("leaf {id:?} {kind:?}"))?;
        if found.is_some() {
            return Err(DecodeError(format!(
                "{what}: leaf {id:?} has more than one descriptor kind"
            )));
        }
        found = Some(LeafDescriptor::Extra(decode_extra_primitive(
            what, kind, value_dict,
        )?));
    }

    let descriptor = found.ok_or_else(|| {
        DecodeError(format!(
            "{what}: leaf {id:?} is missing a descriptor (one of {LEAF_KIND_KEYS:?} or {:?})",
            EXTRA_PRIMITIVE_KINDS
        ))
    })?;

    Ok(LayoutLeaf { id, descriptor })
}

// ─── Render output (flat frame vs. layout tree) ────────────────────────────

/// The decoded shape of a render callback's return value: either the flat
/// per-slot `dict` ([`RenderFrame`], the pre-layout-tree shape) or a
/// layout tree ([`LayoutNode`]).
pub enum RenderOutput {
    Flat(Box<RenderFrame>),
    Tree(LayoutNode),
}

/// A `dict` carrying any of [`LAYOUT_NODE_KEYS`] is parsed as a layout
/// tree; any other `dict` is parsed as the flat per-slot shape, so a
/// render callback written against the flat shape keeps working
/// unchanged.
fn is_layout_tree(value: &Value) -> bool {
    LAYOUT_NODE_KEYS
        .iter()
        .any(|key| obj_get(value, key).is_some())
}

/// Decodes a render callback's returned dict into a [`RenderOutput`] — see
/// [`is_layout_tree`] for the flat-vs-tree sniff. On the tree path, also
/// runs [`validate_unique_leaf_ids`] over the decoded tree before
/// returning it.
pub fn decode_render_output<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
) -> Result<RenderOutput, DecodeError> {
    let value = Value::deserialize(deserializer)
        .map_err(|err| DecodeError(format!("{what}: invalid descriptor: {err}")))?;
    if is_layout_tree(&value) {
        let node = layout_node_from_value(what, &value)?;
        validate_unique_leaf_ids(&node)?;
        Ok(RenderOutput::Tree(node))
    } else {
        Ok(RenderOutput::Flat(Box::new(render_frame_from_value(
            what, &value,
        )?)))
    }
}

/// Reads and decodes one optional slot entry from a render callback's
/// returned `dict`: a missing key or a `null` value both mean "draw
/// nothing in this slot this frame"; anything else must be a `dict`
/// decodable into `T`.
fn slot_entry<T: serde::de::DeserializeOwned>(
    what: &str,
    value: &Value,
    key: &str,
) -> Result<Option<T>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => {
            if !v.is_object() {
                return Err(DecodeError(format!(
                    "{what}: {key:?} must be a dict or None"
                )));
            }
            Ok(Some(decode_value(what, v)?))
        }
        None => Ok(None),
    }
}

/// Reads and decodes one optional additive primitive slot entry, keyed by
/// `kind` (one of [`EXTRA_PRIMITIVE_KINDS`]), from a render callback's
/// returned `dict`. Mirrors [`slot_entry`]'s missing/`null`/dict-or-error
/// shape, dispatching the decode through [`decode_extra_primitive`]
/// instead of a single fixed type.
fn extra_slot_entry(
    what: &str,
    value: &Value,
    kind: &str,
) -> Result<Option<Rc<dyn ExtraPaint>>, DecodeError> {
    match obj_get(value, kind) {
        Some(v) => {
            if !v.is_object() {
                return Err(DecodeError(format!(
                    "{what}: {kind:?} must be a dict or None"
                )));
            }
            Ok(Some(decode_extra_primitive(what, kind, v)?))
        }
        None => Ok(None),
    }
}

/// Builds a [`RenderFrame`] from an already-materialized render callback
/// `dict` — the shared body of [`RenderFrame`]'s `Deserialize` impl and
/// [`decode_render_output`]'s flat-shape branch.
pub(crate) fn render_frame_from_value(
    what: &str,
    value: &Value,
) -> Result<RenderFrame, DecodeError> {
    let mut extra = HashMap::new();
    for kind in EXTRA_PRIMITIVE_KINDS {
        if let Some(v) = extra_slot_entry(what, value, kind)? {
            extra.insert(kind.to_string(), v);
        }
    }
    Ok(RenderFrame {
        button: slot_entry(what, value, "button")?,
        text_display: slot_entry(what, value, "text_display")?,
        list: slot_entry(what, value, "list")?,
        tree: slot_entry(what, value, "tree")?,
        data_table: slot_entry(what, value, "data_table")?,
        tab_bar: slot_entry(what, value, "tab_bar")?,
        extra,
    })
}

// ─── `run_shell` config decode ──────────────────────────────────────────────

/// [`dict_str`], but raises a [`DecodeError`] when `key` is absent or
/// `null` instead of returning `Ok(None)`.
fn dict_required_str(what: &str, value: &Value, key: &str) -> Result<String, DecodeError> {
    dict_str(what, value, key)?
        .ok_or_else(|| DecodeError(format!("{what}: missing required {key:?}")))
}

/// Reads `value[key]` as a string, raising a [`DecodeError`] if present
/// but not a string. `None` when `key` is absent or `null`.
fn dict_str(what: &str, value: &Value, key: &str) -> Result<Option<String>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| DecodeError(format!("{what}: {key:?} must be a string"))),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an `f32`, raising a [`DecodeError`] if present but
/// not a number. `None` when `key` is absent or `null`.
fn dict_f32(what: &str, value: &Value, key: &str) -> Result<Option<f32>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| DecodeError(format!("{what}: {key:?} must be a number"))),
        None => Ok(None),
    }
}

/// Reads `value[key]` as a `bool`, raising a [`DecodeError`] if present but
/// not a bool. `None` when `key` is absent or `null`.
fn dict_bool(what: &str, value: &Value, key: &str) -> Result<Option<bool>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| DecodeError(format!("{what}: {key:?} must be a bool"))),
        None => Ok(None),
    }
}

/// Decodes one panel dict (`{"id", "icon", "tooltip", "title"}`, all but
/// `"id"` optional) into a [`PanelDefinition`] — one entry of `run_shell`'s
/// `"panels"`/`"bottom_items"` lists.
fn panel_definition_from_value(what: &str, value: &Value) -> Result<PanelDefinition, DecodeError> {
    let id = dict_required_str(what, value, "id")?;
    let icon = dict_str(what, value, "icon")?.unwrap_or_default();
    let tooltip = dict_str(what, value, "tooltip")?.unwrap_or_default();
    let title = dict_str(what, value, "title")?.unwrap_or_else(|| id.clone());
    Ok(PanelDefinition {
        id: WidgetId::new(id),
        icon,
        tooltip,
        title,
    })
}

/// Decodes a `list` of panel dicts via [`panel_definition_from_value`].
fn panel_list_from_value(what: &str, value: &Value) -> Result<Vec<PanelDefinition>, DecodeError> {
    let list = value
        .as_array()
        .ok_or_else(|| DecodeError(format!("{what}: panels must be a list")))?;
    list.iter()
        .map(|item| {
            if !item.is_object() {
                return Err(DecodeError(format!("{what}: each panel must be a dict")));
            }
            panel_definition_from_value(what, item)
        })
        .collect()
}

/// [`panel_list_from_value`] applied to `value[key]`, defaulting to an
/// empty list when `key` is absent or `null`.
fn panel_list_from_dict(
    what: &str,
    value: &Value,
    key: &str,
) -> Result<Vec<PanelDefinition>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => panel_list_from_value(what, v),
        None => Ok(Vec::new()),
    }
}

/// Decodes one bottom-panel-tab dict (`{"id", "label", "closable",
/// "badge"}`, all but `"id"` optional) into a [`BottomPanelTab`], with its
/// content set to a [`StaticTabContent`] naming the tab's own id.
fn bottom_panel_tab_from_value(what: &str, value: &Value) -> Result<BottomPanelTab, DecodeError> {
    let id = dict_required_str(what, value, "id")?;
    let label = dict_str(what, value, "label")?.unwrap_or_else(|| id.clone());
    let closable = dict_bool(what, value, "closable")?.unwrap_or(false);
    let badge = dict_str(what, value, "badge")?;
    Ok(BottomPanelTab {
        content: Box::new(StaticTabContent(id.clone())),
        id,
        label,
        closable,
        badge,
    })
}

/// Decodes `run_shell`'s `"bottom_panel"` dict (`{"tabs", "active_tab_id",
/// "maximised", "height_fraction"}`, all optional except at least one tab)
/// into a [`BottomPanelConfig`]. `"active_tab_id"` falls back to the first
/// tab's id, mirroring [`BottomPanelConfig::active_tab_id`]'s own
/// "unknown id falls back to the first tab" contract.
fn bottom_panel_config_from_value(
    what: &str,
    value: &Value,
) -> Result<BottomPanelConfig, DecodeError> {
    let tabs = match obj_get(value, "tabs") {
        Some(v) => {
            let list = v.as_array().ok_or_else(|| {
                DecodeError(format!("{what}: bottom_panel \"tabs\" must be a list"))
            })?;
            list.iter()
                .map(|item| {
                    if !item.is_object() {
                        return Err(DecodeError(format!(
                            "{what}: each bottom_panel tab must be a dict"
                        )));
                    }
                    bottom_panel_tab_from_value(what, item)
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        None => Vec::new(),
    };
    let active_tab_id = dict_str(what, value, "active_tab_id")?
        .or_else(|| tabs.first().map(|tab| tab.id.clone()))
        .unwrap_or_default();
    let maximised = dict_bool(what, value, "maximised")?.unwrap_or(false);
    let height_fraction = dict_f32(what, value, "height_fraction")?.unwrap_or(0.3);
    Ok(BottomPanelConfig {
        tabs,
        active_tab_id,
        maximised,
        height_fraction,
    })
}

/// Decodes `run_shell`'s `config` dict into a [`ShellConfig`]. There is no
/// `Deserialize` impl on `ShellConfig`/`PanelDefinition`/`BottomPanelConfig`
/// upstream, so this reads the subset of `ShellConfig`'s builder surface a
/// binding supports field-by-field, the same `*_from_value` style every
/// other decoder in this module uses, rather than depending on one.
/// Recognized keys: `"title"` (required), `"panels"`, `"bottom_items"`,
/// `"position"` (`"left"`/`"right"`), `"sidebar_visible"`,
/// `"default_sidebar_width"`, `"min_sidebar_width"`, `"max_sidebar_width"`,
/// `"activity_bar_width"`, `"has_title_bar"` + `"title_bar_height_lh"`,
/// `"has_status_bar"`, `"has_command_line"`, `"has_bottom_panel"` +
/// `"bottom_panel_height_lh"` (the bare, untabbed region), and
/// `"bottom_panel"` (a tabbed [`BottomPanelConfig`], see
/// [`bottom_panel_config_from_value`]). Any other key is silently ignored.
fn shell_config_from_value(value: &Value) -> Result<ShellConfig, DecodeError> {
    let what = "run_shell";
    let title = dict_required_str(what, value, "title")?;
    let panels = panel_list_from_dict(what, value, "panels")?;
    let mut config = ShellConfig::new(title, panels);

    let bottom_items = panel_list_from_dict(what, value, "bottom_items")?;
    if !bottom_items.is_empty() {
        config = config.with_bottom_items(bottom_items);
    }

    if let Some(position) = dict_str(what, value, "position")? {
        let position = match position.as_str() {
            "left" => ShellPosition::Left,
            "right" => ShellPosition::Right,
            other => {
                return Err(DecodeError(format!(
                    "{what}: \"position\" must be \"left\" or \"right\", got {other:?}"
                )));
            }
        };
        config = config.with_position(position);
    }

    if let Some(visible) = dict_bool(what, value, "sidebar_visible")? {
        config = config.with_sidebar_visible(visible);
    }
    if let Some(width) = dict_f32(what, value, "default_sidebar_width")? {
        config.default_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "min_sidebar_width")? {
        config.min_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "max_sidebar_width")? {
        config.max_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "activity_bar_width")? {
        config = config.with_activity_bar_width(width);
    }
    if dict_bool(what, value, "has_title_bar")?.unwrap_or(false) {
        let height = dict_f32(what, value, "title_bar_height_lh")?.unwrap_or(1.5);
        config = config.with_title_bar(height);
    }
    if dict_bool(what, value, "has_status_bar")?.unwrap_or(false) {
        config = config.with_status_bar();
    }
    if dict_bool(what, value, "has_command_line")?.unwrap_or(false) {
        config = config.with_command_line();
    }
    if dict_bool(what, value, "has_bottom_panel")?.unwrap_or(false) {
        let height = dict_f32(what, value, "bottom_panel_height_lh")?.unwrap_or(10.0);
        config = config.with_bottom_panel(height);
    }
    if let Some(bp_value) = obj_get(value, "bottom_panel") {
        if !bp_value.is_object() {
            return Err(DecodeError(format!(
                "{what}: \"bottom_panel\" must be a dict"
            )));
        }
        config = config.with_bottom_panel_config(bottom_panel_config_from_value(what, bp_value)?);
    }

    Ok(config)
}

/// Decodes `run_shell`'s `config` dict from `deserializer` into a
/// [`ShellConfig`] — see [`shell_config_from_value`] for the recognized
/// key vocabulary.
pub fn decode_shell_config<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ShellConfig, DecodeError> {
    let value = Value::deserialize(deserializer)
        .map_err(|err| DecodeError(format!("run_shell: invalid descriptor: {err}")))?;
    shell_config_from_value(&value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn decode(value: &Value) -> Result<LayoutNode, DecodeError> {
        decode_layout_node("test", value)
    }

    /// Unwraps a `DecodeError`, panicking on an `Ok` -- the `#[cfg(test)]`
    /// counterpart of `Result::expect_err`, which the `Ok` types below
    /// (`LayoutNode`, `RenderOutput`, `Rc<dyn ExtraPaint>`) don't derive
    /// `Debug` for.
    fn err<T>(result: Result<T, DecodeError>) -> DecodeError {
        match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e,
        }
    }

    /// A `"split"` node round-trips its id, orientation and ratio, and
    /// recursively decodes its two named panes.
    #[test]
    fn round_trips_a_split_node() {
        let value = json!({
            "split": {
                "id": "s1",
                "orientation": "vertical",
                "ratio": 0.4,
                "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                "second": {"id": "b", "button": {"id": "b", "label": "B"}},
            }
        });
        let node = decode(&value).expect("valid split");
        let LayoutNode::Split(split) = node else {
            panic!("expected a split node");
        };
        assert_eq!(split.split.id.as_str(), "s1");
        assert_eq!(split.split.direction, SplitDirection::Vertical);
        assert_eq!(split.split.ratio, 0.4);
        assert!(matches!(*split.first, LayoutNode::Leaf(_)));
        assert!(matches!(*split.second, LayoutNode::Leaf(_)));
    }

    /// A `"tabs"` node marks the `"selected"` entry active and carries
    /// every other tab's content un-selected.
    #[test]
    fn round_trips_a_tabs_node() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "selected": 1,
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                    {"id": "two", "title": "Two", "content": {"id": "two-leaf", "text_display": {"id": "two-leaf", "lines": []}}},
                ],
            }
        });
        let node = decode(&value).expect("valid tabs");
        let LayoutNode::Tabs(tabs) = node else {
            panic!("expected a tabs node");
        };
        assert_eq!(tabs.selected, 1);
        assert_eq!(tabs.tab_bar.tabs.len(), 2);
        assert!(!tabs.tab_bar.tabs[0].is_active);
        assert!(tabs.tab_bar.tabs[1].is_active);
        assert_eq!(tabs.entries.len(), 2);
    }

    /// A `"panel"` node decodes its chrome generically and recurses into
    /// its required `"content"` child.
    #[test]
    fn round_trips_a_panel_node() {
        let value = json!({
            "panel": {
                "id": "p1",
                "content": {"id": "p1-leaf", "button": {"id": "p1-leaf", "label": "Go"}},
            }
        });
        let node = decode(&value).expect("valid panel");
        let LayoutNode::Panel(panel) = node else {
            panic!("expected a panel node");
        };
        assert_eq!(panel.panel.id.as_str(), "p1");
        assert!(matches!(*panel.content, LayoutNode::Leaf(_)));
    }

    /// A `"sidebar_panel"` node decodes the same way a `"panel"` node does.
    #[test]
    fn round_trips_a_sidebar_panel_node() {
        let value = json!({
            "sidebar_panel": {
                "id": "sb1",
                "content": {"id": "sb1-leaf", "button": {"id": "sb1-leaf", "label": "Go"}},
            }
        });
        let node = decode(&value).expect("valid sidebar_panel");
        let LayoutNode::SidebarPanel(sidebar) = node else {
            panic!("expected a sidebar_panel node");
        };
        assert_eq!(sidebar.sidebar_panel.id.as_str(), "sb1");
        assert!(matches!(*sidebar.content, LayoutNode::Leaf(_)));
    }

    /// A `"split_tree"` node builds a `SplitTree` geometry with one leaf
    /// per `{"leaf": ...}` entry, in pre-order.
    #[test]
    fn round_trips_a_split_tree_node() {
        let value = json!({
            "split_tree": {
                "id": "st1",
                "root": {
                    "split": {
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "first": {"leaf": {"id": "a", "button": {"id": "a", "label": "A"}}},
                        "second": {"leaf": {"id": "b", "button": {"id": "b", "label": "B"}}},
                    }
                },
            }
        });
        let node = decode(&value).expect("valid split_tree");
        let LayoutNode::SplitTree(split_tree) = node else {
            panic!("expected a split_tree node");
        };
        assert_eq!(split_tree.id, "st1");
        assert_eq!(split_tree.tree.leaf_count(), 2);
        assert_eq!(split_tree.tree.split_count(), 1);
        assert_eq!(split_tree.leaves.len(), 2);
    }

    /// A split node missing its required `"id"` key is rejected.
    #[test]
    fn rejects_a_split_missing_id() {
        let value = json!({
            "split": {
                "orientation": "horizontal",
                "ratio": 0.5,
                "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                "second": {"id": "b", "button": {"id": "b", "label": "B"}},
            }
        });
        let err = err(decode(&value));
        assert!(err.0.contains("is missing required \"id\""), "{}", err.0);
    }

    /// A render callback tree with two leaves sharing the same id is
    /// rejected once [`validate_unique_leaf_ids`] walks it.
    #[test]
    fn rejects_duplicate_leaf_ids() {
        let value = json!({
            "row": [
                {"id": "dup", "button": {"id": "dup", "label": "A"}},
                {"id": "dup", "button": {"id": "dup", "label": "B"}},
            ]
        });
        let output = err(decode_render_output("render", &value));
        assert!(output.0.contains("duplicate leaf id"), "{}", output.0);
    }

    /// A split's `"orientation"` must be `"horizontal"` or `"vertical"`.
    #[test]
    fn rejects_a_bad_orientation() {
        let value = json!({
            "split": {
                "id": "s1",
                "orientation": "diagonal",
                "ratio": 0.5,
                "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                "second": {"id": "b", "button": {"id": "b", "label": "B"}},
            }
        });
        let err = err(decode(&value));
        assert!(err.0.contains("must be one of"), "{}", err.0);
    }

    /// A tabs node's `"selected"` index must be in range for its tab list.
    #[test]
    fn rejects_an_out_of_range_selected_index() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "selected": 5,
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "button": {"id": "one-leaf", "label": "A"}}},
                ],
            }
        });
        let err = err(decode(&value));
        assert!(err.0.contains("out of range"), "{}", err.0);
    }

    /// A leaf dict naming two descriptor kinds at once is rejected.
    #[test]
    fn rejects_a_leaf_with_two_descriptor_kinds() {
        let value = json!({
            "id": "leaf1",
            "button": {"id": "leaf1", "label": "A"},
            "list": {"id": "leaf1", "items": []},
        });
        let err = err(decode(&value));
        assert!(err.0.contains("more than one descriptor kind"), "{}", err.0);
    }

    /// An unregistered additive primitive kind is rejected with every
    /// registered kind named in the error.
    #[test]
    fn rejects_an_unknown_extra_primitive_kind() {
        let err = err(crate::extra_paint::decode_extra_primitive(
            "test",
            "not_a_real_kind",
            json!({}),
        ));
        assert!(
            err.0.contains("unknown additive primitive kind"),
            "{}",
            err.0
        );
    }
}
