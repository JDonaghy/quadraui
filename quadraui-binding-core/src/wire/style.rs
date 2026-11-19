//! The [`StyleContext`] a layout-tree decode resolves a node's `"class"`
//! key against, and the `"style"` override dict both an inline node key
//! and a binding's `define_style(name, overrides)` surface decode through
//! the same closed [`STYLE_OVERRIDE_KEYS`] vocabulary.

use serde_json::Value;

use quadraui::{Color, Theme};

use crate::layout::{style_override_tokens, StyleOverride, StyleSets};
use crate::DecodeError;

use super::value::{must_be_dict, obj_get, opt_f32, opt_u64, opt_u8};

/// The host state a layout-tree decode resolves a node's `"class"` key
/// against: the [`StyleSets`] registry a binding's own `define_style`
/// surface fills, plus the [`quadraui::Theme`] a built-in set derives its
/// tokens from (see [`StyleSets`]'s own doc for which theme that is and
/// why). Threaded through every node decode rather than resolved in a
/// later pass, so a decoded [`crate::layout::LayoutNode`] is never
/// half-resolved: by the time one exists, its `"class"` references have
/// already folded into the same [`crate::layout::LayoutStyled`] wrapper an
/// inline `"style"` key produces.
#[derive(Clone, Copy)]
pub struct StyleContext<'a> {
    pub(super) sets: &'a StyleSets,
    pub(super) theme: &'a Theme,
}

impl<'a> StyleContext<'a> {
    /// Pairs a style-set registry with the theme its built-in sets derive
    /// against.
    pub fn new(sets: &'a StyleSets, theme: &'a Theme) -> Self {
        Self { sets, theme }
    }
}
/// [`style_override_tokens!`]'s callback for this module: the closed key
/// vocabulary [`STYLE_OVERRIDE_KEYS`] rejects anything outside of, and
/// [`style_override_fields`]'s own per-key decode — generated from the
/// exact same table [`StyleOverride`] itself expands from (`crate::layout`'s
/// own `define_style_override` callback), so a token the struct accepts
/// and a token this decode rejects as unknown can never drift apart.
/// `surface_bg`/`accent_bg` are the semantic aliases [`StyleOverride`]'s
/// doc calls out for a `Button`/`List`/`Dialog`/... fill `bg` alone
/// doesn't reach.
macro_rules! define_style_decode {
    (
        colors: [ $( $cfield:ident => $tfield:ident ),* $(,)? ],
        metrics: [ $( $mfield:ident : $mty:ty => $setter:ident => $decode:ident ),* $(,)? ],
    ) => {
        const STYLE_OVERRIDE_KEYS: &[&str] = &[
            $(stringify!($cfield),)*
            $(stringify!($mfield),)*
        ];

        /// Reads every [`STYLE_OVERRIDE_KEYS`] token off `dict` (already
        /// known to be an object), rejecting any key outside that list —
        /// the same "typo is a decode error, not a silently-dropped key"
        /// policy [`edge_insets_from_value`] already applies to
        /// `"padding"`'s own object shape. `context` prefixes any error
        /// raised.
        fn style_override_fields(context: &str, dict: &Value) -> Result<StyleOverride, DecodeError> {
            // The caller already guaranteed `dict.is_object()`, so this is
            // never `None` -- `.as_object()` just gets at the key set to
            // check.
            for key in dict
                .as_object()
                .expect("the caller guarantees an object")
                .keys()
            {
                if !STYLE_OVERRIDE_KEYS.contains(&key.as_str()) {
                    return Err(DecodeError::unknown_key(
                        context,
                        "style",
                        key.as_str(),
                        STYLE_OVERRIDE_KEYS,
                    ));
                }
            }
            Ok(StyleOverride {
                $($cfield: opt_color(dict, stringify!($cfield), context)?,)*
                $($mfield: $decode(dict, stringify!($mfield), context)?,)*
            })
        }
    };
}
style_override_tokens!(define_style_decode);

