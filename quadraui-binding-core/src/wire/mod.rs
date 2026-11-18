//! The neutral end of a binding's descriptor wire decode: a render
//! callback's returned layout tree / flat [`crate::render::RenderFrame`] shape
//! ([`decode_render_output`]), one layout tree node
//! ([`decode_layout_node`]), and `run_shell`'s `config` dict
//! ([`decode_shell_config`]).
//!
//! Every decoder here is generic over `serde::Deserializer`, not tied to
//! any host language's own wire format — a binding supplies its own
//! `Deserializer` implementation over its own object model (a Python dict
//! today, a JS object next) and maps the returned [`crate::DecodeError`]
//! to its own exception type. Internally, every entry point first
//! [`value::materialize`]s the whole input into one [`serde_json::Value`]
//! tree, then dispatches against that uniform shape — the dynamically-keyed,
//! "which of these N keys is present" dict shapes a layout tree node or
//! `ShellConfig` decodes from don't map onto a single `#[derive(Deserialize)]`
//! struct, but a `serde_json::Value` is cheap to re-inspect by key as many
//! times as the recursive decode needs. [`value::materialize`] differs from
//! `Value`'s own `Deserialize` impl in exactly two ways — see its own doc —
//! so a byte string and a non-finite float each still round-trip (or fail
//! loudly) the way a decode straight off the original `Deserializer`
//! would, instead of silently losing information once `Value` is in the
//! way. `host::merge_overrides` uses the same crate but its own,
//! unmodified `Value::deserialize` — its medium is always a backend's own
//! `Theme`/`Style`, already round-tripped through `serde_json` once on the
//! way in, never raw binding input.
//!
//! Split by concern, along the seams the decode itself already has: the
//! shared `Value` accessor/diagnostics vocabulary ([`value`]), the eight
//! container node kinds ([`layout`]), `"class"`/`"style"` resolution
//! ([`style`]), the per-widget leaf dispatch ([`leaf`]), popover/dialog
//! overlay content ([`overlay`]), the flat-vs-tree render-output sniff
//! ([`frame`]), and `run_shell`'s own config dict ([`shell`]). Every path
//! reachable today as `quadraui_binding_core::wire::X` still resolves,
//! through this module's own re-exports below.
//!
//! [`leaf`]'s dispatch includes one kind with no `Deserialize` shape of
//! its own: a `"scripted_widget"` leaf carries only an id and this frame's
//! JSON props, and is spliced in from whatever live
//! [`crate::scripted::ScriptedPrimitive<W>`] a binding already registered
//! against that id via [`crate::scripted::register_scripted_widget`] —
//! see that function's own doc, and `crate::scripted`'s module doc
//! ("Splicing into a decoded tree without a `Deserialize` impl"), for the
//! full contract.

use serde::Deserializer;

use quadraui::{ShellConfig, Theme};

use crate::layout::{LayoutNode, StyleSets};
use crate::DecodeError;

mod frame;
mod layout;
mod leaf;
mod overlay;
mod shell;
mod style;
mod value;

#[cfg(test)]
mod test_support;

pub(crate) use frame::render_frame_from_value;
pub use frame::RenderOutput;
pub use overlay::{decode_layout_dialog, decode_popover, OverlayContent};
pub use style::{decode_style_overrides, validate_style_set_name, StyleContext};
pub(crate) use value::decode_value_checked;
pub use value::DecodeDiagnostics;

pub use layout::LAYOUT_NODE_KEYS;
pub use value::StaticTabContent;

use frame::{check_wire_version, is_layout_tree};
use layout::layout_node_from_value;
use shell::shell_config_from_value;
use value::materialize;

/// Decodes one layout tree node from `deserializer`: a `"row"` or `"column"`
/// key holds the container's children, a `"split"` key holds a two-pane
/// divider, a `"tabs"` key holds a tab strip plus per-tab content, a
/// `"panel"`/`"sidebar_panel"` key holds one chrome descriptor plus a
/// single content subtree, a `"split_tree"` key holds an arbitrarily
/// nested divider tree, a `"scroll"` key holds one content subtree hosted
/// in a scrollable viewport; any other shape is decoded as a leaf.
pub fn decode_layout_node<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
) -> Result<LayoutNode, DecodeError> {
    let sets = StyleSets::new();
    let theme = Theme::default();
    let diagnostics = DecodeDiagnostics::warn();
    decode_layout_node_with_styles(
        what,
        deserializer,
        StyleContext::new(&sets, &theme),
        &diagnostics,
    )
}

