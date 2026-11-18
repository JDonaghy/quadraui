//! The flat-vs-tree sniff a render callback's return value goes through
//! ([`is_layout_tree`]), the `"wire_version"` mismatch policy checked
//! once up front ([`check_wire_version`]), and [`render_frame_from_value`]
//! — the flat, pre-layout-tree [`RenderFrame`] shape's own decode.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use crate::extra_paint::{decode_extra_primitive_checked, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
use crate::layout::{LayoutNode, LEAF_KIND_KEYS};
use crate::render::RenderFrame;
use crate::DecodeError;

use super::layout::LAYOUT_NODE_KEYS;
use super::value::{
    check_unknown_keys, decode_value_checked, obj_get, DecodeDiagnostics, SCROLL_PX_EXTRA_KEYS,
    UNIVERSAL_NODE_SIBLING_KEYS,
};

// ─── Render output (flat frame vs. layout tree) ────────────────────────────

/// The decoded shape of a render callback's return value: either the flat
/// per-slot `dict` ([`RenderFrame`], the pre-layout-tree shape) or a
/// layout tree ([`LayoutNode`]).
pub enum RenderOutput {
    Flat(Box<RenderFrame>),
    Tree(LayoutNode),
}

/// A `dict` carrying any of [`LAYOUT_NODE_KEYS`], a top-level `"id"` key
/// (the one key a [`LayoutLeafNode`](crate::layout::LayoutLeaf) carries that
/// [`RenderFrame`]'s flat shape never does — see `docs/bindings-wire-format.md`'s
/// own `{"id": "c", "button": {...}}` example), or an [`unknown_node_kind_key`]
/// (a dict attempting some node kind this decoder doesn't recognize at
/// all, e.g. `{"carousel": {...}}`), is parsed as a layout tree; any other
/// `dict` is parsed as the flat per-slot shape, so a render callback
/// written against the flat shape keeps working unchanged. Without the
/// `"id"` check, a render callback returning a bare top-level leaf would
/// fall through to the flat branch, which has no `"id"`/`"style"`/`"class"`/
/// sizing-key vocabulary of its own and so would both mis-report `"id"` as
/// unknown and silently drop the leaf's `"style"`/`"class"`. Without the
/// `unknown_node_kind_key` check, a dict naming an unrecognized kind and
/// nothing else would fall through to the flat branch too, which only warns
/// on an unrecognized key by default and so would decode as an empty,
/// silently blank [`RenderFrame`] instead of raising.
pub(super) fn is_layout_tree(value: &Value) -> bool {
    obj_get(value, "id").is_some()
        || LAYOUT_NODE_KEYS
            .iter()
            .any(|key| obj_get(value, key).is_some())
        || unknown_node_kind_key(value).is_some()
}

/// Every key [`layout_node_from_value_inner`] dispatches a node or leaf
/// kind on: every [`LAYOUT_NODE_KEYS`] container, every fixed
/// [`crate::layout::LEAF_KIND_KEYS`] leaf, and every additive
/// [`EXTRA_PRIMITIVE_KINDS`] leaf.
pub(super) fn all_node_kind_keys() -> Vec<&'static str> {
    LAYOUT_NODE_KEYS
        .iter()
        .chain(LEAF_KIND_KEYS.iter())
        .chain(EXTRA_PRIMITIVE_KINDS.iter())
        .copied()
        .collect()
}

/// `value`'s first key that is neither `"id"`, a [`UNIVERSAL_NODE_SIBLING_KEYS`]
/// sibling, nor one of [`all_node_kind_keys`] -- the dict's own attempt at
/// naming a node kind this decoder doesn't recognize at all (e.g.
/// `"carousel"`). `None` when `value` isn't an object, when it carries a
/// recognized kind key already (that kind's own descriptor failing is a
/// separate, later error, not this one), or when every key present is
/// universal and no kind was attempted at all (a dict missing its
/// descriptor entirely, which stays [`DecodeError::MissingField`]/
/// [`DecodeError::Borrow`] from the kind-specific decoder rather than
/// becoming an [`DecodeError::UnknownVariant`] here).
pub(super) fn unknown_node_kind_key(value: &Value) -> Option<&str> {
    let obj = value.as_object()?;
    let has_recognized_kind = obj.keys().any(|k| {
        LAYOUT_NODE_KEYS.contains(&k.as_str())
            || LEAF_KIND_KEYS.contains(&k.as_str())
            || EXTRA_PRIMITIVE_KINDS.contains(&k.as_str())
    });
    if has_recognized_kind {
        return None;
    }
    obj.keys()
        .map(String::as_str)
        .find(|k| *k != "id" && !UNIVERSAL_NODE_SIBLING_KEYS.contains(k))
}

