//! Decodes a content-bearing overlay descriptor — `{"float": {...},
//! "content": {...}}` for a popover, `{"dialog": {...}, "content": {...}}`
//! for a layout dialog — sharing both their required-key shape
//! ([`overlay_content`]) and their content subtree's decode with every
//! other layout-tree entry point.

use serde::Deserializer;

use quadraui::WidgetId;

use crate::layout::{validate_unique_leaf_ids, LayoutNode};
use crate::{decode_value, DecodeError};

use super::layout::layout_node_from_value;
use super::style::StyleContext;
use super::value::{check_unknown_keys, materialize, must_be_dict, obj_get, DecodeDiagnostics};

// ─── Content-bearing overlay descriptors ───────────────────────────────────

/// A content-bearing overlay descriptor, decoded: the overlay's own chrome
/// (`T` is [`quadraui::Float`] for a popover, [`quadraui::Dialog`] for a
/// layout dialog) plus the [`LayoutNode`] subtree the engine lays out and
/// paints inside it — [`crate::InteractionEngine::show_popover_layout`] /
/// [`crate::InteractionEngine::open_layout_dialog`]'s two arguments, in
/// the shape a binding hands them over.
pub struct OverlayContent<T> {
    /// The overlay's own chrome descriptor, from the `"float"` /
    /// `"dialog"` key.
    pub chrome: T,
    /// The subtree, from the `"content"` key.
    pub content: LayoutNode,
}

/// Decodes a `{"float": {...}, "content": {...}}` popover descriptor —
/// the wire shape a binding's `show_overlay("popover", ...)` surface
/// accepts. `"float"` decodes as a plain [`quadraui::Float`] descriptor,
/// `"content"` as one layout-tree node through [`crate::wire::decode_layout_node`] (so
/// every node shape a render callback can return is equally valid inside
/// an overlay, nested to any depth).
pub fn decode_popover<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<OverlayContent<quadraui::Float>, DecodeError> {
    overlay_content(what, deserializer, "float", styles, diagnostics)
}

