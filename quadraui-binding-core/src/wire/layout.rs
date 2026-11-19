//! Decodes the nine [`LAYOUT_NODE_KEYS`] container node kinds a layout
//! tree may hold — `"row"`/`"column"`/`"grid"`/`"split"`/`"tabs"`/
//! `"panel"`/`"sidebar_panel"`/`"split_tree"`/`"scroll"` — plus the `"style"`/
//! `"class"` wrapping and `"padding"`/`"margin"`/`"gap"`/`"justify"`/
//! `"align"` siblings every node dict may carry regardless of its own
//! kind. A dict matching none of these falls through to
//! [`super::leaf::layout_leaf_from_value`].

use serde_json::Value;

#[allow(deprecated)]
use quadraui::Panel;
use quadraui::{
    Color, ScrollAxis, SectionSize, SidebarPanel, Split, SplitDirection, SplitTree, TabBar,
    TabBarSegment, TabItem, WidgetId,
};

use quadraui::layout::GridTrack;

use crate::layout::{
    Align, EdgeInsets, Justify, LayoutChild, LayoutContainer, LayoutGrid, LayoutNode, LayoutPanel,
    LayoutScroll, LayoutSidebarPanel, LayoutSplit, LayoutSplitTree, LayoutStyled, LayoutTabEntry,
    LayoutTabs, Spacing,
};
use crate::{decode_value, DecodeError};

use super::frame::{all_node_kind_keys, unknown_node_kind_key};
use super::leaf::layout_leaf_from_value;
use super::style::{style_classes_from_value, style_override_from_value, StyleContext};
use super::value::{
    check_node_sibling_keys, check_unknown_keys, must_be_dict, obj_get, opt_bool, opt_f32,
    opt_usize, required_dict, required_f32, required_list, required_str, DecodeDiagnostics,
};

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
        _ => Err(DecodeError::unknown_variant(
            context,
            "orientation",
            orientation,
            &SPLIT_ORIENTATIONS,
        )),
    }
}

// ─── Layout tree decode ─────────────────────────────────────────────────────

/// The dispatch keys [`crate::wire::decode_layout_node`] checks, in check
/// order — a `"row"`/`"column"`/`"grid"`/`"split"`/`"tabs"`/`"panel"`/
/// `"sidebar_panel"`/`"split_tree"`/`"scroll"` node, else a leaf. Shared
/// with [`super::frame::is_layout_tree`]'s flat-vs-tree sniff so the two
/// can't drift onto different key lists.
pub const LAYOUT_NODE_KEYS: [&str; 9] = [
    "row",
    "column",
    "grid",
    "split",
    "tabs",
    "panel",
    "sidebar_panel",
    "split_tree",
    "scroll",
];

// Per-kind sibling-key vocabularies [`layout_node_from_value_inner`] passes
// to [`check_node_sibling_keys`] (`UNIVERSAL_NODE_SIBLING_KEYS` folds in
// automatically, so none of these repeats `"style"`/`"class"`/`"size"`/
// `"min"`/`"max"`/`"wire_version"`). Named constants, not inline literals
// at each call site, so this module's own `#[cfg(test)]`
// `node_shape_key_constants_match_the_wire_schema_field_list` test can
// cross-check every one against `wire_schema::schema()`'s own field list
// for that node kind: a field added on one side without the other fails
// that test instead of silently drifting into a false "unknown key"
// warning (or a strict-mode app's new hard rejection) the first time a real
// app sends it.
const ROW_SHAPE_KEYS: [&str; 7] = [
    "row", "padding", "margin", "gap", "justify", "align", "wrap",
];
const COLUMN_SHAPE_KEYS: [&str; 7] = [
    "column", "padding", "margin", "gap", "justify", "align", "wrap",
];
const GRID_SHAPE_KEYS: [&str; 3] = ["grid", "padding", "margin"];
const SPLIT_SHAPE_KEYS: [&str; 4] = ["split", "padding", "margin", "gap"];
const TABS_SHAPE_KEYS: [&str; 3] = ["tabs", "padding", "margin"];
const PANEL_SHAPE_KEYS: [&str; 3] = ["panel", "padding", "margin"];
const SIDEBAR_PANEL_SHAPE_KEYS: [&str; 1] = ["sidebar_panel"];
const SPLIT_TREE_SHAPE_KEYS: [&str; 1] = ["split_tree"];
const SCROLL_SHAPE_KEYS: [&str; 3] = ["scroll", "padding", "margin"];