/// Checks a render callback's returned value's optional top-level
/// `"wire_version"` key against [`crate::wire_schema::WIRE_VERSION`] — see
/// that constant's own doc for the full mismatch policy (absent accepted
/// as version 1, older-or-equal accepted, newer rejected). Called once, by
/// [`decode_render_output`], before the flat-vs-tree dispatch — the
/// version key is a property of the whole render output, not of any one
/// layout-tree node, so nested [`decode_layout_node`] calls never re-check
/// it.
pub(super) fn check_wire_version(what: &str, value: &Value) -> Result<(), DecodeError> {
    let Some(v) = obj_get(value, "wire_version") else {
        return Ok(());
    };
    let version = v.as_u64().ok_or_else(|| {
        DecodeError::type_mismatch(
            what,
            "wire_version",
            format!("a non-negative integer, got {v}"),
        )
    })?;
    let current = u64::from(crate::wire_schema::WIRE_VERSION);
    if version > current {
        return Err(DecodeError::version_mismatch(what, version, current));
    }
    Ok(())
}

/// Reads and decodes one optional slot entry from a render callback's
/// returned `dict`: a missing key or a `null` value both mean "draw
/// nothing in this slot this frame"; anything else must be a `dict`
/// decodable into `T`. An unrecognized key inside that dict routes through
/// `diagnostics` via [`decode_value_checked`], the same strict/warn policy
/// a layout-tree leaf's own body already gets. `extra_keys` is forwarded
/// straight to [`decode_value_checked`] — `&SCROLL_PX_EXTRA_KEYS` for
/// `"list"`/`"tree"`/`"data_table"`, `&[]` for every other slot.
fn slot_entry<T: serde::de::DeserializeOwned>(
    diagnostics: &DecodeDiagnostics,
    what: &str,
    value: &Value,
    key: &str,
    extra_keys: &[&str],
) -> Result<Option<T>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => {
            if !v.is_object() {
                return Err(DecodeError::type_mismatch(what, key, "an object or null"));
            }
            Ok(Some(decode_value_checked(
                diagnostics,
                what,
                key,
                v,
                extra_keys,
            )?))
        }
        None => Ok(None),
    }
}