/// [`decode_layout_node`] against a caller-supplied [`StyleContext`] and
/// [`DecodeDiagnostics`] — the entry point a binding's own render pass
/// calls, handing in the [`StyleSets`] its `define_style` surface has been
/// filling plus the backend's current [`quadraui::Theme`] (so every
/// `"class"` key in the tree resolves against them) and its own
/// session-scoped `diagnostics`, so an unknown optional key found anywhere
/// in the tree is reported through that same session rather than a fresh
/// one per call.
pub fn decode_layout_node_with_styles<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutNode, DecodeError> {
    let value = materialize(deserializer)
        .map_err(|err| DecodeError::borrow(what, format!("invalid descriptor: {err}")))?;
    layout_node_from_value(what, &value, styles, diagnostics)
}

/// Decodes a render callback's returned dict into a [`RenderOutput`] — see
/// [`check_wire_version`] for the `"wire_version"` mismatch policy checked
/// first, and [`is_layout_tree`] for the flat-vs-tree sniff. On the tree
/// path, also runs [`crate::layout::validate_unique_leaf_ids`] over the
/// decoded tree before returning it.
pub fn decode_render_output<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
) -> Result<RenderOutput, DecodeError> {
    let sets = StyleSets::new();
    let theme = Theme::default();
    let diagnostics = DecodeDiagnostics::warn();
    decode_render_output_with_styles(
        what,
        deserializer,
        StyleContext::new(&sets, &theme),
        &diagnostics,
    )
}

/// [`decode_render_output`] against a caller-supplied [`StyleContext`] and
/// [`DecodeDiagnostics`] — the entry point a binding's own render pass
/// calls, so every `"class"` key in the returned tree resolves against the
/// [`StyleSets`] that binding's `define_style` surface has been filling,
/// and an unknown optional key anywhere in the output is reported through
/// that binding's own session rather than a fresh one per call. The flat
/// (pre-layout-tree) shape carries no nodes, hence no `"class"` keys, and
/// decodes identically either way.
pub fn decode_render_output_with_styles<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<RenderOutput, DecodeError> {
    let value = materialize(deserializer)
        .map_err(|err| DecodeError::borrow(what, format!("invalid descriptor: {err}")))?;
    check_wire_version(what, &value)?;
    if is_layout_tree(&value) {
        let node = layout_node_from_value(what, &value, styles, diagnostics)?;
        crate::layout::validate_unique_leaf_ids(what, &node)?;
        Ok(RenderOutput::Tree(node))
    } else {
        Ok(RenderOutput::Flat(Box::new(render_frame_from_value(
            what,
            &value,
            diagnostics,
        )?)))
    }
}