/// Decodes one layout-tree node dict, then wraps it in a
/// [`LayoutNode::Styled`] if that same dict also carries a `"class"`
/// and/or `"style"` sibling key — see [`style_classes_from_value`] and
/// [`style_override_from_value`]. Every node kind dispatches through
/// [`layout_node_from_value_inner`] first, so both keys are handled
/// exactly once here rather than separately in each of that function's own
/// branches.
///
/// The two merge with inline `"style"` winning over the `"class"` list's
/// own left-to-right merge — [`StyleSets::resolve`]'s own precedence doc.
/// A node whose classes are all unknown and whose `"style"` is absent or
/// empty gets no wrapper at all, the same as a node carrying neither key.
pub(super) fn layout_node_from_value(
    what: &str,
    value: &Value,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutNode, DecodeError> {
    let node = layout_node_from_value_inner(what, value, styles, diagnostics)?;
    let classes = style_classes_from_value(what, value)?;
    let inline = style_override_from_value(what, value)?.unwrap_or_default();
    let style = inline.merged_over(styles.sets.resolve(&classes, styles.theme));
    if style.is_empty() {
        return Ok(node);
    }
    Ok(LayoutNode::Styled(LayoutStyled {
        style: Box::new(style),
        node: Box::new(node),
    }))
}

/// Decodes the node-kind-specific part of a layout-tree node dict --
/// [`layout_node_from_value`]'s own `"style"`-wrapping is applied around
/// this function's result, not inside it.
fn layout_node_from_value_inner(
    what: &str,
    value: &Value,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutNode, DecodeError> {
    if let Some(v) = obj_get(value, "row") {
        check_node_sibling_keys(diagnostics, what, "\"row\"", value, &ROW_SHAPE_KEYS)?;
        let children = layout_children_from_value(what, v, "\"row\"", styles, diagnostics)?;
        let spacing = spacing_from_value(what, value, "\"row\"", true)?;
        let justify = justify_from_value(what, value, "\"row\"")?;
        let align = align_from_value(what, value, "\"row\"")?;
        let wrap = wrap_from_value(what, value, "\"row\"")?;
        return Ok(LayoutNode::Row(LayoutContainer {
            children,
            spacing,
            justify,
            align,
            wrap,
        }));
    }
    if let Some(v) = obj_get(value, "column") {
        check_node_sibling_keys(diagnostics, what, "\"column\"", value, &COLUMN_SHAPE_KEYS)?;
        let children = layout_children_from_value(what, v, "\"column\"", styles, diagnostics)?;
        let spacing = spacing_from_value(what, value, "\"column\"", true)?;
        let justify = justify_from_value(what, value, "\"column\"")?;
        let align = align_from_value(what, value, "\"column\"")?;
        let wrap = wrap_from_value(what, value, "\"column\"")?;
        return Ok(LayoutNode::Column(LayoutContainer {
            children,
            spacing,
            justify,
            align,
            wrap,
        }));
    }
    if let Some(v) = obj_get(value, "grid") {
        check_node_sibling_keys(diagnostics, what, "\"grid\"", value, &GRID_SHAPE_KEYS)?;
        let spacing = spacing_from_value(what, value, "\"grid\"", false)?;
        let grid_value = must_be_dict(what, v, "grid")?;
        return Ok(LayoutNode::Grid(layout_grid_from_value(
            what,
            grid_value,
            spacing,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "split") {
        check_node_sibling_keys(diagnostics, what, "\"split\"", value, &SPLIT_SHAPE_KEYS)?;
        // `gap` is meaningful here -- the space between the split's two
        // panes -- unlike `"tabs"`/`"panel"` below.
        let spacing = spacing_from_value(what, value, "\"split\"", true)?;
        let split_value = must_be_dict(what, v, "split")?;
        return Ok(LayoutNode::Split(layout_split_from_value(
            what,
            split_value,
            spacing,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "tabs") {
        check_node_sibling_keys(diagnostics, what, "\"tabs\"", value, &TABS_SHAPE_KEYS)?;
        let spacing = spacing_from_value(what, value, "\"tabs\"", false)?;
        let tabs_value = must_be_dict(what, v, "tabs")?;
        return Ok(LayoutNode::Tabs(layout_tabs_from_value(
            what,
            tabs_value,
            spacing,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "panel") {
        check_node_sibling_keys(diagnostics, what, "\"panel\"", value, &PANEL_SHAPE_KEYS)?;
        let spacing = spacing_from_value(what, value, "\"panel\"", false)?;
        let panel_value = must_be_dict(what, v, "panel")?;
        return Ok(LayoutNode::Panel(layout_panel_from_value(
            what,
            panel_value,
            spacing,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "sidebar_panel") {
        reject_unsupported_spacing_keys(what, value, "a \"sidebar_panel\" node")?;
        check_node_sibling_keys(
            diagnostics,
            what,
            "\"sidebar_panel\"",
            value,
            &SIDEBAR_PANEL_SHAPE_KEYS,
        )?;
        let sidebar_value = must_be_dict(what, v, "sidebar_panel")?;
        return Ok(LayoutNode::SidebarPanel(layout_sidebar_panel_from_value(
            what,
            sidebar_value,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "split_tree") {
        reject_unsupported_spacing_keys(what, value, "a \"split_tree\" node")?;
        check_node_sibling_keys(
            diagnostics,
            what,
            "\"split_tree\"",
            value,
            &SPLIT_TREE_SHAPE_KEYS,
        )?;
        let split_tree_value = must_be_dict(what, v, "split_tree")?;
        return Ok(LayoutNode::SplitTree(layout_split_tree_from_value(
            what,
            split_tree_value,
            styles,
            diagnostics,
        )?));
    }
    if let Some(v) = obj_get(value, "scroll") {
        check_node_sibling_keys(diagnostics, what, "\"scroll\"", value, &SCROLL_SHAPE_KEYS)?;
        let spacing = spacing_from_value(what, value, "\"scroll\"", false)?;
        let scroll_value = must_be_dict(what, v, "scroll")?;
        return Ok(LayoutNode::Scroll(layout_scroll_from_value(
            what,
            scroll_value,
            spacing,
            styles,
            diagnostics,
        )?));
    }
    if let Some(key) = unknown_node_kind_key(value) {
        let expected = all_node_kind_keys();
        return Err(DecodeError::unknown_variant(
            what,
            "node kind",
            key,
            &expected,
        ));
    }
    Ok(LayoutNode::Leaf(layout_leaf_from_value(
        what,
        value,
        diagnostics,
    )?))
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
            return Err(DecodeError::borrow(
                what,
                format!(
                    "{label} does not support \"{key}\" yet -- only \"row\"/\"column\"/\"split\"/\"tabs\"/\"panel\"/\"scroll\" nodes do"
                ),
            ));
        }
    }
    Ok(())
}

/// Decodes a `"row"`/`"column"` container's children: a `list` of child
/// dicts, each recursively decoded by [`layout_node_from_value`] into a
/// [`LayoutChild`], picking up that child's own optional `"size"`/`"min"`/
/// `"max"` keys (siblings of whatever key names the child's own node kind,
/// in the same child dict) via [`child_size_from_value`]. Accepts any
/// `[Value::Array]`-shaped input, not only a `list` in the strict
/// Python-type sense — `materialize`'s own `visit_seq` already folds a
/// Python `tuple`, `set`, or `frozenset` into the same `Value::Array`
/// shape, so a tuple or (nondeterministically ordered) set of children is
/// accepted here too. Tightening that back to "a real `list`" needs a
/// pyo3-aware check this crate can't make; it would live in a binding's
/// own decode adapter instead.
///
/// `label` is the container's own key (`"row"`/`"column"`), used
/// the same way [`spacing_from_value`]/[`justify_from_value`]/
/// [`align_from_value`] already fold it into their own error paths --
/// a container-shape problem (this `list` itself, or one of its entries,
/// not being the shape expected) names the container it was found on,
/// not just the ancestor path leading to it.
fn layout_children_from_value(
    what: &str,
    value: &Value,
    label: &str,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<Vec<LayoutChild>, DecodeError> {
    let container_context = format!("{what} › {label}");
    let list = value
        .as_array()
        .ok_or_else(|| DecodeError::type_mismatch(&container_context, "children", "an array"))?;
    list.iter()
        .enumerate()
        .map(|(index, child)| {
            let context = format!("{container_context} › children[{index}]");
            if !child.is_object() {
                return Err(DecodeError::type_mismatch(
                    &container_context,
                    format!("children[{index}]"),
                    "an object",
                ));
            }
            let node = layout_node_from_value(&context, child, styles, diagnostics)?;
            let size = child_size_from_value(child, &context)?;
            let min = opt_f32(child, "min", &context)?;
            let max = opt_f32(child, "max", &context)?;
            Ok(LayoutChild {
                node,
                size,
                min,
                max,
            })
        })
        .collect()
}

/// Decodes a row/column child's optional `"size"` key: `"EqualShare"`/
/// `"Content"` (bare strings) or `{"Fixed": n}`/`{"Percent": p}`/
/// `{"Weight": w}` (object wrappers) — [`quadraui::SectionSize`]'s own
/// serde representation, the same wire shape `quadraui::MultiSectionView`'s
/// own per-section `"size"` key already uses, reused here rather than
/// inventing a second vocabulary for the same concept. Missing defaults to
/// [`SectionSize::default`] (`EqualShare`), matching every child's
/// pre-per-child-sizing behaviour. `{"Percent": p}` is additionally
/// range-checked to `0.0..=1.0` — `quadraui::SectionSize::Percent`'s own
/// documented "share of the original container" contract — so a plausible
/// typo like `{"Percent": 25}` (meaning "25 percent") is rejected here
/// rather than silently read as `2500%` and clamped to `100%` by
/// `crate::layout::resolve_main_axis_sizes`.
fn child_size_from_value(value: &Value, context: &str) -> Result<SectionSize, DecodeError> {
    let size = match obj_get(value, "size") {
        None => return Ok(SectionSize::default()),
        Some(v) => serde_json::from_value::<SectionSize>(v.clone())
            .map_err(|err| DecodeError::borrow(context, format!("\"size\": {err}")))?,
    };
    if let SectionSize::Percent(p) = size {
        if !(0.0..=1.0).contains(&p) {
            return Err(DecodeError::borrow(
                context,
                format!("\"size\": {{\"Percent\": ..}} must be between 0.0 and 1.0, got {p}"),
            ));
        }
    }
    Ok(size)
}

/// The lowercase, Python-idiomatic spellings `"justify"` accepts —
/// [`SPLIT_ORIENTATIONS`]'s own convention, independent of
/// [`crate::layout::Justify`]'s PascalCase Rust variant names.
const JUSTIFY_VALUES: [&str; 4] = ["start", "center", "end", "space_between"];

/// Decodes a `"row"`/`"column"` node's optional `"justify"` key — the
/// main-axis distribution of any leftover space once every child's size is
/// resolved. Missing defaults to [`Justify::default`] (`Start`), matching
/// every container's pre-`justify` behaviour.
fn justify_from_value(what: &str, value: &Value, label: &str) -> Result<Justify, DecodeError> {
    let context = format!("{what} › {label}");
    match obj_get(value, "justify") {
        None => Ok(Justify::default()),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| DecodeError::type_mismatch(&context, "justify", "a string"))?;
            match s {
                "start" => Ok(Justify::Start),
                "center" => Ok(Justify::Center),
                "end" => Ok(Justify::End),
                "space_between" => Ok(Justify::SpaceBetween),
                _ => Err(DecodeError::unknown_variant(
                    &context,
                    "justify",
                    s,
                    &JUSTIFY_VALUES,
                )),
            }
        }
    }
}

/// [`JUSTIFY_VALUES`]'s counterpart for `"align"`.
const ALIGN_VALUES: [&str; 4] = ["start", "center", "end", "stretch"];

/// Decodes a `"row"`/`"column"` node's optional `"align"` key — the
/// cross-axis treatment of each child. Missing defaults to
/// [`Align::default`] (`Stretch`), matching every container's
/// pre-`align` behaviour (every child filled the container's full
/// cross-axis extent).
fn align_from_value(what: &str, value: &Value, label: &str) -> Result<Align, DecodeError> {
    let context = format!("{what} › {label}");
    match obj_get(value, "align") {
        None => Ok(Align::default()),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| DecodeError::type_mismatch(&context, "align", "a string"))?;
            match s {
                "start" => Ok(Align::Start),
                "center" => Ok(Align::Center),
                "end" => Ok(Align::End),
                "stretch" => Ok(Align::Stretch),
                _ => Err(DecodeError::unknown_variant(
                    &context,
                    "align",
                    s,
                    &ALIGN_VALUES,
                )),
            }
        }
    }
}

/// Decodes a `"row"`/`"column"` node's optional `"wrap"` key. Missing
/// defaults to `false` — the pre-`wrap` behaviour, every child laid out on
/// a single line regardless of overflow.
fn wrap_from_value(what: &str, value: &Value, label: &str) -> Result<bool, DecodeError> {
    let context = format!("{what} › {label}");
    Ok(opt_bool(value, "wrap", &context)?.unwrap_or(false))
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
    let context = format!("{what} › {label}");
    if !allow_gap {
        if let Some(_gap) = obj_get(value, "gap") {
            return Err(DecodeError::borrow(
                &context,
                "does not support \"gap\" -- only \"row\"/\"column\"/\"split\" nodes do",
            ));
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
                return Err(DecodeError::unknown_key(
                    context,
                    "padding",
                    key.as_str(),
                    &KNOWN_KEYS,
                ));
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
            return Err(DecodeError::borrow(
                context,
                "\"padding\" cannot mix \"x\"/\"y\" with \"top\"/\"right\"/\"bottom\"/\"left\" in the same object",
            ));
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
    Err(DecodeError::borrow(
        context,
        "\"padding\" must be a number, or an object with \"x\"/\"y\" or \"top\"/\"right\"/\"bottom\"/\"left\"",
    ))
}
/// Decodes a `"split"` node: `{"id", "orientation", "ratio", "first",
/// "second"}`, with `"first_min"`/`"second_min"` optional (defaulting to
/// `0.0`, in the same portable surface-native-pixel unit [`Spacing`]'s own
/// fields use — [`crate::render::paint_layout`]'s `LayoutNode::Split` arm
/// quantises each into the backend's native unit on the axis it actually
/// bounds, `char_width` for a `Horizontal` split's width-bounding minimum
/// or `line_height` for a `Vertical` one's height-bounding minimum, right
/// before painting). `"first"`/`"second"` are full child node dicts, recursively
/// decoded by [`layout_node_from_value`]. `spacing` is the node's own
/// `"padding"`/`"margin"`/`"gap"` siblings, already decoded by
/// [`spacing_from_value`] in the caller (its `"gap"` is the space between
/// the two panes).
fn layout_split_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutSplit, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(what, "split.id", "a string"))?,
        None => return Err(DecodeError::missing_field(what, "split.id")),
    };
    let context = format!("{what} › split#{id}");
    check_unknown_keys(
        diagnostics,
        &context,
        "split",
        value,
        &[
            "id",
            "orientation",
            "ratio",
            "first_min",
            "second_min",
            "first",
            "second",
        ],
    )?;

    let orientation = required_str(value, "orientation", &context)?;
    let direction = split_direction(&context, &orientation)?;
    let ratio = required_f32(value, "ratio", &context)?;
    let first_min = opt_f32(value, "first_min", &context)?.unwrap_or(0.0);
    let second_min = opt_f32(value, "second_min", &context)?.unwrap_or(0.0);

    let first_value = required_dict(value, "first", &context)?;
    let first = layout_node_from_value(
        &format!("{context} › first"),
        first_value,
        styles,
        diagnostics,
    )?;
    let second_value = required_dict(value, "second", &context)?;
    let second = layout_node_from_value(
        &format!("{context} › second"),
        second_value,
        styles,
        diagnostics,
    )?;

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
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own wire-decode plumbing.
fn layout_panel_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutPanel, DecodeError> {
    let panel: Panel = decode_value(what, value)?;
    let id = panel.id.as_str().to_owned();
    let context = format!("{what} › panel#{id}");
    check_unknown_keys(
        diagnostics,
        &context,
        "panel",
        value,
        &["id", "title", "actions", "accent", "collapsed", "content"],
    )?;
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(
        &format!("{context} › content"),
        content_value,
        styles,
        diagnostics,
    )?;
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
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutSidebarPanel, DecodeError> {
    let sidebar_panel: SidebarPanel = decode_value(what, value)?;
    let id = sidebar_panel.id.as_str().to_owned();
    let context = format!("{what} › sidebar_panel#{id}");
    check_unknown_keys(
        diagnostics,
        &context,
        "sidebar_panel",
        value,
        &["id", "toolbar", "toolbar_height", "content"],
    )?;
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(
        &format!("{context} › content"),
        content_value,
        styles,
        diagnostics,
    )?;
    Ok(LayoutSidebarPanel {
        sidebar_panel,
        content: Box::new(content),
    })
}

/// The lowercase, Python-idiomatic spellings `"scroll"`'s own `"axis"` key
/// accepts — [`SPLIT_ORIENTATIONS`]'s own convention, independent of
/// [`quadraui::ScrollAxis`]'s own (PascalCase, serde-default) wire form,
/// which the standalone `"scroll_view"` additive-primitive leaf still uses
/// via generic [`decode_value`].
const SCROLL_AXES: [&str; 2] = ["vertical", "horizontal"];

fn scroll_axis_from_value(context: &str, value: &Value) -> Result<ScrollAxis, DecodeError> {
    match obj_get(value, "axis") {
        None => Ok(ScrollAxis::Vertical),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| DecodeError::type_mismatch(context, "axis", "a string"))?;
            match s {
                "vertical" => Ok(ScrollAxis::Vertical),
                "horizontal" => Ok(ScrollAxis::Horizontal),
                _ => Err(DecodeError::unknown_variant(
                    context,
                    "axis",
                    s,
                    &SCROLL_AXES,
                )),
            }
        }
    }
}

/// Decodes one `"columns"`/`"rows"` track list entry:
/// `{"Fixed": n}`/`{"Percent": p}`/`{"Weight": w}`/`{"Content": n}` —
/// [`GridTrack`]'s own serde representation, deliberately the same object-
/// wrapper vocabulary a row/column child's `"size"` key already uses for
/// [`quadraui::SectionSize`] (see [`child_size_from_value`]) rather than a
/// second spelling of the same concept. `{"Percent": p}` is range-checked
/// to `0.0..=1.0` for the same reason it is there: `{"Percent": 25}`
/// meaning "25 percent" is a plausible typo, and silently reading it as
/// `2500%` would paint a single track over the whole axis.
fn grid_track_from_value(
    value: &Value,
    context: &str,
    axis: &str,
) -> Result<GridTrack, DecodeError> {
    let track = serde_json::from_value::<GridTrack>(value.clone())
        .map_err(|err| DecodeError::borrow(context, format!("\"{axis}\": {err}")))?;
    if let GridTrack::Percent(p) = track {
        if !(0.0..=1.0).contains(&p) {
            return Err(DecodeError::borrow(
                context,
                format!("\"{axis}\": {{\"Percent\": ..}} must be between 0.0 and 1.0, got {p}"),
            ));
        }
    }
    Ok(track)
}

/// Decodes a `"grid"` node's `"columns"` or `"rows"` key: a required list
/// of [`grid_track_from_value`] entries, one per track along that axis.
fn grid_tracks_from_value(
    value: &Value,
    axis: &str,
    context: &str,
) -> Result<Vec<GridTrack>, DecodeError> {
    required_list(value, axis, context)?
        .iter()
        .map(|track| grid_track_from_value(track, context, axis))
        .collect()
}

/// Decodes a `"grid"` node: `{"columns", "rows", "cells"}`, where
/// `"columns"`/`"rows"` are the two independent track lists (see
/// [`grid_tracks_from_value`]) and `"cells"` is the row-major list of
/// child nodes filling the grid — left to right within a row, then top to
/// bottom — each recursively decoded by [`layout_node_from_value`].
///
/// More cells than the tracks describe (`columns.len() * rows.len()`) is a
/// decode error naming both counts: the overflow has nowhere to paint, and
/// silently dropping it would read as "my last tile vanished" with nothing
/// pointing at the cause. Fewer is accepted — the trailing cells stay
/// empty, which is how a dashboard with a ragged last row is written.
///
/// `spacing` is the node's own `"padding"`/`"margin"` siblings, already
/// decoded by [`spacing_from_value`] in the caller (`"gap"` is rejected
/// there: grid tracks carry no gutter, so a caller wanting one pads the
/// cell's own child or inserts a `Fixed` spacer track).
fn layout_grid_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutGrid, DecodeError> {
    let context = format!("{what} › grid");
    check_unknown_keys(
        diagnostics,
        &context,
        "grid",
        value,
        &["columns", "rows", "cells"],
    )?;
    let columns = grid_tracks_from_value(value, "columns", &context)?;
    let rows = grid_tracks_from_value(value, "rows", &context)?;
    let cells = required_list(value, "cells", &context)?;
    let capacity = columns.len() * rows.len();
    if cells.len() > capacity {
        return Err(DecodeError::borrow(
            &context,
            format!(
                "\"cells\" has {} entries but {} columns x {} rows leaves room for {capacity}",
                cells.len(),
                columns.len(),
                rows.len()
            ),
        ));
    }
    let children = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let cell_context = format!("{context} › cells[{index}]");
            let cell = must_be_dict(&context, cell, &format!("cells[{index}]"))?;
            layout_node_from_value(&cell_context, cell, styles, diagnostics)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LayoutGrid {
        columns,
        rows,
        children,
        spacing,
    })
}

/// Decodes a `"scroll"` node: `{"id", "axis", "offset", "content"}`.
/// `"axis"` is optional (defaulting to `"vertical"`); `"offset"` is
/// optional and, when present, pins the controlled scroll position (see
/// [`LayoutScroll::offset`]'s own doc for the controlled/uncontrolled
/// policy); `"content"` is a required child node, recursively decoded by
/// [`layout_node_from_value`]. `spacing` is the node's own `"padding"`/
/// `"margin"` siblings, already decoded by [`spacing_from_value`] in the
/// caller (`"gap"` is rejected there — a scroll has no second sibling to
/// space it from).
fn layout_scroll_from_value(
    what: &str,
    value: &Value,
    spacing: Spacing,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutScroll, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(what, "scroll.id", "a string"))?,
        None => return Err(DecodeError::missing_field(what, "scroll.id")),
    };
    let context = format!("{what} › scroll#{id}");
    check_unknown_keys(
        diagnostics,
        &context,
        "scroll",
        value,
        &["id", "axis", "offset", "content"],
    )?;
    let axis = scroll_axis_from_value(&context, value)?;
    let offset = opt_f32(value, "offset", &context)?;
    let content_value = required_dict(value, "content", &context)?;
    let content = layout_node_from_value(
        &format!("{context} › content"),
        content_value,
        styles,
        diagnostics,
    )?;
    Ok(LayoutScroll {
        id,
        axis,
        offset,
        content: Box::new(content),
        spacing,
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
fn layout_split_tree_from_value(
    what: &str,
    value: &Value,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutSplitTree, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(what, "split_tree.id", "a string"))?,
        None => return Err(DecodeError::missing_field(what, "split_tree.id")),
    };
    let context = format!("{what} › split_tree#{id}");
    check_unknown_keys(diagnostics, &context, "split_tree", value, &["id", "root"])?;
    let root_value = required_dict(value, "root", &context)?;

    let mut leaves = Vec::new();
    let tree =
        split_tree_node_from_value(&context, &id, root_value, &mut leaves, styles, diagnostics)?;
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
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<SplitTree, DecodeError> {
    check_unknown_keys(
        diagnostics,
        what,
        "split_tree node",
        value,
        &["leaf", "split"],
    )?;
    if let Some(v) = obj_get(value, "leaf") {
        let leaf_context = format!("{what} › leaf");
        let leaf_value = must_be_dict(what, v, "leaf")?;
        let content = layout_node_from_value(&leaf_context, leaf_value, styles, diagnostics)?;
        let placeholder = WidgetId::new(format!("{tree_id}::leaf::{}", leaves.len()));
        leaves.push(content);
        return Ok(SplitTree::leaf(placeholder));
    }
    if let Some(v) = obj_get(value, "split") {
        let split_value = must_be_dict(what, v, "split")?;
        let context = format!("{what} › split");
        check_unknown_keys(
            diagnostics,
            &context,
            "split",
            split_value,
            &["orientation", "ratio", "first", "second"],
        )?;

        let orientation = required_str(split_value, "orientation", &context)?;
        let direction = split_direction(&context, &orientation)?;
        let ratio = required_f32(split_value, "ratio", &context)?;

        let first_value = required_dict(split_value, "first", &context)?;
        let first = split_tree_node_from_value(
            &format!("{context} › first"),
            tree_id,
            first_value,
            leaves,
            styles,
            diagnostics,
        )?;
        let second_value = required_dict(split_value, "second", &context)?;
        let second = split_tree_node_from_value(
            &format!("{context} › second"),
            tree_id,
            second_value,
            leaves,
            styles,
            diagnostics,
        )?;

        return Ok(SplitTree::split(direction, ratio, first, second));
    }
    Err(DecodeError::borrow(
        what,
        "node must have a \"leaf\" or \"split\" key",
    ))
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
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutTabs, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(what, "tabs.id", "a string"))?,
        None => return Err(DecodeError::missing_field(what, "tabs.id")),
    };
    let context = format!("{what} › tabs#{id}");
    check_unknown_keys(
        diagnostics,
        &context,
        "tabs",
        value,
        &[
            "id",
            "tabs",
            "selected",
            "closable",
            "scroll_offset",
            "right_segments",
            "active_accent",
            "compact",
            "focused_index",
        ],
    )?;

    let tabs_list = required_list(value, "tabs", &context)?;
    let mut items = Vec::with_capacity(tabs_list.len());
    let mut entries = Vec::with_capacity(tabs_list.len());
    for (index, entry) in tabs_list.iter().enumerate() {
        let entry_context = format!("{context} › tabs[{index}]");
        if !entry.is_object() {
            return Err(DecodeError::type_mismatch(
                &context,
                format!("tabs[{index}]"),
                "an object",
            ));
        }
        check_unknown_keys(
            diagnostics,
            &entry_context,
            "tab",
            entry,
            &[
                "id",
                "title",
                "content",
                "closable",
                "is_dirty",
                "is_preview",
            ],
        )?;
        let tab_id = required_str(entry, "id", &entry_context)?;
        let tab_context = format!("{context} › tab#{tab_id}");
        let title = required_str(entry, "title", &tab_context)?;
        let content_value = required_dict(entry, "content", &tab_context)?;
        let content = layout_node_from_value(
            &format!("{tab_context} › content"),
            content_value,
            styles,
            diagnostics,
        )?;
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
        return Err(DecodeError::borrow(
            &context,
            format!(
                "\"selected\" index {selected} is out of range for {len} tab(s)",
                len = items.len()
            ),
        ));
    }
    if let Some(item) = items.get_mut(selected) {
        item.is_active = true;
    }

    let show_tab_close = opt_bool(value, "closable", &context)?.unwrap_or(true);
    let scroll_offset = opt_usize(value, "scroll_offset", &context)?.unwrap_or(0);
    let focused_index = opt_usize(value, "focused_index", &context)?;
    if let Some(idx) = focused_index {
        if !items.is_empty() && idx >= items.len() {
            return Err(DecodeError::borrow(
                &context,
                format!(
                    "\"focused_index\" {idx} is out of range for {len} tab(s)",
                    len = items.len()
                ),
            ));
        }
    }
    let right_segments: Vec<TabBarSegment> = match obj_get(value, "right_segments") {
        Some(v) => {
            if !v.is_array() {
                return Err(DecodeError::type_mismatch(
                    &context,
                    "right_segments",
                    "an array",
                ));
            }
            serde_json::from_value(v.clone()).map_err(|err| {
                DecodeError::borrow(&context, format!("invalid \"right_segments\": {err}"))
            })?
        }
        None => Vec::new(),
    };
    let active_accent: Option<Color> = match obj_get(value, "active_accent") {
        Some(v) => {
            let accent_value = must_be_dict(&context, v, "active_accent")?;
            Some(decode_value(&context, accent_value)?)
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

#[cfg(test)]
#[allow(deprecated)] // `Panel` is `#[deprecated]`; these tests cover the `"panel"` wire node's decode directly.
#[path = "tests/layout.rs"]
mod tests;