/// Decodes an optional `"class"` sibling key found alongside any layout
/// node dict, in the same place [`style_override_from_value`] reads
/// `"style"`: a single string, or a list of strings, each naming a
/// registered [`StyleSets`] entry. Every string involved — the whole value
/// itself, or each list entry — is split on ASCII whitespace, so
/// `{"class": "card danger"}` and `{"class": ["card danger"]}` both name
/// the same two sets — the CSS `class=` attribute's own convention, which
/// this key is the analogue of, applied the same way regardless of which
/// of the two shapes a given name arrived in. Returns an empty `Vec` when
/// the key is absent.
///
/// Rejects a value that is neither a string nor a list of strings (a typed
/// shape error, like every other wrongly-typed key in this decoder), but
/// **not** a name no registry holds: an unknown class resolves to nothing
/// and is silently ignored, the data-format-v1 unknown-key policy an
/// unrecognised sibling key on a node dict already gets. That is the
/// opposite of a `"style"` dict's own closed key vocabulary, deliberately
/// so: a class name is open-ended host data (an app defines its own set
/// names at runtime), while `"style"`'s keys are a fixed, documented token
/// list where a typo can only ever be a typo.
pub(super) fn style_classes_from_value(
    what: &str,
    value: &Value,
) -> Result<Vec<String>, DecodeError> {
    let Some(raw) = obj_get(value, "class") else {
        return Ok(Vec::new());
    };
    let context = format!("{what} › \"class\"");
    match raw {
        Value::String(names) => Ok(names.split_whitespace().map(str::to_owned).collect()),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .ok_or_else(|| DecodeError::type_mismatch(&context, "class[]", "a string"))
            })
            .collect::<Result<Vec<&str>, DecodeError>>()
            .map(|entries| {
                entries
                    .into_iter()
                    .flat_map(str::split_whitespace)
                    .map(str::to_owned)
                    .collect()
            }),
        _ => Err(DecodeError::type_mismatch(
            &context,
            "class",
            "a string or a list of strings",
        )),
    }
}

/// Decodes an optional `"style"` sibling key found alongside any layout
/// node dict (any of `"row"`/`"column"`/`"split"`/`"tabs"`/`"panel"`/
/// `"sidebar_panel"`/`"split_tree"`/`"scroll"`, or a leaf's own `{"id",
/// "<kind>"}` dict) into a [`StyleOverride`]. `None` when the key is
/// absent, or present but empty — [`StyleOverride::is_empty`] folds
/// `{"style": {}}` down to the same `None` a missing key produces, so
/// [`layout_node_from_value`] never wraps a node in a no-op
/// [`crate::layout::LayoutStyled`].
///
/// Rejects any key outside [`STYLE_OVERRIDE_KEYS`] — the same "typo is a
/// decode error, not a silently-dropped key" policy [`edge_insets_from_value`]
/// already applies to `"padding"`'s own object shape. This is stricter
/// than an unrecognised sibling key on the surrounding node dict itself
/// (e.g. `{"row": [...], "bogus": 1}` decodes fine, silently ignoring
/// `"bogus"`) — deliberate, since a `"style"` dict's whole point is a
/// closed, documented vocabulary of theme/style tokens, where a typo'd key
/// silently doing nothing is a worse failure mode than the data format v1
/// unknown-key policy a sibling key gets.
pub(super) fn style_override_from_value(
    what: &str,
    value: &Value,
) -> Result<Option<StyleOverride>, DecodeError> {
    let Some(raw) = obj_get(value, "style") else {
        return Ok(None);
    };
    let dict = must_be_dict(what, raw, "style")?;
    let context = format!("{what} › \"style\"");
    let over = style_override_fields(&context, dict)?;
    Ok(if over.is_empty() { None } else { Some(over) })
}