/// Decodes `run_shell`'s `config` dict from `deserializer` into a
/// [`ShellConfig`] — see [`shell::shell_config_from_value`] for the
/// recognized key vocabulary.
pub fn decode_shell_config<'de, D: Deserializer<'de>>(
    deserializer: D,
    diagnostics: &DecodeDiagnostics,
) -> Result<ShellConfig, DecodeError> {
    let value = materialize(deserializer)
        .map_err(|err| DecodeError::borrow("run_shell", format!("invalid descriptor: {err}")))?;
    shell_config_from_value(&value, diagnostics)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    use serde_json::json;
    use serde_json::Value;

    use crate::wire::test_support::{decode, err};

    use super::*;

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
        assert!(
            output.to_string().contains("duplicate leaf id"),
            "{}",
            output.to_string()
        );
    }

    /// A node dict naming a kind this decoder has never heard of (`"grid"`,
    /// neither a [`LAYOUT_NODE_KEYS`] container nor a
    /// [`crate::layout::LEAF_KIND_KEYS`]/[`EXTRA_PRIMITIVE_KINDS`] leaf) is
    /// an [`DecodeError::UnknownVariant`] naming both the key and the path
    /// it was found at, under the default (warn) [`DecodeDiagnostics`]
    /// policy -- not a silently blank render, whether the dict is the
    /// whole tree or nested inside a container's children.
    #[test]
    fn an_unknown_node_kind_is_rejected_at_the_top_level_and_nested() {
        let top_level = err(decode(&json!({"grid": {}})));
        assert!(
            matches!(top_level, DecodeError::UnknownVariant { .. }),
            "{top_level:?}"
        );
        assert!(top_level.to_string().contains("grid"), "{top_level}");
        assert_eq!(top_level.path(), "test");

        let nested = err(decode(&json!({
            "row": [{"id": "a", "grid": {}}],
        })));
        assert!(
            matches!(nested, DecodeError::UnknownVariant { .. }),
            "{nested:?}"
        );
        assert!(nested.to_string().contains("grid"), "{nested}");
        assert!(nested.path().contains("children[0]"), "{}", nested.path());
    }

    /// The same unknown-kind rejection, exercised through
    /// [`decode_render_output`] -- the entry point a binding's render
    /// callback actually returns into. Without [`is_layout_tree`]'s own
    /// [`unknown_node_kind_key`] check, a lone `"grid"` key matches
    /// neither flat-vs-tree sniff and falls through to the flat
    /// [`RenderFrame`] shape, decoding as an empty, silently blank frame
    /// (every slot `None`) instead of raising.
    #[test]
    fn decode_render_output_rejects_an_unknown_top_level_node_kind() {
        let err = err(decode_render_output("render", json!({"grid": {}})));
        assert!(matches!(err, DecodeError::UnknownVariant { .. }), "{err:?}");
        assert!(err.to_string().contains("grid"), "{err}");
        assert_eq!(err.path(), "render");
    }

    /// A decode failure several levels into a layout tree names every
    /// ancestor node on the way down, not just the render callback's own
    /// top-level label -- the whole point of carrying a path rather than
    /// a bare field name. A `"ratio"` missing from a `"split"` nested
    /// inside a `"panel"` nested inside a `"tabs"` entry's content names
    /// `"tabs#frm"`, `"panel#chrome"` and `"split#inner"` in that order,
    /// not just `"render"`.
    #[test]
    fn a_deeply_nested_decode_error_names_every_ancestor_on_the_way_down() {
        let value = json!({
            "tabs": {
                "id": "frm",
                "tabs": [{
                    "id": "one",
                    "title": "One",
                    "content": {
                        "panel": {
                            "id": "chrome",
                            "content": {
                                "split": {
                                    "id": "inner",
                                    "orientation": "horizontal",
                                    // "ratio" deliberately omitted.
                                    "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                                    "second": {"id": "b", "button": {"id": "b", "label": "B"}},
                                },
                            },
                        },
                    },
                }],
            },
        });
        let error = err(decode_render_output("render", &value));
        assert!(
            matches!(error, DecodeError::MissingField { .. }),
            "{error:?}"
        );
        assert_eq!(
            error.path(),
            "render › tabs#frm › tab#one › content › panel#chrome › content › split#inner"
        );
        let message = error.to_string();
        assert!(
            message.starts_with("render: at "),
            "expected the root label and \"at\", got: {message}"
        );
        assert!(message.contains("tabs#frm"), "{message}");
        assert!(message.contains("panel#chrome"), "{message}");
        assert!(message.contains("split#inner"), "{message}");
        assert!(message.contains("missing field \"ratio\""), "{message}");
    }

    /// A container-shape error -- the `"row"`/`"column"` key's own value
    /// not being an array -- names the container it was raised on, not
    /// just the ancestor path leading to it, the same way a sibling
    /// `"padding"`/`"justify"`/`"align"` key error on that same container
    /// already does. Nested one level inside an outer `"row"`, the inner
    /// `"column"`'s malformed children keep that outer `children[0]` step
    /// in the path too, not just the container's own label.
    #[test]
    fn a_malformed_nested_container_names_the_container_its_shape_error_is_on() {
        let value = json!({
            "row": [{
                "id": "a",
                "column": "not a list",
            }],
        });
        let error = err(decode_render_output("render", &value));
        assert!(
            matches!(error, DecodeError::TypeMismatch { .. }),
            "{error:?}"
        );
        assert_eq!(error.path(), "render › \"row\" › children[0] › \"column\"");
        let message = error.to_string();
        assert!(message.contains("children[0]"), "{message}");
        assert!(message.contains("\"column\""), "{message}");
        assert!(
            message.contains("\"children\" must be an array"),
            "{message}"
        );
    }

    /// Every [`DecodeError`] variant pairs a stable [`DecodeError::code`]
    /// with the [`DecodeError::path`] at which it was raised -- the two
    /// pieces of information a binding's exception mapping (and this
    /// crate's own `#[non_exhaustive]` match) relies on instead of parsing
    /// the rendered message.
    #[test]
    fn every_variant_exposes_its_own_code_and_the_path_it_was_raised_at() {
        let missing = err(decode(&json!({"split": {
            "orientation": "horizontal",
            "ratio": 0.5,
            "first": {"id": "a", "button": {"id": "a", "label": "A"}},
            "second": {"id": "b", "button": {"id": "b", "label": "B"}},
        }})));
        assert_eq!(missing.code(), "missing_field");
        assert_eq!(missing.path(), "test");

        let unknown_variant = err(decode(&json!({
            "split": {
                "id": "s1",
                "orientation": "diagonal",
                "ratio": 0.5,
                "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                "second": {"id": "b", "button": {"id": "b", "label": "B"}},
            }
        })));
        assert_eq!(unknown_variant.code(), "unknown_variant");
        assert_eq!(unknown_variant.path(), "test › split#s1");

        let type_mismatch = err(decode(&json!({"row": "not a list"})));
        assert_eq!(type_mismatch.code(), "type_mismatch");
        assert_eq!(type_mismatch.path(), "test › \"row\"");

        let unknown_key = err(decode(&json!({"row": [], "padding": {"lef": 4}})));
        assert_eq!(unknown_key.code(), "unknown_key");
        assert_eq!(unknown_key.path(), "test › \"row\"");

        let version_mismatch = err(decode_render_output(
            "render",
            json!({
                "wire_version": crate::wire_schema::WIRE_VERSION + 1,
                "button": {"id": "b1", "label": "Go"},
            }),
        ));
        assert_eq!(version_mismatch.code(), "version_mismatch");
        assert_eq!(version_mismatch.path(), "render");

        let borrow = err(decode(&json!({
            "id": "leaf1",
            "button": {"id": "leaf1", "label": "A"},
            "list": {"id": "leaf1", "items": []},
        })));
        assert_eq!(borrow.code(), "borrow");
        assert_eq!(borrow.path(), "test › leaf#leaf1");
    }

    /// A tree exercising every node kind decodes cleanly with no unknown
    /// optional keys found, regardless of policy — the "an existing valid
    /// tree keeps decoding exactly as before" regression every per-kind
    /// known-key list above must not break.
    fn full_tree_fixture() -> Value {
        json!({
            "column": [
                {
                    "split": {
                        "id": "split1",
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                        "second": {
                            "tabs": {
                                "id": "tabs1",
                                "tabs": [
                                    {
                                        "id": "tab1",
                                        "title": "One",
                                        "content": {"id": "b", "button": {"id": "b", "label": "B"}},
                                    },
                                ],
                            },
                        },
                    },
                    "padding": 4,
                },
                {
                    "panel": {
                        "id": "panel1",
                        "content": {"id": "c", "button": {"id": "c", "label": "C"}},
                    },
                },
                {
                    "sidebar_panel": {
                        "id": "sidebar1",
                        "content": {"id": "d", "button": {"id": "d", "label": "D"}},
                    },
                },
                {
                    "scroll": {
                        "id": "scroll1",
                        "content": {"id": "e", "button": {"id": "e", "label": "E"}},
                    },
                },
                {
                    "split_tree": {
                        "id": "tree1",
                        "root": {
                            "split": {
                                "orientation": "vertical",
                                "ratio": 0.5,
                                "first": {"leaf": {"id": "f", "button": {"id": "f", "label": "F"}}},
                                "second": {"leaf": {"id": "g", "button": {"id": "g", "label": "G"}}},
                            },
                        },
                    },
                },
            ],
        })
    }

    /// A tree exercising every node kind decodes with zero diagnostics
    /// recorded, under the default warn policy.
    #[test]
    fn a_valid_tree_of_every_node_kind_produces_no_diagnostics() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::warn();
        decode_layout_node_with_styles(
            "test",
            full_tree_fixture(),
            StyleContext::new(&sets, &theme),
            &diagnostics,
        )
        .expect("every node kind above is valid");
        assert!(diagnostics.warned_keys().is_empty());
    }

    /// An unrecognised *optional* sibling key on a layout-tree node dict —
    /// the misspelt `"focussed"` for `"focused_index"` -- warns through the
    /// registered hook exactly once per key for the session's lifetime
    /// under the default (non-strict) policy, decoding otherwise
    /// unaffected, rather than silently dropping it the way this policy
    /// didn't exist before.
    #[test]
    fn unknown_optional_key_warns_once_per_key_per_session() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::warn();
        let hook_calls = Rc::new(RefCell::new(Vec::new()));
        let recorded = hook_calls.clone();
        diagnostics.set_hook(move |path: &str, key: &str| {
            recorded
                .borrow_mut()
                .push((path.to_string(), key.to_string()));
        });
        let value = json!({
            "tabs": {
                "id": "tabs1",
                "tabs": [],
                "focussed": 0,
            },
        });
        for _ in 0..3 {
            decode_layout_node_with_styles(
                "test",
                &value,
                StyleContext::new(&sets, &theme),
                &diagnostics,
            )
            .expect("an unknown optional key does not fail the decode under warn");
        }
        assert_eq!(
            diagnostics.warned_keys(),
            HashSet::from(["focussed".to_string()])
        );
        assert_eq!(
            hook_calls.borrow().as_slice(),
            &[("test › tabs#tabs1".to_string(), "focussed".to_string())],
            "the hook fires exactly once across three identical decodes"
        );
    }

    /// The same unknown optional key [`unknown_optional_key_warns_once_per_key_per_session`]
    /// warns on is a hard [`DecodeError::UnknownKey`] under `strict`.
    #[test]
    fn strict_mode_rejects_an_unknown_optional_key() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::strict();
        let value = json!({
            "tabs": {
                "id": "tabs1",
                "tabs": [],
                "focussed": 0,
            },
        });
        let error = err(decode_layout_node_with_styles(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &diagnostics,
        ));
        assert_eq!(error.code(), "unknown_key");
        assert!(
            error.to_string().contains("\"focussed\""),
            "{}",
            error.to_string()
        );
    }
}