/// Reads and decodes one optional additive primitive slot entry, keyed by
/// `kind` (one of [`EXTRA_PRIMITIVE_KINDS`]), from a render callback's
/// returned `dict`. Mirrors [`slot_entry`]'s missing/`null`/dict-or-error
/// shape (including its `diagnostics` routing), dispatching the decode
/// through [`decode_extra_primitive_checked`] instead of a single fixed
/// type.
fn extra_slot_entry(
    diagnostics: &DecodeDiagnostics,
    what: &str,
    value: &Value,
    kind: &str,
) -> Result<Option<Rc<dyn ExtraPaint>>, DecodeError> {
    match obj_get(value, kind) {
        Some(v) => {
            if !v.is_object() {
                return Err(DecodeError::type_mismatch(what, kind, "an object or null"));
            }
            Ok(Some(decode_extra_primitive_checked(
                diagnostics,
                what,
                kind,
                v,
            )?))
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
    diagnostics: &DecodeDiagnostics,
) -> Result<RenderFrame, DecodeError> {
    let known_keys: Vec<&str> = [
        "button",
        "text_display",
        "list",
        "tree",
        "data_table",
        "tab_bar",
        "wire_version",
    ]
    .into_iter()
    .chain(EXTRA_PRIMITIVE_KINDS.iter().copied())
    .collect();
    check_unknown_keys(diagnostics, what, "render output", value, &known_keys)?;
    let mut extra = HashMap::new();
    for kind in EXTRA_PRIMITIVE_KINDS {
        if let Some(v) = extra_slot_entry(diagnostics, what, value, kind)? {
            extra.insert(kind.to_string(), v);
        }
    }
    let list_scroll_px = obj_get(value, "list").and_then(crate::scroll_px_from_value);
    let tree_scroll_px = obj_get(value, "tree").and_then(crate::scroll_px_from_value);
    let data_table_scroll_px = obj_get(value, "data_table").and_then(crate::scroll_px_from_value);
    Ok(RenderFrame {
        button: slot_entry(diagnostics, what, value, "button", &[])?,
        text_display: slot_entry(diagnostics, what, value, "text_display", &[])?,
        list: slot_entry(diagnostics, what, value, "list", &SCROLL_PX_EXTRA_KEYS)?,
        tree: slot_entry(diagnostics, what, value, "tree", &SCROLL_PX_EXTRA_KEYS)?,
        data_table: slot_entry(
            diagnostics,
            what,
            value,
            "data_table",
            &SCROLL_PX_EXTRA_KEYS,
        )?,
        tab_bar: slot_entry(diagnostics, what, value, "tab_bar", &[])?,
        extra,
        list_scroll_px,
        tree_scroll_px,
        data_table_scroll_px,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use quadraui::Theme;

    use crate::layout::{LayoutNode, StyleSets};
    use crate::wire::test_support::err;
    use crate::wire::{decode_render_output, decode_render_output_with_styles, StyleContext};

    use super::*;

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

    /// A render callback returning a bare top-level leaf (`{"id": ...,
    /// "<kind>": {...}}`, no container key) is a one-node [`LayoutNode`]
    /// tree, not the flat [`RenderFrame`] shape -- `docs/bindings-wire-format.md`'s
    /// own documented example (`{"id": "c", "button": {...}, "class":
    /// "card"}`). Decoded under `strict`, it must produce zero diagnostics:
    /// before `is_layout_tree` also checked for a top-level `"id"` key,
    /// this fell through to the flat branch, which has no `"id"` in its own
    /// known-key list and so raised `UnknownKeyError` on a perfectly valid
    /// descriptor.
    #[test]
    fn a_bare_top_level_leaf_decodes_as_a_tree_with_no_diagnostics_under_strict() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::strict();
        let value = json!({
            "id": "q:out",
            "text_display": {"id": "q:out", "lines": []},
        });
        let output = decode_render_output_with_styles(
            "render",
            &value,
            StyleContext::new(&sets, &theme),
            &diagnostics,
        )
        .expect("a bare top-level leaf is a valid, documented render output");
        let RenderOutput::Tree(LayoutNode::Leaf(leaf)) = output else {
            panic!("expected a one-node leaf tree");
        };
        assert_eq!(leaf.id, "q:out");
        assert!(diagnostics.warned_keys().is_empty());
    }

    /// A render callback's returned value with no `"wire_version"` key at
    /// all decodes exactly like before this key existed -- the versionless
    /// policy [`crate::wire_schema::WIRE_VERSION`]'s own doc describes.
    #[test]
    fn decode_render_output_accepts_a_versionless_tree() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        });
        let output = decode_render_output("render", &value).expect("versionless tree accepted");
        assert!(matches!(output, RenderOutput::Tree(_)));
    }

    /// A `"wire_version"` equal to the current [`crate::wire_schema::WIRE_VERSION`] is accepted
    /// on both the tree and flat shapes.
    #[test]
    fn decode_render_output_accepts_the_current_wire_version() {
        let tree = json!({
            "wire_version": crate::wire_schema::WIRE_VERSION,
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        });
        let output = decode_render_output("render", &tree).expect("current version accepted");
        assert!(matches!(output, RenderOutput::Tree(_)));

        let flat = json!({
            "wire_version": crate::wire_schema::WIRE_VERSION,
            "button": {"id": "b1", "label": "Go"},
        });
        let output = decode_render_output("render", &flat).expect("current version accepted");
        assert!(matches!(output, RenderOutput::Flat(_)));
    }

    /// A `"wire_version"` older than the current [`crate::wire_schema::WIRE_VERSION`] is
    /// accepted -- the accept-older half of the mismatch policy.
    #[test]
    fn decode_render_output_accepts_an_older_wire_version() {
        let value = json!({
            "wire_version": 0,
            "button": {"id": "b1", "label": "Go"},
        });
        let output = decode_render_output("render", &value).expect("older version accepted");
        assert!(matches!(output, RenderOutput::Flat(_)));
    }

    /// A `"wire_version"` newer than the current [`crate::wire_schema::WIRE_VERSION`] is a
    /// clear decode error naming both versions -- the reject-newer half of
    /// the mismatch policy.
    #[test]
    fn decode_render_output_rejects_a_newer_wire_version() {
        let value = json!({
            "wire_version": crate::wire_schema::WIRE_VERSION + 1,
            "button": {"id": "b1", "label": "Go"},
        });
        let decode_err = err(decode_render_output("render", &value));
        assert!(
            decode_err.to_string().contains("\"wire_version\""),
            "{}",
            decode_err.to_string()
        );
        assert!(
            decode_err
                .to_string()
                .contains("newer than this host understands"),
            "{}",
            decode_err.to_string()
        );
    }

    /// A non-integer `"wire_version"` is a decode error, not a silent
    /// coercion.
    #[test]
    fn decode_render_output_rejects_a_non_integer_wire_version() {
        let value = json!({
            "wire_version": "1",
            "button": {"id": "b1", "label": "Go"},
        });
        let decode_err = err(decode_render_output("render", &value));
        assert!(
            decode_err
                .to_string()
                .contains("must be a non-negative integer"),
            "{}",
            decode_err.to_string()
        );
    }
}