/// Decodes a bare overrides dict — the same closed
/// [`STYLE_OVERRIDE_KEYS`] vocabulary a node's own `"style"` key carries,
/// but read straight off `value` rather than out of a `"style"` member.
/// The entry point a binding's `define_style(name, overrides)` surface
/// decodes its argument through, so a named style set and an inline
/// `"style"` key accept exactly the same tokens in exactly the same
/// shapes. `what` names the calling surface in any error raised.
pub fn decode_style_overrides(what: &str, value: &Value) -> Result<StyleOverride, DecodeError> {
    let dict = must_be_dict(what, value, "overrides")?;
    style_override_fields(what, dict)
}

/// Validates `name` for a binding's own `define_style(name, overrides)`
/// surface, before it is handed to [`StyleSets::define`]: rejects an empty
/// name (nothing to register under) and a name containing whitespace.
/// The whitespace rule closes a gap [`style_classes_from_value`] would
/// otherwise open — that function always splits a `"class"` string (and,
/// as of this function's own sibling fix, every list entry too) on
/// whitespace, so a name containing it could never be referenced through
/// `"class"` at all, only resolved against by accident if some other code
/// path constructed it directly. CSS forbids whitespace in a class token
/// for the same reason. `what` names the calling surface in any error
/// raised, the same convention [`decode_style_overrides`] uses.
pub fn validate_style_set_name(what: &str, name: &str) -> Result<(), DecodeError> {
    if name.is_empty() {
        return Err(DecodeError::borrow(what, "\"name\" must not be empty"));
    }
    if name.contains(char::is_whitespace) {
        return Err(DecodeError::borrow(
            what,
            format!("\"name\" must not contain whitespace, got {name:?}"),
        ));
    }
    Ok(())
}

