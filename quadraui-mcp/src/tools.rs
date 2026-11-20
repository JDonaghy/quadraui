//! This crate's four built-in "Level 1" MCP tools (`snapshot`, `act`,
//! `key`, `type`), plus "Level 2" — every app-defined named command
//! surfaced as its own tool — and the JSON-RPC-independent glue that
//! turns a `tools/call` request's `arguments` into a [`ControlHandle`]
//! call. See [`quadraui::control`]'s "Levels" doc section for what each
//! level means.
//!
//! Each built-in tool maps 1:1 onto a [`quadraui::control::ControlRequest`]
//! variant. [`definitions`] is what a `tools/list` response answers
//! with — the four built-ins followed by whatever Level 2 commands the
//! live app behind `handle` has registered
//! ([`ControlHandle::list_commands`]), each one surfaced using the
//! command's own name/description/schema. [`call`] is what a
//! `tools/call` request dispatches through: a name that isn't one of
//! the four built-ins is forwarded to [`ControlHandle::command`]
//! unchanged, so a Level 2 command "just works" as an MCP tool without
//! this module needing to know its name ahead of time.

use serde_json::{json, Value};

use quadraui::control::ControlHandle;
use quadraui::{A11yAction, A11yActionValue, A11yNodeId, Key, Modifiers};

/// Builds a `tools/list` response for the running app behind `handle`:
/// this crate's four built-in Level 1 tools, followed by one tool per
/// Level 2 command the app has registered
/// ([`ControlHandle::list_commands`]), using that command's own
/// name/description/schema as the tool's `name`/`description`/
/// `inputSchema`. An app with no Level 2 commands (the default) gets
/// just the four built-ins. A [`ControlHandle::list_commands`] failure
/// (the runner already gone) is silently treated as "no Level 2
/// commands" here — `tools/list` always has at least the four built-ins
/// to answer with; a caller that cares finds out the runner is gone the
/// moment it actually tries to call one.
pub fn definitions(handle: &ControlHandle) -> Vec<Value> {
    let mut defs = builtin_definitions();
    if let Ok(specs) = handle.list_commands() {
        defs.extend(specs.into_iter().map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.schema,
            })
        }));
    }
    defs
}

/// This crate's four built-in Level 1 tools, independent of any running
/// app.
fn builtin_definitions() -> Vec<Value> {
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
///
/// `name` outside the four built-ins is forwarded to
/// [`ControlHandle::command`] unchanged — a Level 2 command the app
/// registered, or [`quadraui::control::CommandError::NotFound`]'s
/// message if nothing matches.
pub fn call(handle: &ControlHandle, name: &str, arguments: &Value) -> Result<Value, String> {
    match name {
        "snapshot" => handle.snapshot().map_err(|e| e.to_string()),
        "act" => call_act(handle, arguments),
        "key" => call_key(handle, arguments),
        "type" => call_type(handle, arguments),
        other => handle
            .command(other, arguments.clone())
            .map_err(|e| e.to_string()),
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
    use quadraui::control::{CommandError, CommandRegistry, CommandSpec};
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{AppLogic, Backend, Reaction, UiEvent};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// An `AppLogic` with no Level 2 commands at all — the default every
    /// `AppLogic` gets for free, so `definitions`/`call` against it
    /// exercise exactly the "no app commands registered" path.
    struct NoOpApp;

    impl AppLogic for NoOpApp {
        type AreaId = ();
        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// An `AppLogic` whose model is a selected index, with `select(index)`
    /// registered against a [`CommandRegistry`] stashed on itself — a
    /// Level 2 command this module's `definitions`/`call` must surface
    /// and route to, same as the four built-ins.
    struct SelectApp {
        selected: usize,
        commands: CommandRegistry<SelectApp>,
    }

    impl SelectApp {
        fn new() -> Self {
            let mut commands = CommandRegistry::new();
            commands.register(
                "select",
                "Selects an item by index.",
                json!({"type": "object", "properties": {"index": {"type": "integer"}}}),
                |app: &mut SelectApp, args: Value| {
                    let index = args
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| CommandError::InvalidArgs("\"index\" required".into()))?
                        as usize;
                    app.selected = index;
                    Ok(json!({"selected": index}))
                },
            );
            Self {
                selected: 0,
                commands,
            }
        }
    }

    impl AppLogic for SelectApp {
        type AreaId = ();
        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
        fn command_specs(&self) -> Vec<CommandSpec> {
            self.commands.specs()
        }
        fn run_command(&mut self, name: &str, args: Value) -> Result<Value, CommandError> {
            let mut commands = std::mem::take(&mut self.commands);
            let result = commands.run(self, name, args);
            self.commands = commands;
            result
        }
    }

    /// Runs `body` against a [`ControlHandle`] backed by a
    /// [`TuiDriver`]-driven `app`, servicing the control port on this
    /// (the test's own) thread in a loop until `body` — run on a
    /// background thread, since `body`'s calls block for a reply — is
    /// done. `TuiDriver` is not `Send` (it owns an `Rc`-based ratatui
    /// terminal), so it has to stay here rather than move into the
    /// spawned thread; only the `ControlHandle` crosses threads.
    fn with_serviced_handle<A, T>(
        app: A,
        body: impl FnOnce(ControlHandle) -> T + Send + 'static,
    ) -> T
    where
        A: AppLogic,
        T: Send + 'static,
    {
        let mut driver = TuiDriver::new(app, 20, 5);
        let waker = driver.backend().waker();
        let (handle, port) = quadraui::control::channel(waker);

        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let worker = thread::spawn(move || {
            let result = body(handle);
            worker_done.store(true, Ordering::SeqCst);
            result
        });
        while !done.load(Ordering::SeqCst) {
            driver.service_control(&port);
            thread::sleep(Duration::from_millis(1));
        }
        worker.join().expect("worker thread panicked")
    }

    #[test]
    fn definitions_cover_all_four_tools_with_no_app_commands() {
        let names = with_serviced_handle(NoOpApp, |handle| {
            definitions(&handle)
                .into_iter()
                .map(|d| d["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(names, vec!["snapshot", "act", "key", "type"]);
    }

    /// `definitions` appends one tool per registered Level 2 command,
    /// using that command's own description/schema as the tool's
    /// `description`/`inputSchema`.
    #[test]
    fn definitions_append_registered_level_2_commands() {
        let defs = with_serviced_handle(SelectApp::new(), |handle| definitions(&handle));
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["snapshot", "act", "key", "type", "select"]);
        let select = defs.last().unwrap();
        assert_eq!(select["description"], json!("Selects an item by index."));
        assert_eq!(
            select["inputSchema"],
            json!({"type": "object", "properties": {"index": {"type": "integer"}}})
        );
    }

    /// A `tools/call` name that isn't one of the four built-ins is
    /// forwarded to the app's own registered command and really mutates
    /// its model — not just a passthrough that happens to return the
    /// right shape.
    #[test]
    fn call_routes_an_unrecognised_name_to_a_registered_level_2_command() {
        let value = with_serviced_handle(SelectApp::new(), |handle| {
            call(&handle, "select", &json!({"index": 2})).expect("select succeeds")
        });
        assert_eq!(value, json!({"selected": 2}));
    }

    #[test]
    fn call_unknown_tool_is_a_clean_error() {
        let err = with_serviced_handle(NoOpApp, |handle| {
            call(&handle, "bogus", &json!({})).expect_err("unknown tool")
        });
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
