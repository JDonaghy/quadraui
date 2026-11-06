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
//! [`materialize`]s the whole input into one [`serde_json::Value`] tree,
//! then dispatches against that uniform shape — the dynamically-keyed,
//! "which of these N keys is present" dict shapes a layout tree node or
//! `ShellConfig` decodes from don't map onto a single `#[derive(Deserialize)]`
//! struct, but a `serde_json::Value` is cheap to re-inspect by key as many
//! times as the recursive decode needs. [`materialize`] differs from
//! `Value`'s own `Deserialize` impl in exactly two ways — see its own doc —
//! so a byte string and a non-finite float each still round-trip (or fail
//! loudly) the way a decode straight off the original `Deserializer`
//! would, instead of silently losing information once `Value` is in the
//! way. `host::merge_overrides` uses the same crate but its own,
//! unmodified `Value::deserialize` — its medium is always a backend's own
//! `Theme`/`Style`, already round-tripped through `serde_json` once on the
//! way in, never raw binding input.

use std::collections::HashMap;
use std::rc::Rc;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use serde_json::Value;

use quadraui::{
    Backend, BackendWidget, BottomPanelConfig, BottomPanelTab, Color, Panel, PanelDefinition, Rect,
    ShellConfig, ShellPosition, SidebarPanel, Split, SplitDirection, SplitTree, TabBar,
    TabBarSegment, TabItem, WidgetId,
};

use crate::extra_paint::{decode_extra_primitive, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
use crate::host::text_display_from_lines;
use crate::layout::{
    validate_unique_leaf_ids, EdgeInsets, LayoutContainer, LayoutLeaf, LayoutNode, LayoutPanel,
    LayoutSidebarPanel, LayoutSplit, LayoutSplitTree, LayoutTabEntry, LayoutTabs, LeafDescriptor,
    Spacing, LEAF_KIND_KEYS,
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

// ─── Materializing the input into a `Value` tree ───────────────────────────

/// Materializes `deserializer`'s whole input into one [`serde_json::Value`]
/// tree — almost `Value::deserialize`, except for the two deltas that keep
/// a decoder below byte-identical to one that decoded straight off the
/// original `Deserializer`, with no `Value` hop in between:
/// - a byte string (Python `bytes`/`bytearray`, which `pythonize` routes to
///   `deserialize_bytes`/`visit_bytes`) becomes a [`Value::Array`] of `u8`
///   numbers — the same shape `Vec<u8>`'s own default `Deserialize` impl
///   produces from a byte string via `deserialize_seq`, so
///   `quadraui::ImageSource::Bytes(Vec<u8>)` keeps decoding through
///   `{"Bytes": <bytes>}` rather than being rejected as "invalid
///   descriptor" by `serde_json::Value`'s own visitor (which has no
///   `visit_bytes` override).
/// - a non-finite float (`NaN`/`inf`/`-inf`) is rejected outright instead
///   of silently becoming [`Value::Null`] the way `serde_json`'s own
///   visitor does (`Number::from_f64` returns `None` for a non-finite
///   value, falling back to `Value::Null`) — [`obj_get`]'s "a `null` entry
///   means the key is absent" convention would otherwise alias a
///   non-finite value with a field the caller never set.
///
/// One accepted narrowing remains versus decoding straight off the
/// original `Deserializer`: every object key becomes a `String` (matching
/// `serde_json::Value::Object`'s own `Map<String, Value>` shape), so a
/// non-string dict key is rejected and a string-spelled-integer key (e.g.
/// `"3"`) is indistinguishable from the integer `3` once both land in the
/// same `Value::Object`. The in-tree callers of this module key every
/// dynamically-typed dict by string already (`"id"`, `"split"`, ...), and
/// `serde_json::Value` has no representation for a non-string key, so
/// fully preserving the original key's type would need abandoning
/// `Value` as the dispatch medium.
fn materialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    deserializer.deserialize_any(ValueVisitor)
}

/// [`DeserializeSeed`] wrapper so [`ValueVisitor::visit_seq`]/`visit_map`
/// can recurse into [`materialize`]'s own visitor for each element/value —
/// `SeqAccess::next_element_seed`/`MapAccess::next_value_seed` take a seed,
/// not a bare `Visitor`.
struct ValueSeed;

impl<'de> DeserializeSeed<'de> for ValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        materialize(deserializer)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a value decodable into a descriptor wire tree")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        if !v.is_finite() {
            return Err(E::custom(format!("non-finite number {v} is not supported")));
        }
        Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        materialize(deserializer)
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Value, E> {
        Ok(Value::Array(
            v.iter().map(|byte| Value::Number((*byte).into())).collect(),
        ))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut vec = Vec::new();
        while let Some(v) = seq.next_element_seed(ValueSeed)? {
            vec.push(v);
        }
        Ok(Value::Array(vec))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut obj = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(ValueSeed)?;
            obj.insert(key, value);
        }
        Ok(Value::Object(obj))
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
        Err(DecodeError(format!("{what}: {suffix} must be an object")))
    }
}