/// Reads `value[key]` as an optional colour: a `"#rrggbb"`/`"#rrggbbaa"`
/// hex string (mirroring [`crate::host::theme_color_value`]'s own
/// convenience for a backend-wide `set_theme` override), or any other
/// shape [`Color`]'s derived `Deserialize` accepts (an `{"r","g","b","a"}`
/// object or `[r, g, b[, a]]` array). `None` when absent.
fn opt_color(value: &Value, key: &str, context: &str) -> Result<Option<Color>, DecodeError> {
    match obj_get(value, key) {
        Some(Value::String(hex)) => Color::from_hex(hex).map(Some).ok_or_else(|| {
            DecodeError::borrow(
                context,
                format!("{key:?}: {hex:?} is not a valid \"#rrggbb\"/\"#rrggbbaa\" hex colour"),
            )
        }),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|err| DecodeError::borrow(context, format!("{key:?}: {err}"))),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::layout::LayoutNode;
    use crate::wire::test_support::{decode, err};
    use crate::wire::{decode_layout_node_with_styles, DecodeDiagnostics};

    use super::*;

    /// A `"style"` sibling key wraps whatever node the rest of the dict
    /// decoded to in a [`LayoutNode::Styled`], decoding `"bg"`/`"fg"` as
    /// hex colours.
    #[test]
    fn round_trips_a_style_key_as_a_styled_wrapper() {
        let value = json!({
            "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
            "style": {"bg": "#112233", "fg": "#ffffff"},
        });
        let node = decode(&value).expect("valid styled row");
        let LayoutNode::Styled(styled) = node else {
            panic!("expected a styled wrapper");
        };
        assert_eq!(styled.style.bg, Color::from_hex("#112233"));
        assert_eq!(styled.style.fg, Color::from_hex("#ffffff"));
        assert_eq!(styled.style.border, None);
        assert!(matches!(*styled.node, LayoutNode::Row(_)));
    }

    /// `"style"` works the same on a leaf node as on a container -- it's
    /// a sibling of the leaf's own `"id"`/`"<kind>"` keys, not nested
    /// inside the descriptor.
    #[test]
    fn round_trips_a_style_key_on_a_leaf_node() {
        let value = json!({
            "id": "btn",
            "button": {"id": "btn", "label": "Go"},
            "style": {"accent": "#ff0000", "corner_radius": 6.0},
        });
        let node = decode(&value).expect("valid styled leaf");
        let LayoutNode::Styled(styled) = node else {
            panic!("expected a styled wrapper");
        };
        assert_eq!(styled.style.accent, Color::from_hex("#ff0000"));
        assert_eq!(styled.style.corner_radius, Some(6.0));
        assert!(matches!(*styled.node, LayoutNode::Leaf(_)));
    }

    /// `"surface_bg"`/`"accent_bg"` are the semantic aliases a
    /// `Button`/`List`/`Dialog`/... fill actually reads -- see
    /// `crate::layout::StyleOverride`'s own doc for why a plain `"bg"`
    /// doesn't reach those.
    #[test]
    fn round_trips_surface_bg_and_accent_bg() {
        let value = json!({
            "id": "btn",
            "button": {"id": "btn", "label": "Go"},
            "style": {"surface_bg": "#112233", "accent_bg": "#445566"},
        });
        let node = decode(&value).expect("valid styled leaf");
        let LayoutNode::Styled(styled) = node else {
            panic!("expected a styled wrapper");
        };
        assert_eq!(styled.style.surface_bg, Color::from_hex("#112233"));
        assert_eq!(styled.style.accent_bg, Color::from_hex("#445566"));
    }

    /// Every `Theme` colour field beyond the six semantic aliases is
    /// overridable under its own name, and every `Style` metric field
    /// (`f32`, `u8`, or `u64`) decodes too — [`StyleOverride`]'s own
    /// "Every `Theme`/`Style` token is covered" doc.
    #[test]
    fn every_generated_token_round_trips_not_just_the_six_aliases() {
        let value = json!({
            "id": "btn",
            "button": {"id": "btn", "label": "Go"},
            "style": {
                "tab_bar_bg": "#102030",
                "row_height": 30.0,
                "shadow_elevation": 2,
                "tooltip_delay_ms": 250,
            },
        });
        let node = decode(&value).expect("valid styled leaf");
        let LayoutNode::Styled(styled) = node else {
            panic!("expected a styled wrapper");
        };
        assert_eq!(styled.style.tab_bar_bg, Color::from_hex("#102030"));
        assert_eq!(styled.style.row_height, Some(30.0));
        assert_eq!(styled.style.shadow_elevation, Some(2));
        assert_eq!(styled.style.tooltip_delay_ms, Some(250));
    }

    /// `"shadow_elevation"` decodes through [`super::value::opt_u8`],
    /// which rejects a value outside `0..=255` rather than silently
    /// wrapping it.
    #[test]
    fn shadow_elevation_rejects_a_value_outside_u8_range() {
        let value = json!({
            "row": [],
            "style": {"shadow_elevation": 1000},
        });
        let error = err(decode(&value));
        assert!(
            error.to_string().contains("shadow_elevation"),
            "{}",
            error.to_string()
        );
    }

    /// An empty `"style": {}` is a no-op -- decoding falls through to the
    /// plain (unwrapped) node, exactly as if the key were absent.
    #[test]
    fn an_empty_style_dict_does_not_wrap_the_node() {
        let value = json!({
            "row": [],
            "style": {},
        });
        let node = decode(&value).expect("valid row");
        assert!(matches!(node, LayoutNode::Row(_)));
    }

    /// Nested `"style"` keys compose: an inner override only narrows the
    /// fields it names, so the outer `"bg"` still applies to anything
    /// inside the inner node that doesn't override it itself.
    #[test]
    fn nested_style_keys_each_wrap_their_own_node() {
        let value = json!({
            "row": [{
                "id": "a",
                "button": {"id": "a", "label": "A"},
                "style": {"fg": "#00ff00"},
            }],
            "style": {"bg": "#000000"},
        });
        let node = decode(&value).expect("valid nested styled tree");
        let LayoutNode::Styled(outer) = node else {
            panic!("expected an outer styled wrapper");
        };
        assert_eq!(outer.style.bg, Color::from_hex("#000000"));
        let LayoutNode::Row(container) = outer.node.as_ref() else {
            panic!("expected the wrapped node to be a row");
        };
        let LayoutNode::Styled(inner) = &container.children[0].node else {
            panic!("expected the row's own child to carry its own styled wrapper");
        };
        assert_eq!(inner.style.fg, Color::from_hex("#00ff00"));
        assert_eq!(inner.style.bg, None);
    }

    /// An unknown `"style"` key is a decode error naming it, the same
    /// policy `"padding"`'s own object shape already enforces.
    #[test]
    fn style_rejects_an_unknown_key() {
        let value = json!({
            "row": [],
            "style": {"background": "#000000"},
        });
        let error = err(decode(&value));
        assert!(
            error.to_string().contains("unknown key"),
            "{}",
            error.to_string()
        );
        assert!(
            error.to_string().contains("\"background\""),
            "{}",
            error.to_string()
        );
    }

    /// `"style.bg"` rejects a malformed hex string instead of silently
    /// falling back to some default colour.
    #[test]
    fn style_rejects_an_invalid_hex_colour() {
        let value = json!({
            "row": [],
            "style": {"bg": "not-a-colour"},
        });
        let error = err(decode(&value));
        assert!(
            error.to_string().contains("\"bg\""),
            "{}",
            error.to_string()
        );
    }

    /// `"style"` must itself be an object, not a bare value.
    #[test]
    fn style_rejects_a_non_object_value() {
        let value = json!({
            "row": [],
            "style": "bg",
        });
        let error = err(decode(&value));
        assert!(
            error.to_string().contains("\"style\""),
            "{}",
            error.to_string()
        );
        assert!(
            error.to_string().contains("must be an object"),
            "{}",
            error.to_string()
        );
    }

    /// Decodes `value` against a registry holding exactly `sets` on top of
    /// the built-ins, with the default theme -- the `"class"` tests'
    /// counterpart of [`decode`].
    fn decode_with_sets(value: &Value, sets: &[(&str, StyleOverride)]) -> LayoutNode {
        let mut registry = StyleSets::new();
        for (name, overrides) in sets {
            registry.define(*name, *overrides);
        }
        let theme = Theme::default();
        decode_layout_node_with_styles(
            "test",
            value,
            StyleContext::new(&registry, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid tree")
    }

    /// The [`StyleOverride`] a decoded node's own `Styled` wrapper
    /// carries, panicking if the node isn't wrapped at all.
    fn styled_override(node: &LayoutNode) -> StyleOverride {
        let LayoutNode::Styled(styled) = node else {
            panic!("expected a styled wrapper");
        };
        *styled.style
    }

    fn set_with_fg(hex: &str) -> StyleOverride {
        StyleOverride {
            fg: Color::from_hex(hex),
            ..StyleOverride::default()
        }
    }

    /// A `"class"` naming one defined set resolves to that set's tokens,
    /// wrapping the node exactly the way an inline `"style"` key does.
    #[test]
    fn a_class_resolves_to_its_defined_sets_tokens() {
        let value = json!({
            "row": [],
            "class": "card",
        });
        let node = decode_with_sets(
            &value,
            &[(
                "card",
                StyleOverride {
                    bg: Color::from_hex("#112233"),
                    padding: Some(2.0),
                    ..StyleOverride::default()
                },
            )],
        );
        let style = styled_override(&node);
        assert_eq!(style.bg, Color::from_hex("#112233"));
        assert_eq!(style.padding, Some(2.0));
    }

    /// Classes merge left to right: a later name wins a token an earlier
    /// one also named, while a token only the earlier one named survives.
    #[test]
    fn later_classes_win_earlier_ones_token_by_token() {
        let both = json!({"row": [], "class": ["first", "second"]});
        let reversed = json!({"row": [], "class": ["second", "first"]});
        let sets = [
            (
                "first",
                StyleOverride {
                    fg: Color::from_hex("#ff0000"),
                    bg: Color::from_hex("#000011"),
                    ..StyleOverride::default()
                },
            ),
            ("second", set_with_fg("#00ff00")),
        ];
        let forward = styled_override(&decode_with_sets(&both, &sets));
        assert_eq!(forward.fg, Color::from_hex("#00ff00"), "\"second\" wins fg");
        assert_eq!(
            forward.bg,
            Color::from_hex("#000011"),
            "a token only \"first\" named survives the merge"
        );
        let backward = styled_override(&decode_with_sets(&reversed, &sets));
        assert_eq!(
            backward.fg,
            Color::from_hex("#ff0000"),
            "reversing the list reverses which name wins"
        );
    }

    /// A single `"class"` string is whitespace-split into several names,
    /// the CSS `class=` convention -- same result as the array form.
    #[test]
    fn a_whitespace_separated_class_string_names_several_sets() {
        let sets = [
            (
                "first",
                StyleOverride {
                    fg: Color::from_hex("#ff0000"),
                    bg: Color::from_hex("#000011"),
                    ..StyleOverride::default()
                },
            ),
            ("second", set_with_fg("#00ff00")),
        ];
        let as_string = styled_override(&decode_with_sets(
            &json!({"row": [], "class": "first second"}),
            &sets,
        ));
        let as_array = styled_override(&decode_with_sets(
            &json!({"row": [], "class": ["first", "second"]}),
            &sets,
        ));
        assert_eq!(as_string, as_array);
    }

    /// A whitespace-separated array entry splits into several names too --
    /// `{"class": ["first second"]}` names the same two sets as
    /// `{"class": "first second"}`, not one set named `"first second"`.
    #[test]
    fn a_whitespace_separated_array_entry_also_names_several_sets() {
        let sets = [
            (
                "first",
                StyleOverride {
                    fg: Color::from_hex("#ff0000"),
                    bg: Color::from_hex("#000011"),
                    ..StyleOverride::default()
                },
            ),
            ("second", set_with_fg("#00ff00")),
        ];
        let as_string = styled_override(&decode_with_sets(
            &json!({"row": [], "class": "first second"}),
            &sets,
        ));
        let as_array_entry = styled_override(&decode_with_sets(
            &json!({"row": [], "class": ["first second"]}),
            &sets,
        ));
        assert_eq!(as_string, as_array_entry);
    }

    /// An inline `"style"` key beats every class the same node names --
    /// a CSS `style=` attribute beating a `class=` rule.
    #[test]
    fn an_inline_style_key_wins_over_the_nodes_own_classes() {
        let value = json!({
            "row": [],
            "class": "card",
            "style": {"fg": "#0000ff"},
        });
        let node = decode_with_sets(
            &value,
            &[(
                "card",
                StyleOverride {
                    fg: Color::from_hex("#ff0000"),
                    bg: Color::from_hex("#000011"),
                    ..StyleOverride::default()
                },
            )],
        );
        let style = styled_override(&node);
        assert_eq!(style.fg, Color::from_hex("#0000ff"), "inline wins");
        assert_eq!(
            style.bg,
            Color::from_hex("#000011"),
            "a class token the inline style didn't name still applies"
        );
    }

    /// Redefining a set under a name already in use replaces it wholesale
    /// -- the very next decode resolves to the new tokens, since nothing
    /// caches a resolved override.
    #[test]
    fn redefining_a_set_changes_what_the_next_decode_resolves_to() {
        let value = json!({"row": [], "class": "card"});
        let mut registry = StyleSets::new();
        let theme = Theme::default();
        registry.define("card", set_with_fg("#ff0000"));
        let before = decode_layout_node_with_styles(
            "test",
            &value,
            StyleContext::new(&registry, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid tree");
        assert_eq!(styled_override(&before).fg, Color::from_hex("#ff0000"));

        registry.define("card", set_with_fg("#00ff00"));
        let after = decode_layout_node_with_styles(
            "test",
            &value,
            StyleContext::new(&registry, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid tree");
        assert_eq!(styled_override(&after).fg, Color::from_hex("#00ff00"));
    }

    /// An unknown class name is silently ignored -- the data-format-v1
    /// unknown-key policy, not the closed-vocabulary error a `"style"`
    /// key's own unknown token raises. A node whose only class is unknown
    /// isn't wrapped at all.
    #[test]
    fn an_unknown_class_name_is_ignored_rather_than_rejected() {
        let node = decode_with_sets(&json!({"row": [], "class": "nope"}), &[]);
        assert!(
            matches!(node, LayoutNode::Row(_)),
            "an unknown class resolves to nothing, so nothing wraps the node"
        );
        let mixed = decode_with_sets(
            &json!({"row": [], "class": ["nope", "card"]}),
            &[("card", set_with_fg("#00ff00"))],
        );
        assert_eq!(
            styled_override(&mixed).fg,
            Color::from_hex("#00ff00"),
            "the known name in the list still applies"
        );
    }

    /// The built-in sets exist without any `define_style` call, and each
    /// derives from the theme passed in rather than from a literal colour
    /// -- a custom palette moves them with it.
    #[test]
    fn builtin_sets_derive_from_the_theme_they_resolve_against() {
        let value = json!({"id": "btn", "button": {"id": "btn", "label": "Go"}, "class": "danger"});
        let registry = StyleSets::new();
        let theme = Theme::default();
        let style = styled_override(
            &decode_layout_node_with_styles(
                "test",
                &value,
                StyleContext::new(&registry, &theme),
                &DecodeDiagnostics::warn(),
            )
            .expect("valid tree"),
        );
        assert_eq!(style.surface_bg, Some(theme.button_danger_bg()));
        assert_eq!(style.accent_bg, Some(theme.button_danger_bg()));

        let recoloured = Theme {
            error_fg: Color::rgb(1, 2, 3),
            ..Theme::default()
        };
        let style = styled_override(
            &decode_layout_node_with_styles(
                "test",
                &value,
                StyleContext::new(&registry, &recoloured),
                &DecodeDiagnostics::warn(),
            )
            .expect("valid tree"),
        );
        assert_eq!(style.surface_bg, Some(Color::rgb(1, 2, 3)));
    }

    /// `define_style` under a built-in's own name replaces it.
    #[test]
    fn a_defined_set_replaces_a_builtin_of_the_same_name() {
        let node = decode_with_sets(
            &json!({"row": [], "class": "danger"}),
            &[("danger", set_with_fg("#00ff00"))],
        );
        let style = styled_override(&node);
        assert_eq!(style.fg, Color::from_hex("#00ff00"));
        assert_eq!(
            style.surface_bg, None,
            "the replacement's tokens are the only ones left"
        );
    }

    /// `"class"` must be a string or a list of strings -- a wrongly typed
    /// value is a decode error, unlike an unknown *name*.
    #[test]
    fn class_rejects_a_wrongly_typed_value() {
        let error = err(decode(&json!({"row": [], "class": 7})));
        assert!(
            error.to_string().contains("\"class\""),
            "{}",
            error.to_string()
        );
        let error = err(decode(&json!({"row": [], "class": ["ok", 7]})));
        assert!(
            error.to_string().contains("\"class[]\""),
            "{}",
            error.to_string()
        );
    }

    /// `validate_style_set_name` rejects both an empty name and one
    /// containing whitespace -- the latter closes the gap a `"class"`
    /// string/array entry splits on whitespace: a name that itself
    /// contained whitespace could never be referenced through either
    /// `"class"` shape.
    #[test]
    fn validate_style_set_name_rejects_empty_and_whitespace_names() {
        assert!(validate_style_set_name("define_style", "card").is_ok());

        let error = validate_style_set_name("define_style", "").unwrap_err();
        assert!(
            error.to_string().contains("must not be empty"),
            "{}",
            error.to_string()
        );

        let error = validate_style_set_name("define_style", "card danger").unwrap_err();
        assert!(
            error.to_string().contains("must not contain whitespace"),
            "{}",
            error.to_string()
        );
    }
}
