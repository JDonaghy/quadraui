//! The four MCP tools this crate exposes, and the JSON-RPC-independent
//! glue that turns a `tools/call` request's `arguments` into a
//! [`ControlHandle`] call.
//!
//! Each tool maps 1:1 onto a [`quadraui::control::ControlRequest`]
//! variant — see that module's "Levels" doc section for what each one
//! does to the running app. [`definitions`] is what a `tools/list`
//! response answers with; [`call`] is what a `tools/call` request
//! dispatches through.

use serde_json::{json, Value};

use quadraui::control::ControlHandle;
use quadraui::{A11yAction, A11yActionValue, A11yNodeId, Key, Modifiers};

/// The JSON Schema `inputSchema` every tool declares its arguments
/// against, plus a human-readable `description` — exactly the shape an
/// MCP `tools/list` result's `tools` array holds per entry.
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "snapshot",
            "description": "Returns the running app's current accessibility tree as JSON \
                (quadraui::A11yTree's stable JSON form). Call this first to discover node \
                ids, roles and labels before calling `act`.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "act",
            "description": "Routes an assistive-technology-style action against one node \
                from a prior `snapshot`, the same way a mouse/keyboard user's gesture would \
                (click a button, select a list row, expand a tree item, ...).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": {
                        "type": "integer",
                        "description": "A node id copied from a prior `snapshot` response's \
                            `id` field.",
                    },
                    "action": {
                        "type": "string",
                        "enum": [
                            "click", "focus", "expand", "collapse", "scroll_into_view",
                            "scroll_up", "scroll_down", "set_value", "increment", "decrement",
                        ],
                    },
                    "value": {
                        "description": "The new value for a `set_value` action: a string or \
                            a number. Ignored by every other action.",
                        "oneOf": [{"type": "string"}, {"type": "number"}],
                    },
                },
                "required": ["node", "action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "key",
            "description": "Synthesizes one key press, through the same backend \
                pre-processing pipeline (focus cycling, declared accelerators, ...) real \
                keyboard input goes through.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key": {
                        "description": "quadraui::Key's own JSON form: a single character \
                            as {\"Char\": \"a\"}, or a named key as {\"Named\": \"Enter\"} \
                            (Escape, Tab, BackTab, Enter, Backspace, Delete, Insert, Home, \
                            End, PageUp, PageDown, Up, Down, Left, Right, {\"F\": 1}, \
                            CapsLock, NumLock, ScrollLock, Menu).",
                        "oneOf": [
                            {"type": "object", "properties": {"Char": {"type": "string"}}},
                            {"type": "object", "properties": {"Named": {}}},
                        ],
                    },
                    "modifiers": {
                        "type": "object",
                        "description": "Defaults to no modifiers held.",
                        "properties": {
                            "shift": {"type": "boolean"},
                            "ctrl": {"type": "boolean"},
                            "alt": {"type": "boolean"},
                            "cmd": {"type": "boolean"},
                        },
                        "additionalProperties": false,
                    },
                },
                "required": ["key"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "type",
            "description": "Synthesizes one unmodified key press per character of `text`, \
                left to right.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                },
                "required": ["text"],
                "additionalProperties": false,
            },
        }),
    ]
}