/// Reads `value[key]` as a required sub-dict, raising a [`DecodeError`]
/// from `context` (a caller-supplied `"{what}: <label>"` prefix) if it is
/// missing or not a dict.
fn required_dict<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a Value, DecodeError> {
    match obj_get(value, key) {
        Some(v) if v.is_object() => Ok(v),
        Some(_) => Err(DecodeError(format!("{context} {key:?} must be an object"))),
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
            .ok_or_else(|| DecodeError(format!("{context} {key:?} must be an array"))),
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
    let value = materialize(deserializer)
        .map_err(|err| DecodeError(format!("{what}: invalid descriptor: {err}")))?;
    layout_node_from_value(what, &value)
}

fn layout_node_from_value(what: &str, value: &Value) -> Result<LayoutNode, DecodeError> {
    if let Some(v) = obj_get(value, "row") {
        let children = layout_children_from_value(what, v)?;
        let spacing = spacing_from_value(what, value, "\"row\"", true)?;
        return Ok(LayoutNode::Row(LayoutContainer { children, spacing }));
    }
    if let Some(v) = obj_get(value, "column") {
        let children = layout_children_from_value(what, v)?;
        let spacing = spacing_from_value(what, value, "\"column\"", true)?;
        return Ok(LayoutNode::Column(LayoutContainer { children, spacing }));
    }
    if let Some(v) = obj_get(value, "split") {
        // `gap` is meaningful here -- the space between the split's two
        // panes -- unlike `"tabs"`/`"panel"` below.
        let spacing = spacing_from_value(what, value, "\"split\"", true)?;
        let split_value = must_be_dict(what, v, "\"split\"")?;
        return Ok(LayoutNode::Split(layout_split_from_value(
            what,
            split_value,
            spacing,
        )?));
    }
    if let Some(v) = obj_get(value, "tabs") {
        let spacing = spacing_from_value(what, value, "\"tabs\"", false)?;
        let tabs_value = must_be_dict(what, v, "\"tabs\"")?;
        return Ok(LayoutNode::Tabs(layout_tabs_from_value(
            what, tabs_value, spacing,
        )?));
    }
    if let Some(v) = obj_get(value, "panel") {
        let spacing = spacing_from_value(what, value, "\"panel\"", false)?;
        let panel_value = must_be_dict(what, v, "\"panel\"")?;
        return Ok(LayoutNode::Panel(layout_panel_from_value(
            what,
            panel_value,
            spacing,
        )?));
    }
    if let Some(v) = obj_get(value, "sidebar_panel") {
        reject_unsupported_spacing_keys(what, value, "a \"sidebar_panel\" node")?;
        let sidebar_value = must_be_dict(what, v, "\"sidebar_panel\"")?;
        return Ok(LayoutNode::SidebarPanel(layout_sidebar_panel_from_value(
            what,
            sidebar_value,
        )?));
    }
    if let Some(v) = obj_get(value, "split_tree") {
        reject_unsupported_spacing_keys(what, value, "a \"split_tree\" node")?;
        let split_tree_value = must_be_dict(what, v, "\"split_tree\"")?;
        return Ok(LayoutNode::SplitTree(layout_split_tree_from_value(
            what,
            split_tree_value,
        )?));
    }
    Ok(LayoutNode::Leaf(layout_leaf_from_value(what, value)?))
}

/// Rejects a `"padding"`/`"margin"`/`"gap"` spacing key found alongside a
/// node `label` names, for a node kind that doesn't honour those keys yet
/// — `"sidebar_panel"` and `"split_tree"` are the only ones left; every
/// other node kind decodes spacing via [`spacing_from_value`] instead.
/// Without this, `{"sidebar_panel": {...}, "padding": 12}` would decode
/// successfully and silently drop the key, which is a worse failure mode
/// for an optional styling key than a decode error naming exactly what was
/// ignored.
fn reject_unsupported_spacing_keys(
    what: &str,
    value: &Value,
    label: &str,
) -> Result<(), DecodeError> {
    for key in ["padding", "margin", "gap"] {
        if obj_get(value, key).is_some() {
            return Err(DecodeError(format!(
                "{what}: {label} does not support \"{key}\" yet -- only \"row\"/\"column\"/\"split\"/\"tabs\"/\"panel\" nodes do"
            )));
        }
    }
    Ok(())
}

/// Decodes a `"row"`/`"column"` container's children: a `list` of child
/// dicts, each recursively decoded by [`layout_node_from_value`]. Accepts
/// any `[Value::Array]`-shaped input, not only a `list` in the strict
/// Python-type sense — `materialize`'s own `visit_seq` already folds a
/// Python `tuple`, `set`, or `frozenset` into the same `Value::Array`
/// shape, so a tuple or (nondeterministically ordered) set of children is
/// accepted here too. Tightening that back to "a real `list`" needs a
/// pyo3-aware check this crate can't make; it would live in a binding's
/// own decode adapter instead.
fn layout_children_from_value(what: &str, value: &Value) -> Result<Vec<LayoutNode>, DecodeError> {
    let list = value
        .as_array()
        .ok_or_else(|| DecodeError(format!("{what}: row/column children must be an array")))?;
    list.iter()
        .map(|child| {
            if !child.is_object() {
                return Err(DecodeError(format!(
                    "{what}: each row/column child must be an object"
                )));
            }
            layout_node_from_value(what, child)
        })
        .collect()
}

/// Decodes a layout node's optional `"padding"`/`"margin"`/`"gap"` spacing
/// keys — siblings of the node's own key (`"row"`, `"split"`, ...) in the
/// same node dict, e.g. `{"row": [...], "padding": 12, "gap": 8}`. Every
/// key is optional and defaults to [`Spacing::default`], so an existing
/// dict with none of them decodes to exactly the same (no-op) spacing as
/// before this existed. `allow_gap` is false for a node kind where `"gap"`
/// has no meaning (`"tabs"`/`"panel"` — there's no second sibling to space
/// it from); `"gap"` present there is a decode error naming the key,
/// rather than a silent no-op.
fn spacing_from_value(
    what: &str,
    value: &Value,
    label: &str,
    allow_gap: bool,
) -> Result<Spacing, DecodeError> {
    let context = format!("{what}: {label}");
    if !allow_gap {
        if let Some(_gap) = obj_get(value, "gap") {
            return Err(DecodeError(format!(
                "{context} does not support \"gap\" -- only \"row\"/\"column\"/\"split\" nodes do"
            )));
        }
    }
    let padding = match obj_get(value, "padding") {
        Some(v) => edge_insets_from_value(v, &context)?,
        None => EdgeInsets::default(),
    };
    let margin = opt_f32(value, "margin", &context)?.unwrap_or(0.0);
    let gap = if allow_gap {
        opt_f32(value, "gap", &context)?.unwrap_or(0.0)
    } else {
        0.0
    };
    Ok(Spacing {
        padding,
        margin,
        gap,
    })
}

/// Decodes a `"padding"` value: a bare number (the same inset on every
/// side), an `{"x": .., "y": ..}` axis pair, or a full `{"top": ..,
/// "right": .., "bottom": .., "left": ..}` shape. Every key inside either
/// dict shape is itself optional and defaults to `0.0` — but an object
/// shape rejects any key outside the six named above (a typo like
/// `{"lef": 4}` is a decode error, not a silently-zero `left`) and
/// rejects mixing the two shapes (`{"x": 6, "top": 3}` is a decode error
/// instead of silently dropping `top`).
fn edge_insets_from_value(value: &Value, context: &str) -> Result<EdgeInsets, DecodeError> {
    if let Some(n) = value.as_f64() {
        return Ok(EdgeInsets::uniform(n as f32));
    }
    if let Some(obj) = value.as_object() {
        const KNOWN_KEYS: [&str; 6] = ["x", "y", "top", "right", "bottom", "left"];
        for key in obj.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                return Err(DecodeError(format!(
                    "{context} \"padding\" has unknown key {key:?} -- expected one of \"x\"/\"y\"/\"top\"/\"right\"/\"bottom\"/\"left\""
                )));
            }
        }
        let x = opt_f32(value, "x", context)?;
        let y = opt_f32(value, "y", context)?;
        let top = opt_f32(value, "top", context)?;
        let right = opt_f32(value, "right", context)?;
        let bottom = opt_f32(value, "bottom", context)?;
        let left = opt_f32(value, "left", context)?;
        let has_axis = x.is_some() || y.is_some();
        let has_sides = top.is_some() || right.is_some() || bottom.is_some() || left.is_some();
        if has_axis && has_sides {
            return Err(DecodeError(format!(
                "{context} \"padding\" cannot mix \"x\"/\"y\" with \"top\"/\"right\"/\"bottom\"/\"left\" in the same object"
            )));
        }
        if has_axis {
            return Ok(EdgeInsets::xy(x.unwrap_or(0.0), y.unwrap_or(0.0)));
        }
        return Ok(EdgeInsets {
            top: top.unwrap_or(0.0),
            right: right.unwrap_or(0.0),
            bottom: bottom.unwrap_or(0.0),
            left: left.unwrap_or(0.0),
        });
    }
    Err(DecodeError(format!(
        "{context} \"padding\" must be a number, or an object with \"x\"/\"y\" or \"top\"/\"right\"/\"bottom\"/\"left\""
    )))
}

