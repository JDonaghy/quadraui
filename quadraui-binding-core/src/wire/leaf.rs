//! Decodes a `{"id": ..., "<kind>": <descriptor>}` layout-tree leaf dict —
//! the per-widget dispatch every [`super::layout`] container falls
//! through to once none of its own [`super::layout::LAYOUT_NODE_KEYS`]
//! match.

use serde_json::Value;

use crate::extra_paint::{decode_extra_primitive_checked, EXTRA_PRIMITIVE_KINDS};
use crate::layout::{LayoutLeaf, LeafDescriptor, LEAF_KIND_KEYS};
use crate::scripted::{lookup_scripted_widget, ScriptedWidgetDescriptor};
use crate::DecodeError;

use super::value::{
    check_unknown_keys, decode_value_checked, must_be_dict, obj_get, DecodeDiagnostics,
    SCROLL_PX_EXTRA_KEYS, UNIVERSAL_NODE_SIBLING_KEYS,
};

/// Decodes one `{"id": ..., "<kind>": <descriptor>}` leaf dict. `"id"`
/// must be present and a string; exactly one of [`LEAF_KIND_KEYS`] or an
/// [`EXTRA_PRIMITIVE_KINDS`] kind must be present and hold a `dict` — a
/// leaf with two keys from either set (fixed+fixed, fixed+extra, or
/// extra+extra) is rejected by the single `found` duplicate check shared
/// across both loops below.
pub(super) fn layout_leaf_from_value(
    what: &str,
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<LayoutLeaf, DecodeError> {
    let id = match obj_get(value, "id") {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(what, "leaf.id", "a string"))?,
        None => return Err(DecodeError::missing_field(what, "leaf.id")),
    };
    let context = format!("{what} › leaf#{id}");
    // "id" plus whichever of `LEAF_KIND_KEYS`/`EXTRA_PRIMITIVE_KINDS` holds
    // this leaf's one descriptor, plus the sibling keys every layout-tree
    // node dict may carry regardless of its own kind.
    let known_keys: Vec<&str> = std::iter::once("id")
        .chain(LEAF_KIND_KEYS)
        .chain(EXTRA_PRIMITIVE_KINDS.iter().copied())
        .chain(UNIVERSAL_NODE_SIBLING_KEYS)
        .collect();
    check_unknown_keys(diagnostics, &context, "leaf", value, &known_keys)?;

    let mut found: Option<LeafDescriptor> = None;
    // Set alongside `found` only for the three scrollable kinds —
    // `scroll_px_from_value` reads the same leaf dict `decode_value` just
    // decoded a typed descriptor from, so it stays `None` for every other
    // kind (and whenever the dict never sets the key).
    let mut scroll_px: Option<f32> = None;
    for key in LEAF_KIND_KEYS {
        let Some(v) = obj_get(value, key) else {
            continue;
        };
        let value_dict = must_be_dict(&context, v, key)?;
        if found.is_some() {
            return Err(DecodeError::borrow(
                &context,
                "has more than one descriptor kind",
            ));
        }
        found = Some(match key {
            "button" => LeafDescriptor::Button(decode_value_checked(
                diagnostics,
                &context,
                key,
                value_dict,
                &[],
            )?),
            "text_display" => LeafDescriptor::TextDisplay(decode_value_checked(
                diagnostics,
                &context,
                key,
                value_dict,
                &[],
            )?),
            "list" => {
                scroll_px = crate::scroll_px_from_value(v);
                LeafDescriptor::List(decode_value_checked(
                    diagnostics,
                    &context,
                    key,
                    value_dict,
                    &SCROLL_PX_EXTRA_KEYS,
                )?)
            }
            "tree" => {
                scroll_px = crate::scroll_px_from_value(v);
                LeafDescriptor::Tree(decode_value_checked(
                    diagnostics,
                    &context,
                    key,
                    value_dict,
                    &SCROLL_PX_EXTRA_KEYS,
                )?)
            }
            "data_table" => {
                scroll_px = crate::scroll_px_from_value(v);
                LeafDescriptor::DataTable(decode_value_checked(
                    diagnostics,
                    &context,
                    key,
                    value_dict,
                    &SCROLL_PX_EXTRA_KEYS,
                )?)
            }
            "tab_bar" => LeafDescriptor::TabBar(decode_value_checked(
                diagnostics,
                &context,
                key,
                value_dict,
                &[],
            )?),
            // No `Deserialize` impl constructs this one from scratch — see
            // `crate::scripted`'s own module doc, "Splicing into a decoded
            // tree without a `Deserialize` impl". The dict carries only
            // this frame's `"props"`; the live widget itself is recovered
            // from whatever `crate::scripted::register_scripted_widget`
            // call already registered it under this leaf's own `id`.
            "scripted_widget" => {
                let descriptor: ScriptedWidgetDescriptor =
                    decode_value_checked(diagnostics, &context, key, value_dict, &[])?;
                let extra = lookup_scripted_widget(&id).ok_or_else(|| {
                    DecodeError::borrow(
                        &context,
                        format!(
                            "no scripted widget is registered for leaf id {id:?} — \
                             the binding must call \
                             quadraui_binding_core::scripted::register_scripted_widget \
                             before decoding a tree containing it"
                        ),
                    )
                })?;
                if let Some(scripted) = extra.as_scripted() {
                    scripted.set_scripted_props(descriptor.props);
                }
                LeafDescriptor::Extra(extra)
            }
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
        let value_dict = must_be_dict(&context, v, kind)?;
        if found.is_some() {
            return Err(DecodeError::borrow(
                &context,
                "has more than one descriptor kind",
            ));
        }
        found = Some(LeafDescriptor::Extra(decode_extra_primitive_checked(
            diagnostics,
            &context,
            kind,
            value_dict,
        )?));
    }

    let descriptor = found.ok_or_else(|| {
        DecodeError::borrow(
            &context,
            format!(
                "is missing a descriptor (one of {LEAF_KIND_KEYS:?} or {:?})",
                EXTRA_PRIMITIVE_KINDS
            ),
        )
    })?;

    Ok(LayoutLeaf {
        id,
        descriptor,
        scroll_px,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use quadraui::Theme;

    use crate::layout::StyleSets;
    use crate::wire::test_support::{decode, err};
    use crate::wire::{
        decode_layout_node_with_styles, decode_render_output_with_styles, StyleContext,
    };

    use super::*;

    /// A `ScriptWidget` built directly in Rust test code (mirroring
    /// `crate::scripted`'s own `Swatch` test widget) — the minimum a
    /// `"scripted_widget"` leaf decode needs to splice around; no real
    /// binding callback object anywhere in scope.
    struct TestWidget;

    impl crate::scripted::ScriptWidget for TestWidget {
        fn measure(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            _constraints: crate::scripted::ScriptedConstraints,
        ) -> crate::scripted::ScriptedSize {
            crate::scripted::ScriptedSize {
                width: 1.0,
                height: 1.0,
            }
        }

        fn paint(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            _props: &crate::scripted::ScriptedProps,
            _rect: quadraui::Rect,
        ) -> crate::scripted::ScriptedPaint {
            crate::scripted::ScriptedPaint::default()
        }

        fn on_event(
            &self,
            _event: &crate::scripted::ScriptedInputEvent,
            _props: &crate::scripted::ScriptedProps,
            _state: &crate::scripted::ScriptedState,
        ) -> bool {
            false
        }
    }

    /// A `"scripted_widget"` leaf naming an id with a live registration
    /// (via [`crate::scripted::register_scripted_widget`]) decodes into
    /// the exact `Rc` that was registered — never a fresh instance, the
    /// whole point of this kind not going through
    /// `extra_primitives!`'s `Deserialize`-and-construct dispatch (see
    /// `crate::scripted`'s own module doc) — with this frame's `"props"`
    /// applied onto it.
    #[test]
    fn scripted_widget_leaf_decodes_the_registered_primitive_and_applies_props() {
        use crate::extra_paint::ExtraPaint;
        use crate::scripted::{register_scripted_widget, unregister_scripted_widget};
        use std::rc::Rc;

        let primitive = Rc::new(crate::scripted::ScriptedPrimitive::new(
            quadraui::WidgetId::new("sw"),
            TestWidget,
            false,
        ));
        register_scripted_widget("sw", primitive.clone() as Rc<dyn ExtraPaint>);

        let value = json!({
            "id": "sw",
            "scripted_widget": { "props": { "color": "blue" } },
        });
        let node = decode(&value).expect("a registered scripted_widget leaf decodes");
        let crate::layout::LayoutNode::Leaf(leaf) = node else {
            panic!("expected a leaf node");
        };
        let LeafDescriptor::Extra(extra) = leaf.descriptor else {
            panic!("expected an Extra descriptor");
        };
        assert!(
            Rc::ptr_eq(&extra, &(primitive.clone() as Rc<dyn ExtraPaint>)),
            "the decoded leaf must wrap the exact Rc that was registered, not a fresh instance"
        );
        assert_eq!(primitive.props(), json!({"color": "blue"}));

        unregister_scripted_widget("sw");
    }

    /// A `"scripted_widget"` leaf naming an id with no live registration
    /// is a clear decode error, not a panic or a silently blank leaf.
    #[test]
    fn scripted_widget_leaf_with_no_registration_is_a_decode_error() {
        let value = json!({
            "id": "unregistered",
            "scripted_widget": {},
        });
        let error = err(decode(&value));
        assert!(
            error
                .to_string()
                .contains("no scripted widget is registered"),
            "{error}"
        );
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
        assert!(
            err.to_string().contains("more than one descriptor kind"),
            "{}",
            err.to_string()
        );
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
        assert!(matches!(err, DecodeError::UnknownVariant { .. }), "{err:?}");
        assert!(
            err.to_string().contains("not_a_real_kind"),
            "{}",
            err.to_string()
        );
    }

    /// An unrecognised key inside a `"button"` descriptor body -- a fixed
    /// leaf's own dict, decoded through [`decode_value_checked`] rather
    /// than [`check_unknown_keys`]'s sibling-key path -- is a hard
    /// [`DecodeError::UnknownKey`] under `strict`, naming the key, the
    /// dict it was found on, and the keys `Button` actually accepts.
    #[test]
    fn button_leaf_body_rejects_an_unknown_key_under_strict() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::strict();
        let value = json!({
            "id": "b1",
            "button": {"id": "b1", "labl": "Go"},
        });
        let error = err(decode_layout_node_with_styles(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &diagnostics,
        ));
        assert_eq!(error.code(), "unknown_key");
        assert!(
            error.to_string().contains("\"labl\""),
            "{}",
            error.to_string()
        );
    }

    /// The same typo'd key [`button_leaf_body_rejects_an_unknown_key_under_strict`]
    /// rejects under `strict` only warns once, through the registered hook,
    /// under the default (warn) policy -- the decode still succeeds, since
    /// the dict also carries the real `"label"` key `Button` requires.
    #[test]
    fn button_leaf_body_warns_once_under_default_policy() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let diagnostics = DecodeDiagnostics::warn();
        let value = json!({
            "id": "b1",
            "button": {"id": "b1", "label": "Go", "labl": "Go"},
        });
        decode_layout_node_with_styles(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &diagnostics,
        )
        .expect("warn mode still decodes despite the unrecognised key");
        assert_eq!(
            diagnostics.warned_keys(),
            HashSet::from(["labl".to_string()])
        );
    }

    /// A minimal `"list"`/`"tree"`/`"data_table"` descriptor body carrying
    /// `"scroll_px"` -- the documented `scroll_px_from_value` extension key,
    /// not a field any of `ListView`/`TreeView`/`DataTable` declares (see
    /// `docs/bindings-wire-format.md`'s own `"scroll_px"` entries and
    /// [`crate::scroll_px_from_value`]'s doc). Covers both the layout-tree
    /// leaf shape (`{"id": ..., "list": {...}}`) and the flat per-slot shape
    /// (`{"list": {...}}`) [`decode_render_output_with_styles`] dispatches
    /// between.
    fn scrollable_body_with_scroll_px(kind: &str) -> Value {
        let body = match kind {
            "list" => json!({
                "id": "l1",
                "items": [],
                "selected_idx": 0,
                "scroll_px": 3.5,
            }),
            "tree" => json!({
                "id": "t1",
                "rows": [],
                "selection_mode": "Single",
                "selected_path": null,
                "style": {
                    "indent": 2,
                    "show_chevrons": true,
                    "chevron_expanded": "v",
                    "chevron_collapsed": ">",
                },
                "scroll_px": 1.5,
            }),
            "data_table" => json!({
                "id": "dt1",
                "columns": [{"title": "Name"}],
                "rows": [],
                "scroll_px": 2.5,
            }),
            other => panic!("not a scrollable kind: {other}"),
        };
        json!({ kind: body })
    }

    /// `"scroll_px"` inside a `"list"`/`"tree"`/`"data_table"` descriptor
    /// body decodes cleanly under both `warn` (leaving [`DecodeDiagnostics::warned_keys`]
    /// empty -- no false "unknown key" finding) and `strict` (no
    /// [`DecodeError::UnknownKey`]), on both the layout-tree leaf shape and
    /// the flat per-slot shape [`decode_render_output_with_styles`]
    /// dispatches between. This is the regression
    /// [`button_leaf_body_warns_once_under_default_policy`]'s typo'd-key
    /// coverage doesn't exercise: `scroll_px` is a real, documented key
    /// outside the decoded type's own fields.
    #[test]
    fn scroll_px_in_a_scrollable_body_is_not_reported_as_unknown() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        for kind in ["list", "tree", "data_table"] {
            let body = scrollable_body_with_scroll_px(kind);

            // Flat per-slot shape: `{"list": {...}}` etc., no top-level
            // `"id"`.
            let warn = DecodeDiagnostics::warn();
            decode_render_output_with_styles(
                "test",
                &body,
                StyleContext::new(&sets, &theme),
                &warn,
            )
            .unwrap_or_else(|e| panic!("{kind} flat frame with scroll_px: {e}"));
            assert!(
                warn.warned_keys().is_empty(),
                "{kind} flat frame: scroll_px must not warn as unknown, got {:?}",
                warn.warned_keys()
            );
            let strict = DecodeDiagnostics::strict();
            decode_render_output_with_styles(
                "test",
                &body,
                StyleContext::new(&sets, &theme),
                &strict,
            )
            .unwrap_or_else(|e| panic!("{kind} flat frame with scroll_px under strict: {e}"));

            // Layout-tree leaf shape: the same body dict, now under a
            // top-level `"id"` so `is_layout_tree` routes it through
            // `layout_leaf_from_value` instead.
            let mut leaf = body.as_object().unwrap().clone();
            leaf.insert("id".to_string(), json!("leaf-1"));
            let leaf = Value::Object(leaf);

            let warn = DecodeDiagnostics::warn();
            decode_render_output_with_styles(
                "test",
                &leaf,
                StyleContext::new(&sets, &theme),
                &warn,
            )
            .unwrap_or_else(|e| panic!("{kind} layout-tree leaf with scroll_px: {e}"));
            assert!(
                warn.warned_keys().is_empty(),
                "{kind} layout-tree leaf: scroll_px must not warn as unknown, got {:?}",
                warn.warned_keys()
            );
            let strict = DecodeDiagnostics::strict();
            decode_render_output_with_styles(
                "test",
                &leaf,
                StyleContext::new(&sets, &theme),
                &strict,
            )
            .unwrap_or_else(|e| panic!("{kind} layout-tree leaf with scroll_px under strict: {e}"));
        }
    }

    /// A genuine typo next to `"scroll_px"` in a scrollable body -- e.g.
    /// `"scroll_pxx"` -- still reports as unknown; the fix above only
    /// widens the accepted set by the one documented key, not by anything
    /// else.
    #[test]
    fn a_typo_near_scroll_px_still_rejects_under_strict() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let value = json!({
            "id": "leaf-1",
            "list": {
                "id": "l1",
                "items": [],
                "selected_idx": 0,
                "scroll_pxx": 3.5,
            },
        });
        let strict = DecodeDiagnostics::strict();
        let error = err(decode_render_output_with_styles(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &strict,
        ));
        assert_eq!(error.code(), "unknown_key");
        assert!(
            error.to_string().contains("\"scroll_pxx\""),
            "{}",
            error.to_string()
        );
    }
}