/// Dispatches one `tools/call` request's `name`/`arguments` against
/// `handle`. `Ok` carries the tool's result payload (an MCP `tools/call`
/// response wraps this as one `{"type": "text", ...}` content item —
/// see `crate::rpc`); `Err` carries a human-readable message an MCP
/// client should show to its model, not a JSON-RPC protocol error.
pub fn call(handle: &ControlHandle, name: &str, arguments: &Value) -> Result<Value, String> {
    match name {
        "snapshot" => handle.snapshot().map_err(|e| e.to_string()),
        "act" => call_act(handle, arguments),
        "key" => call_key(handle, arguments),
        "type" => call_type(handle, arguments),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn call_act(handle: &ControlHandle, arguments: &Value) -> Result<Value, String> {
    let node = arguments
        .get("node")
        .and_then(Value::as_u64)
        .ok_or("act: \"node\" must be an integer id from a prior snapshot")?;
    let action_name = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or("act: \"action\" is required")?;
    let action = parse_action(action_name)?;
    let value = parse_action_value(arguments.get("value"))?;
    handle
        .act(A11yNodeId::from_raw(node), action, value)
        .map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

fn call_key(handle: &ControlHandle, arguments: &Value) -> Result<Value, String> {
    let key_value = arguments
        .get("key")
        .cloned()
        .ok_or("key: \"key\" is required")?;
    let key: Key =
        serde_json::from_value(key_value).map_err(|e| format!("key: invalid \"key\": {e}"))?;
    let modifiers: Modifiers = match arguments.get("modifiers") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| format!("key: invalid \"modifiers\": {e}"))?,
        None => Modifiers::default(),
    };
    handle.key(key, modifiers).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

fn call_type(handle: &ControlHandle, arguments: &Value) -> Result<Value, String> {
    let text = arguments
        .get("text")
        .and_then(Value::as_str)
        .ok_or("type: \"text\" is required")?;
    handle.type_text(text).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

/// `A11yAction` has no `Deserialize` impl (only `Serialize`, for a
/// `snapshot`'s `supported_actions` field) — this is the other
/// direction, matching `act`'s own tool schema's `enum` list above
/// verbatim.
fn parse_action(name: &str) -> Result<A11yAction, String> {
    Ok(match name {
        "click" => A11yAction::Click,
        "focus" => A11yAction::Focus,
        "expand" => A11yAction::Expand,
        "collapse" => A11yAction::Collapse,
        "scroll_into_view" => A11yAction::ScrollIntoView,
        "scroll_up" => A11yAction::ScrollUp,
        "scroll_down" => A11yAction::ScrollDown,
        "set_value" => A11yAction::SetValue,
        "increment" => A11yAction::Increment,
        "decrement" => A11yAction::Decrement,
        other => return Err(format!("act: unknown action {other:?}")),
    })
}

/// `A11yActionValue` carries no `serde` derive at all — a `set_value`
/// action's payload is just whichever JSON scalar the caller sent.
fn parse_action_value(value: Option<&Value>) -> Result<Option<A11yActionValue>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(A11yActionValue::Text(s.clone()))),
        Some(Value::Number(n)) => n
            .as_f64()
            .map(|n| Some(A11yActionValue::Number(n)))
            .ok_or_else(|| "act: \"value\" is not a finite number".to_string()),
        Some(other) => Err(format!(
            "act: \"value\" must be a string or number, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_cover_all_four_tools() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["snapshot", "act", "key", "type"]);
    }

    #[test]
    fn call_unknown_tool_is_a_clean_error() {
        let no_op_waker: std::sync::Arc<dyn Fn(quadraui::UserPayload) + Send + Sync> =
            std::sync::Arc::new(|_| {});
        let (handle, _port) = quadraui::control::channel(no_op_waker);
        let err = call(&handle, "bogus", &json!({})).expect_err("unknown tool");
        assert!(err.contains("bogus"));
    }

    #[test]
    fn parse_action_rejects_unknown_strings() {
        assert!(parse_action("click").is_ok());
        assert!(parse_action("nonsense").is_err());
    }

    #[test]
    fn parse_action_value_accepts_string_number_and_absent() {
        assert_eq!(parse_action_value(None).unwrap(), None);
        assert_eq!(
            parse_action_value(Some(&json!("hi"))).unwrap(),
            Some(A11yActionValue::Text("hi".into()))
        );
        assert_eq!(
            parse_action_value(Some(&json!(3.5))).unwrap(),
            Some(A11yActionValue::Number(3.5))
        );
        assert!(parse_action_value(Some(&json!(true))).is_err());
    }
}