/// Decodes a `"split"` node: `{"id", "orientation", "ratio", "first",
/// "second"}`, with `"first_min"`/`"second_min"` optional (defaulting to
/// `0.0`, a portable line count — [`crate::render::paint_layout`]'s
/// `LayoutNode::Split` arm scales them by the backend's line height before
/// painting). `"first"`/`"second"` are full child node dicts, recursively
/// decoded by [`layout_node_from_value`]. `spacing` is the node's own
/// `"padding"`/`"margin"`/`"gap"` siblings, already decoded by
/// [`spacing_from_value`] in the caller (its `"gap"` is the space between
/// the two panes).
fn layout_split_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
) -> Result<LayoutSplit, DecodeError> {
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
        spacing,
    })
}

/// Decodes a `"panel"` node: the chrome dict decoded generically into a
/// [`Panel`] (`title`/`actions`/`accent`/`collapsed`, each `#[serde(default)]`,
/// so a minimal `{"id": ...}` is enough) plus a required `"content"` child
/// node, recursively decoded by [`layout_node_from_value`]. The `"content"`
/// key is harmless noise to the `Panel` decode itself — `Panel` has no
/// `deny_unknown_fields`. `spacing` is the node's own `"padding"`/`"margin"`
/// siblings, already decoded by [`spacing_from_value`] in the caller
/// (`"gap"` is rejected there — a panel has no second sibling to space it
/// from).
fn layout_panel_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
) -> Result<LayoutPanel, DecodeError> {
    let panel: Panel = decode_value(what, value)?;
    let id = panel.id.as_str().to_owned();
    let context = format!("{what}: panel {id:?}");
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(what, content_value)?;
    Ok(LayoutPanel {
        panel,
        content: Box::new(content),
        spacing,
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
/// when there are tabs to select. `spacing` is the node's own
/// `"padding"`/`"margin"` siblings, already decoded by
/// [`spacing_from_value`] in the caller (`"gap"` is rejected there — a tab
/// group has no second sibling to space it from).
fn layout_tabs_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
) -> Result<LayoutTabs, DecodeError> {
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
            return Err(DecodeError(format!("{context} each tab must be an object")));
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
    let focused_index = opt_usize(value, "focused_index", &context)?;
    if let Some(idx) = focused_index {
        if !items.is_empty() && idx >= items.len() {
            return Err(DecodeError(format!(
                "{context} \"focused_index\" {idx} is out of range for {len} tab(s)",
                len = items.len()
            )));
        }
    }
    let right_segments: Vec<TabBarSegment> = match obj_get(value, "right_segments") {
        Some(v) => {
            if !v.is_array() {
                return Err(DecodeError(format!(
                    "{context} \"right_segments\" must be an array"
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
        spacing,
        focused_index,
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
    let value = materialize(deserializer)
        .map_err(|err| DecodeError(format!("{what}: invalid descriptor: {err}")))?;
    if is_layout_tree(&value) {
        let node = layout_node_from_value(what, &value)?;
        validate_unique_leaf_ids(what, &node)?;
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
                    "{what}: {key:?} must be an object or null"
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
                    "{what}: {kind:?} must be an object or null"
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
        .ok_or_else(|| DecodeError(format!("{what}: panels must be an array")))?;
    list.iter()
        .map(|item| {
            if !item.is_object() {
                return Err(DecodeError(format!("{what}: each panel must be an object")));
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
                DecodeError(format!("{what}: bottom_panel \"tabs\" must be an array"))
            })?;
            list.iter()
                .map(|item| {
                    if !item.is_object() {
                        return Err(DecodeError(format!(
                            "{what}: each bottom_panel tab must be an object"
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
                "{what}: \"bottom_panel\" must be an object"
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
    let value = materialize(deserializer)
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

    /// A `"row"`'s `"padding"`/`"margin"`/`"gap"` keys are siblings of
    /// the `"row"` key in the same node dict, decoded into
    /// [`crate::layout::Spacing`] alongside the children. `"padding"` as
    /// a bare number applies uniformly to every edge.
    #[test]
    fn round_trips_a_row_node_s_spacing_keys() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
            "padding": 12,
            "margin": 4,
            "gap": 8,
        });
        let node = decode(&value).expect("valid row");
        let LayoutNode::Row(container) = node else {
            panic!("expected a row node");
        };
        assert_eq!(container.children.len(), 1);
        assert_eq!(container.spacing.padding, EdgeInsets::uniform(12.0));
        assert_eq!(container.spacing.margin, 4.0);
        assert_eq!(container.spacing.gap, 8.0);
    }

    /// `"padding"` also accepts an `{"x": .., "y": ..}` axis pair, applying
    /// `x` to the left/right edges and `y` to top/bottom.
    #[test]
    fn row_padding_accepts_an_x_y_axis_pair() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
            "padding": {"x": 6, "y": 2},
        });
        let node = decode(&value).expect("valid row");
        let LayoutNode::Row(container) = node else {
            panic!("expected a row node");
        };
        assert_eq!(container.spacing.padding, EdgeInsets::xy(6.0, 2.0));
    }

    /// A `"row"` with no spacing keys at all -- the shape every
    /// pre-existing render callback uses -- decodes to [`Spacing::default`]
    /// (no inset, no gap), unchanged from before spacing keys existed.
    #[test]
    fn row_with_no_spacing_keys_decodes_to_the_default_spacing() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        });
        let node = decode(&value).expect("valid row");
        let LayoutNode::Row(container) = node else {
            panic!("expected a row node");
        };
        assert_eq!(container.spacing, Spacing::default());
    }

    /// A `"panel"` node's `"padding"`/`"margin"` keys are siblings of the
    /// `"panel"` key itself, decoded into [`crate::layout::Spacing`]
    /// alongside the chrome/content.
    #[test]
    fn round_trips_a_panel_node_s_padding_and_margin() {
        let value = json!({
            "panel": {
                "id": "p1",
                "content": {"id": "p1-leaf", "button": {"id": "p1-leaf", "label": "Go"}},
            },
            "padding": 12,
            "margin": 4,
        });
        let node = decode(&value).expect("valid panel with spacing");
        let LayoutNode::Panel(panel) = node else {
            panic!("expected a panel node");
        };
        assert_eq!(panel.spacing.padding, EdgeInsets::uniform(12.0));
        assert_eq!(panel.spacing.margin, 4.0);
        assert_eq!(panel.spacing.gap, 0.0);
    }

    /// `"gap"` has no meaning on a `"panel"` node -- there's no second
    /// sibling to space it from -- so it's a decode error instead of a
    /// silent no-op, same as it was for `padding`/`margin` before they
    /// were supported.
    #[test]
    fn panel_rejects_a_gap_key_it_does_not_support() {
        let value = json!({
            "panel": {
                "id": "p1",
                "content": {"id": "p1-leaf", "button": {"id": "p1-leaf", "label": "Go"}},
            },
            "gap": 8,
        });
        let error = err(decode(&value));
        assert!(error.0.contains("gap"), "{}", error.0);
    }

    /// A `"tabs"` node's `"padding"`/`"margin"` keys decode the same way a
    /// `"panel"` node's do.
    #[test]
    fn round_trips_a_tabs_node_s_padding_and_margin() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                ],
            },
            "padding": 6,
            "margin": 2,
        });
        let node = decode(&value).expect("valid tabs with spacing");
        let LayoutNode::Tabs(tabs) = node else {
            panic!("expected a tabs node");
        };
        assert_eq!(tabs.spacing.padding, EdgeInsets::uniform(6.0));
        assert_eq!(tabs.spacing.margin, 2.0);
        assert_eq!(tabs.spacing.gap, 0.0);
    }

    /// `"gap"` has no meaning on a `"tabs"` node either.
    #[test]
    fn tabs_rejects_a_gap_key_it_does_not_support() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                ],
            },
            "gap": 8,
        });
        let error = err(decode(&value));
        assert!(error.0.contains("gap"), "{}", error.0);
    }

    /// A `"split"` node's `"padding"`/`"margin"`/`"gap"` keys all decode --
    /// unlike `"tabs"`/`"panel"`, `"gap"` has a real meaning here: the
    /// space between the two panes.
    #[test]
    fn round_trips_a_split_node_s_spacing_keys() {
        let value = json!({
            "split": {
                "id": "s1",
                "orientation": "horizontal",
                "ratio": 0.5,
                "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                "second": {"id": "b", "button": {"id": "b", "label": "B"}},
            },
            "padding": 10,
            "margin": 3,
            "gap": 6,
        });
        let node = decode(&value).expect("valid split with spacing");
        let LayoutNode::Split(split) = node else {
            panic!("expected a split node");
        };
        assert_eq!(split.spacing.padding, EdgeInsets::uniform(10.0));
        assert_eq!(split.spacing.margin, 3.0);
        assert_eq!(split.spacing.gap, 6.0);
    }

    /// A `"sidebar_panel"` node still rejects every spacing key -- only
    /// `"row"`/`"column"`/`"split"`/`"tabs"`/`"panel"` nodes support any
    /// of them.
    #[test]
    fn sidebar_panel_rejects_a_padding_key_it_does_not_support_yet() {
        let value = json!({
            "sidebar_panel": {
                "id": "sb1",
                "content": {"id": "sb1-leaf", "button": {"id": "sb1-leaf", "label": "Go"}},
            },
            "padding": 12,
        });
        let error = err(decode(&value));
        assert!(error.0.contains("padding"), "{}", error.0);
    }

    /// An unknown key inside a `"padding"` object (a typo like `"lef"`)
    /// is a decode error, not a silently-zero `left`.
    #[test]
    fn row_padding_rejects_an_unknown_object_key() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
            "padding": {"lef": 4},
        });
        let error = err(decode(&value));
        assert!(error.0.contains("lef"), "{}", error.0);
    }

    /// Mixing the `{"x", "y"}` axis shape with the `{"top", ...}` side
    /// shape in the same `"padding"` object is a decode error instead of
    /// silently dropping whichever side keys were present.
    #[test]
    fn row_padding_rejects_mixing_axis_and_side_keys() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
            "padding": {"x": 6, "top": 3},
        });
        let error = err(decode(&value));
        assert!(error.0.contains("mix"), "{}", error.0);
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

    /// A `"tabs"` node's optional `"focused_index"` key decodes to
    /// `LayoutTabs::focused_index`, independent of `"selected"` — a py/
    /// node app can keyboard-focus a different tab than the one whose
    /// content is currently shown.
    #[test]
    fn round_trips_a_tabs_node_s_focused_index() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "selected": 1,
                "focused_index": 0,
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
        assert_eq!(tabs.focused_index, Some(0));
        assert_eq!(tabs.selected, 1);
    }

    /// Omitting `"focused_index"` entirely decodes to `None` — no
    /// keyboard-focus treatment painted, matching every other optional
    /// `"tabs"` key's absent-means-default convention.
    #[test]
    fn tabs_node_focused_index_defaults_to_none() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                ],
            }
        });
        let node = decode(&value).expect("valid tabs");
        let LayoutNode::Tabs(tabs) = node else {
            panic!("expected a tabs node");
        };
        assert_eq!(tabs.focused_index, None);
    }

    /// A `"focused_index"` past the end of `"tabs"` is a decode error, the
    /// same out-of-range convention `"selected"` already uses.
    #[test]
    fn tabs_node_rejects_an_out_of_range_focused_index() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "focused_index": 5,
                "tabs": [
                    {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                ],
            }
        });
        let error = err(decode(&value));
        assert!(error.0.contains("focused_index"), "{}", error.0);
    }

    /// `"focused_index": 0` on an empty `"tabs"` array decodes without
    /// error — the same `!items.is_empty() && idx >= items.len()` escape
    /// hatch `"selected"` already uses two blocks up, so a `0` default
    /// payload for an as-yet-empty tab group doesn't trip the range check
    /// before there's anything to be out of range of.
    #[test]
    fn tabs_node_allows_a_zero_focused_index_on_an_empty_tab_list() {
        let value = json!({
            "tabs": {
                "id": "t1",
                "focused_index": 0,
                "tabs": [],
            }
        });
        let node = decode(&value).expect("valid tabs");
        let LayoutNode::Tabs(tabs) = node else {
            panic!("expected a tabs node");
        };
        assert_eq!(tabs.focused_index, Some(0));
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

    /// A minimal `Deserializer` whose `deserialize_any` calls exactly one
    /// visitor method -- standing in for a slice of `pythonize`'s own
    /// `Depythonizer` dispatch (a Python `bytes`/`bytearray` always
    /// routes to `deserialize_bytes`/`visit_bytes`) without needing a
    /// live Python interpreter to drive it.
    struct BytesDeserializer<'a>(&'a [u8]);

    impl<'de> Deserializer<'de> for BytesDeserializer<'_> {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_bytes(self.0)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct enum identifier ignored_any
        }
    }

    /// A minimal `Deserializer` whose `deserialize_any` calls
    /// `visit_f64` directly -- standing in for `pythonize`'s own
    /// `deserialize_f64`/`visit_f64` routing for a Python `float`.
    struct F64Deserializer(f64);

    impl<'de> Deserializer<'de> for F64Deserializer {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_f64(self.0)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct enum identifier ignored_any
        }
    }

    /// A Python `bytes`/`bytearray` value -- which `pythonize` routes
    /// through `deserialize_bytes`/`visit_bytes`, not `deserialize_seq`
    /// -- materializes into the same array-of-numbers shape `Vec<u8>`'s
    /// own `Deserialize` impl expects from a `deserialize_seq` call,
    /// rather than being rejected (the regression this module's own
    /// `materialize`, not `serde_json::Value`'s `Deserialize` impl, is
    /// written to avoid: `serde_json::Value`'s visitor has no
    /// `visit_bytes` override at all).
    #[test]
    fn materializes_bytes_as_a_number_array() {
        let value = materialize(BytesDeserializer(&[1, 2, 3])).expect("bytes materialize");
        assert_eq!(value, json!([1, 2, 3]));
    }

    /// A non-finite float (`NaN`/`inf`/`-inf`) is rejected outright during
    /// materialization rather than silently becoming `Value::Null` --
    /// which `obj_get`'s "a `null` entry means the key is absent"
    /// convention would otherwise alias with a field the caller never
    /// set.
    #[test]
    fn rejects_a_non_finite_float() {
        let result = materialize(F64Deserializer(f64::NAN));
        assert!(result.is_err());
    }

    /// An image leaf's `"Bytes"` source decodes from the same
    /// array-of-numbers shape `materialize` produces for a Python `bytes`
    /// value (see `materializes_bytes_as_a_number_array`) -- the full
    /// decode path the fix in this module protects end to end.
    #[test]
    fn decodes_an_image_leaf_with_byte_source() {
        let value = json!({
            "id": "img1",
            "source": {"Bytes": [1, 2, 3]},
            "intrinsic_size": null,
            "fit": "Contain",
            "fallback_text": "",
        });
        crate::extra_paint::decode_extra_primitive("test", "image", &value)
            .expect("valid image with byte source");
    }

    /// A render callback's flat (pre-layout-tree) `dict` shape decodes
    /// into a [`RenderFrame`]: a present slot decodes into its type, a
    /// missing or `null` slot leaves that frame slot empty.
    #[test]
    fn decodes_a_flat_render_frame() {
        let value = json!({
            "button": {"id": "b1", "label": "Go"},
            "list": null,
        });
        let output = decode_render_output("render", &value).expect("valid flat frame");
        let RenderOutput::Flat(frame) = output else {
            panic!("expected a flat frame");
        };
        assert!(frame.button.is_some());
        assert!(frame.list.is_none());
        assert!(frame.text_display.is_none());
    }

    /// `run_shell`'s `config` dict applies its documented defaults
    /// (`height_fraction` 0.3, `title_bar_height_lh` 1.5,
    /// `bottom_panel_height_lh` 10.0) when a flag is set but its paired
    /// height isn't given.
    #[test]
    fn decode_shell_config_applies_documented_defaults() {
        let value = json!({
            "title": "My Shell",
            "has_title_bar": true,
            "has_bottom_panel": true,
        });
        let config = decode_shell_config(&value).expect("valid shell config");
        assert_eq!(config.title, "My Shell");
        assert!(config.has_title_bar);
        assert_eq!(config.title_bar_height_lh, 1.5);
        assert!(config.has_bottom_panel);
        assert_eq!(config.bottom_panel_height_lh, 10.0);
    }

    /// `run_shell`'s `"position"` key accepts `"left"`/`"right"` and
    /// rejects anything else.
    #[test]
    fn decode_shell_config_validates_position() {
        let value = json!({"title": "My Shell", "position": "right"});
        let config = decode_shell_config(&value).expect("valid position");
        assert_eq!(config.position, ShellPosition::Right);

        let value = json!({"title": "My Shell", "position": "sideways"});
        let err = err(decode_shell_config(&value));
        assert!(err.0.contains("must be \"left\" or \"right\""), "{}", err.0);
    }

    /// `run_shell`'s tabbed `"bottom_panel"` dict applies its own
    /// documented default `height_fraction` (0.3) and falls back
    /// `"active_tab_id"` to the first tab when omitted.
    #[test]
    fn decode_shell_config_bottom_panel_defaults() {
        let value = json!({
            "title": "My Shell",
            "bottom_panel": {
                "tabs": [{"id": "t1"}, {"id": "t2"}],
            },
        });
        let config = decode_shell_config(&value).expect("valid bottom_panel");
        let bottom_panel = config.bottom_panel.expect("bottom_panel is set");
        assert_eq!(bottom_panel.active_tab_id, "t1");
        assert_eq!(bottom_panel.height_fraction, 0.3);
        assert_eq!(bottom_panel.tabs.len(), 2);
    }

    /// `run_shell`'s `config` dict requires `"title"`.
    #[test]
    fn decode_shell_config_requires_title() {
        let err = err(decode_shell_config(json!({})));
        assert!(err.0.contains("missing required \"title\""), "{}", err.0);
    }
}