/// Decodes a `{"dialog": {...}, "content": {...}}` layout-dialog
/// descriptor — the wire shape a binding's
/// `show_overlay("layout_dialog", ...)` surface accepts. Mirrors
/// [`decode_popover`], with the dialog's own chrome
/// ([`DialogChromeWire`]'s friendly plain-string `"title"`/`"body"`
/// shape, the same one the `"message_dialog"` overlay kind already takes)
/// in place of the float's.
pub fn decode_layout_dialog<'de, D: Deserializer<'de>>(
    what: &str,
    deserializer: D,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<OverlayContent<quadraui::Dialog>, DecodeError> {
    let decoded: OverlayContent<DialogChromeWire> =
        overlay_content(what, deserializer, "dialog", styles, diagnostics)?;
    Ok(OverlayContent {
        chrome: decoded.chrome.into_dialog(what)?,
        content: decoded.content,
    })
}

/// A layout dialog's `"dialog"` chrome key, as a binding sends it: plain
/// `"title"`/`"body"` strings rather than [`quadraui::StyledText`]
/// objects (`"body"` splitting on `\n` into one line each, exactly like
/// [`quadraui::compose::MessageDialogController::new`] does with
/// [`quadraui::backend::MessageDialogOptions::body`]) and a friendly
/// lowercase `"severity"` word, so the shape matches the
/// `"message_dialog"` overlay kind a binding already exposes. `"buttons"`
/// is omittable — a content-bearing dialog may rely on its own subtree
/// (a form's own Cancel/Save row, a close icon) rather than the chrome's
/// button row to resolve. An empty list routes every event into the
/// content subtree first and leaves the dialog open on an Escape neither
/// the subtree nor a cancel/default button claims; closing then comes
/// from the content itself or the overlay being dismissed by id.
#[derive(serde::Deserialize)]
struct DialogChromeWire {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    buttons: Vec<quadraui::DialogButton>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    vertical_buttons: bool,
}

impl DialogChromeWire {
    fn into_dialog(self, what: &str) -> Result<quadraui::Dialog, DecodeError> {
        let severity = self
            .severity
            .map(|s| crate::host::dialog_severity(&s))
            .transpose()
            .map_err(|err| DecodeError::borrow(what, err))?;
        Ok(quadraui::Dialog {
            id: WidgetId::new(self.id),
            title: quadraui::StyledText::plain(self.title),
            body: if self.body.is_empty() {
                Vec::new()
            } else {
                self.body.lines().map(quadraui::StyledText::plain).collect()
            },
            buttons: self.buttons,
            severity,
            vertical_buttons: self.vertical_buttons,
            table: None,
            input: None,
        })
    }
}

/// [`decode_popover`]/[`decode_layout_dialog`]'s shared body: both keys
/// are required (an overlay with no chrome has nothing to anchor or
/// frame; one with no content is a plain float/dialog the binding's own
/// `"float"`/`"message_dialog"` kinds already cover), and both are
/// decoded from the same materialized [`Value`] tree every other decoder
/// in this module works against.
fn overlay_content<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    what: &str,
    deserializer: D,
    chrome_key: &str,
    styles: StyleContext<'_>,
    diagnostics: &DecodeDiagnostics,
) -> Result<OverlayContent<T>, DecodeError> {
    let value = materialize(deserializer)
        .map_err(|err| DecodeError::borrow(what, format!("invalid descriptor: {err}")))?;
    check_unknown_keys(
        diagnostics,
        what,
        "overlay",
        &value,
        &[chrome_key, "content"],
    )?;
    let chrome =
        obj_get(&value, chrome_key).ok_or_else(|| DecodeError::missing_field(what, chrome_key))?;
    must_be_dict(what, chrome, chrome_key)?;
    let chrome = decode_value(what, chrome)?;
    let content =
        obj_get(&value, "content").ok_or_else(|| DecodeError::missing_field(what, "content"))?;
    let content =
        layout_node_from_value(&format!("{what} › content"), content, styles, diagnostics)?;
    validate_unique_leaf_ids(what, &content)?;
    Ok(OverlayContent { chrome, content })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use quadraui::Theme;

    use crate::layout::StyleSets;
    use crate::wire::test_support::err;

    use super::*;

    /// A popover descriptor round-trips its `"float"` chrome and decodes
    /// its `"content"` key as a full layout-tree node, not just a leaf.
    #[test]
    fn round_trips_a_popover_descriptor() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let value = json!({
            "float": {
                "id": "popover:menu",
                "anchor": {
                    "rect": {"x": 1.0, "y": 2.0, "width": 3.0, "height": 1.0},
                    "preferred": "Bottom",
                    "margin": 0.0,
                },
            },
            "content": {
                "column": [
                    {"id": "a", "button": {"id": "a", "label": "A"}},
                    {"id": "b", "button": {"id": "b", "label": "B"}},
                ]
            },
        });
        let decoded = decode_popover(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid popover");
        assert_eq!(decoded.chrome.id.as_str(), "popover:menu");
        let LayoutNode::Column(column) = decoded.content else {
            panic!("expected the content to decode as a column node");
        };
        assert_eq!(column.children.len(), 2);
        // The chrome's `transparent` key is optional; omitting it leaves
        // the float opaque, the same default `Float::new` picks.
        assert!(!decoded.chrome.transparent);
    }

    /// A binding can ask for a transparent (fill-free) popover by setting
    /// the `"transparent"` key on the `"float"` chrome.
    #[test]
    fn popover_chrome_carries_the_transparent_flag() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let value = json!({
            "float": {
                "id": "popover:banner",
                "anchor": {
                    "rect": {"x": 1.0, "y": 2.0, "width": 3.0, "height": 1.0},
                    "preferred": "Top",
                    "margin": 0.0,
                },
                "border": false,
                "transparent": true,
            },
            "content": {"id": "a", "button": {"id": "a", "label": "A"}},
        });
        let decoded = decode_popover(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid popover");
        assert!(decoded.chrome.transparent);
        assert!(!decoded.chrome.border);
    }

    /// A layout-dialog descriptor round-trips its `"dialog"` chrome
    /// (title and buttons included) alongside its content subtree.
    #[test]
    fn round_trips_a_layout_dialog_descriptor() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let value = json!({
            "dialog": {
                "id": "dialog:prefs",
                "title": "Preferences",
                "buttons": [{"id": "ok", "label": "OK", "is_default": true, "is_cancel": false}],
            },
            "content": {"id": "a", "button": {"id": "a", "label": "A"}},
        });
        let decoded = decode_layout_dialog(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("valid layout dialog");
        assert_eq!(decoded.chrome.id.as_str(), "dialog:prefs");
        assert_eq!(decoded.chrome.buttons.len(), 1);
        assert!(matches!(decoded.content, LayoutNode::Leaf(_)));
    }

    /// A content-bearing dialog's own `"dialog"` chrome can omit
    /// `"buttons"` entirely -- a form whose content subtree supplies its
    /// own Cancel/Save row has no use for the chrome's button row too.
    #[test]
    fn a_layout_dialog_with_no_buttons_decodes() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let value = json!({
            "dialog": {
                "id": "dialog:prefs",
                "title": "Preferences",
            },
            "content": {"id": "a", "button": {"id": "a", "label": "A"}},
        });
        let decoded = decode_layout_dialog(
            "test",
            &value,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        )
        .expect("a dialog with no buttons decodes");
        assert_eq!(decoded.chrome.id.as_str(), "dialog:prefs");
        assert!(decoded.chrome.buttons.is_empty());
    }

    /// Both keys are required: an overlay with no chrome has nothing to
    /// anchor or frame, and one with no content is a plain float the
    /// `"float"` overlay kind already covers.
    #[test]
    fn rejects_a_popover_descriptor_missing_either_key() {
        let sets = StyleSets::new();
        let theme = Theme::default();
        let no_content = json!({
            "float": {
                "id": "p",
                "anchor": {
                    "rect": {"x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0},
                    "preferred": "Bottom",
                    "margin": 0.0,
                },
            }
        });
        let message = err(decode_popover(
            "test",
            &no_content,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        ))
        .to_string();
        assert!(
            message.contains("\"content\""),
            "the error should name the missing key: {message}"
        );
        let no_chrome = json!({"content": {"id": "a", "button": {"id": "a", "label": "A"}}});
        let message = err(decode_popover(
            "test",
            &no_chrome,
            StyleContext::new(&sets, &theme),
            &DecodeDiagnostics::warn(),
        ))
        .to_string();
        assert!(
            message.contains("\"float\""),
            "the error should name the missing key: {message}"
        );
    }
}
