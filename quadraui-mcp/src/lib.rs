//! MCP (Model Context Protocol) server over a running [`quadraui`] app's
//! [`quadraui::control`] channel.
//!
//! Any `quadraui` app — TUI, GTK, macOS, or Windows, whichever backend
//! it's built against — already exposes a backend-neutral
//! [`quadraui::control::ControlHandle`] once it opts into the `control`
//! feature and wires `attach_control` into its own runner. This crate
//! turns that handle into an MCP server: a JSON-RPC 2.0 endpoint an AI
//! agent connects to and drives through five built-in tools, `snapshot`,
//! `act`, `key`, `type` and `screenshot`, each one a thin wrapper over
//! the matching [`quadraui::control::ControlRequest`] variant, plus one
//! tool per app-defined named command the app has registered (see the
//! "Levels" section below).
//!
//! ```no_run
//! use std::sync::Arc;
//! use quadraui_mcp::Transport;
//!
//! # fn get_handle() -> quadraui::control::ControlHandle { unreachable!() }
//! // `handle` comes from the app's own runner — e.g.
//! // `TuiRunner::attach_control` on TUI.
//! let handle = get_handle();
//! quadraui_mcp::serve(handle, Transport::Stdio).unwrap();
//! ```
//!
//! # Transports
//!
//! A headless or GUI app can share its own stdin/stdout with an MCP
//! client ([`Transport::Stdio`]). A TUI app owns the terminal's stdio for
//! rendering, so it needs a socket an agent can attach to from a second
//! process instead: [`Transport::Unix`] on Unix, or the cross-platform
//! [`Transport::Tcp`] fallback (also this crate's Windows story — see
//! that variant's own doc). [`Transport::parse`] reads an env-var-style
//! spec such as `QUADRAUI_MCP=unix:/tmp/app.sock` or `QUADRAUI_MCP=tcp:
//! 127.0.0.1:7777`; see `transport`'s module doc for the on-the-wire
//! framing every transport shares.
//!
//! # No MCP SDK, no async runtime
//!
//! The protocol surface this crate implements — the `initialize`
//! handshake, `tools/list`, `tools/call`, four tool schemas — is small
//! enough that hand-rolling it over `serde_json` keeps the dependency
//! list to just `quadraui` and `serde_json`. Every MCP SDK on crates.io
//! at the time of writing pulls in `tokio` to get there, which would
//! force an async runtime onto every `quadraui` app that links this
//! crate purely to be driven by an agent; this crate's transports
//! (`transport.rs`) use one blocking OS thread per connection instead.
//!
//! # Levels
//!
//! "Level 1" per the control channel's own doc: UI-level `snapshot`/
//! `act`, built entirely on `quadraui::a11y`, plus the optional
//! `screenshot`, so any `AppLogic` app gets all five built-in tools for
//! free the moment it wires up `control` (`screenshot` still answers an
//! error on a backend with nothing to capture — see its own tool
//! description). "Level 2": app-defined named commands that act on the
//! app's own model rather than its UI, registered against a
//! [`quadraui::control::CommandRegistry`] and wired into
//! [`quadraui::AppLogic::command_specs`]/[`quadraui::AppLogic::run_command`].
//! `tools/list` lists every Level 2 command alongside the five built-ins
//! (`tools::definitions`), and `tools/call` routes any name outside
//! those five straight to it (`tools::call`) — an app with no Level 2
//! commands just keeps the five built-ins, zero behavior change.
//!
//! # Driving a quadraui app from an agent
//!
//! A step-by-step walkthrough of the four pieces above, tied together.
//! `quadraui`'s `examples/tui_mcp_demo.rs` is the running example this
//! section describes — a three-pane app (list, detail, text) with two
//! Level 2 commands (`filter`, `clear_filter`) alongside the four
//! built-ins; read its own module doc for the exact commands to run.
//!
//! ## 1. Enable it
//!
//! Build the app with `--features control` plus whatever backend
//! feature it already uses (`tui`, `gtk`, `macos` — `win`'s runner has
//! no `attach_control` yet), attach a control channel from the running
//! runner once (`TuiRunner::attach_control`/`GtkRunner::attach_control`/
//! `MacRunner::attach_control`), and hand the resulting
//! `quadraui::control::ControlHandle` to [`serve`] on a background
//! thread — see [`serve`]'s own doc for the exact call and why it needs
//! its own thread on an app that also owns an event loop. `key` and
//! `type` work immediately, built entirely on the app's own event
//! handling. `snapshot`, `act` and `screenshot` additionally need a
//! captured frame to read: call `quadraui::set_frame_capture(backend,
//! true)` once, from `AppLogic::setup` (the hook every runner calls
//! before the first render — see `tui_mcp_demo.rs`'s own `setup` impl),
//! since neither the runner nor `attach_control` turns capture on by
//! itself.
//!
//! ## 2. Attach from an agent
//!
//! A TUI app owns the terminal's stdio for rendering, so an agent
//! attaches from a *second* process over the socket the app bound
//! ([`Transport::Unix`]/[`Transport::Tcp`] — see "Transports" above).
//! Claude Code's own MCP client only speaks stdio to a command it
//! spawns, not a raw socket directly, so bridge the two with a one-line
//! `socat` command in its MCP config (`.mcp.json`, or `claude mcp add`):
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "quadraui-demo": {
//!       "command": "socat",
//!       "args": ["STDIO", "UNIX-CONNECT:/tmp/quadraui-mcp-demo-12345.sock"]
//!     }
//!   }
//! }
//! ```
//!
//! A headless or GUI app that uses [`Transport::Stdio`] instead needs no
//! bridge at all: point `command`/`args` straight at the app's own
//! binary, since it already shares this process's stdio with the MCP
//! client.
//!
//! ## 3. Read a snapshot
//!
//! `tools/call` with `{"name": "snapshot"}` returns the stable JSON form
//! [`quadraui::A11yTree`]'s own doc documents: a top-level `focus` field
//! naming the node currently holding accessibility focus, and a `root`
//! field holding the whole tree nested by `children`. Every node's `id`
//! is the same stable value across snapshots for the same widget, so an
//! id read from one snapshot stays valid to `act` on in a later call —
//! as long as the node is still in the tree; see "Virtualised rows"
//! below for the one case where it might not be yet.
//!
//! ## 4. `act` vs an app command
//!
//! `act` (Level 1) is the right tool whenever the goal has a direct UI
//! equivalent an accessibility role already exposes: click a list row,
//! type into a focused field, check a checkbox. It needs no cooperation
//! from the app beyond the `control` feature itself. An app-defined
//! command (Level 2 — every `tools/list` entry past the five built-ins)
//! is the right tool for anything `act` has no node for: a multi-step
//! model operation that should land atomically (`tui_mcp_demo`'s
//! `filter`, which recomputes a derived list *and* resets a selection in
//! one call), a query that reads state with no widget of its own
//! (`tui_commands_demo`'s `count`), or anything that would otherwise
//! need several `act` calls chained together by the agent itself with
//! no atomicity between them.
//!
//! ## Virtualised rows
//!
//! A long list/tree/table does not materialise an accessibility node
//! for every row — only a window of rows around the current scroll
//! offset does. `act`ing on a row's `id` works only once that row is
//! inside the window; a row far outside it has no node to act on yet,
//! even though the snapshot still reports its true `position_in_set`/
//! `set_size` in the full collection. Scroll the row into view first —
//! an `act` call with `scroll_up`/`scroll_down` (or `key` with
//! `PageUp`/`PageDown`), or an app-defined Level 2 command — before
//! trying to `act` on it directly. [`quadraui::A11yAction::ScrollIntoView`]
//! is a logged no-op today, not a working shortcut for this.

mod tools;
mod transport;

use serde_json::{json, Value};

use quadraui::control::ControlHandle;

pub use transport::Transport;

/// Runs an MCP server against `handle` over `transport`, blocking the
/// calling thread until the transport's listener (or, for
/// [`Transport::Stdio`], standard input) closes.
///
/// A caller that also owns an event loop on this thread (e.g. a TUI
/// app's own `run()`) calls this from a background thread instead,
/// exactly the way it would use [`ControlHandle`] itself from any other
/// thread that doesn't own the loop — see that type's own doc.
pub fn serve(handle: ControlHandle, transport: Transport) -> std::io::Result<()> {
    transport::serve(handle, transport)
}

/// The MCP protocol version this server declares in `initialize` when
/// the client's own request doesn't specify one to echo back.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Handles one line of input against `handle`, returning the line to
/// write back (already framed, caller adds the trailing newline) or
/// `None` for a notification (a request with no `id`), which JSON-RPC
/// never answers.
///
/// Shared by every transport in `transport.rs` — this is the one place
/// that understands JSON-RPC envelopes and the MCP methods layered over
/// them; nothing transport-specific leaks in here, and nothing
/// JSON-RPC-specific leaks into `transport.rs`.
fn handle_line(handle: &ControlHandle, line: &str) -> Option<String> {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ))
        }
    };
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    // A request with no `id` is a notification — JSON-RPC never replies
    // to one, success or failure, so side effects (none of the methods
    // below have any beyond answering) still run but nothing is ever
    // written back.
    let id = id?;

    match dispatch(handle, method, &params) {
        Ok(result) => Some(success_response(id, result)),
        Err((code, message)) => Some(error_response(id, code, &message)),
    }
}

/// Routes one JSON-RPC `method` to its MCP handling. `Err` is a
/// JSON-RPC-level protocol error (unknown method, malformed params) —
/// distinct from a tool's own failure, which `tools_call` reports inside
/// a normal `tools/call` *result* via `isError`, per the MCP spec, not
/// as a JSON-RPC error.
fn dispatch(handle: &ControlHandle, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(initialize_result(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::definitions(handle) })),
        "tools/call" => tools_call(handle, params),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

fn initialize_result(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "quadraui-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn tools_call(handle: &ControlHandle, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call: \"name\" is required".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // A tool failure (bad arguments, the control channel answering an
    // error) is reported *inside* a normal JSON-RPC success result via
    // `isError: true` — the MCP spec's own distinction between a
    // protocol-level error (this function's `Err`, e.g. an unknown
    // method) and a tool-level one, which an agent should see as the
    // tool's own output, not a transport failure.
    let (text, is_error) = match tools::call(handle, name, &arguments) {
        Ok(value) => (value.to_string(), false),
        Err(message) => (message, true),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    }))
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::control::channel;
    use quadraui::UserPayload;
    use std::sync::Arc;

    fn no_op_handle() -> (ControlHandle, quadraui::control::ControlPort) {
        let waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_| {});
        channel(waker)
    }

    #[test]
    fn notification_gets_no_reply() {
        let (handle, _port) = no_op_handle();
        let reply = handle_line(
            &handle,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert!(reply.is_none());
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let (handle, _port) = no_op_handle();
        let reply = handle_line(&handle, "not json").expect("parse error is still a reply");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"], json!(-32700));
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let (handle, _port) = no_op_handle();
        let reply = handle_line(&handle, r#"{"jsonrpc":"2.0","id":1,"method":"bogus"}"#)
            .expect("a request with an id always gets a reply");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], json!(1));
        assert_eq!(v["error"]["code"], json!(-32601));
    }

    #[test]
    fn initialize_echoes_protocol_version_and_names_the_server() {
        let (handle, _port) = no_op_handle();
        let reply = handle_line(
            &handle,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["result"]["protocolVersion"], json!("2024-11-05"));
        assert_eq!(v["result"]["serverInfo"]["name"], json!("quadraui-mcp"));
    }

    #[test]
    fn tools_list_names_all_five_tools() {
        // Dropping the port rather than leaving it unserviced:
        // `tools::definitions`'s `ControlHandle::list_commands` call
        // (querying Level 2 app commands) answers `RunnerGone`
        // immediately this way, same as every other no-app-attached
        // request below — an unserviced-but-still-held port would
        // instead block for `ControlHandle`'s own multi-second timeout
        // before `definitions` falls back to "no Level 2 commands".
        let (handle, port) = no_op_handle();
        drop(port);
        let reply =
            handle_line(&handle, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        let v: Value = serde_json::from_str(&reply).unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["snapshot", "act", "key", "type", "screenshot"]);
    }

    /// A tool-level failure (here: the runner already gone, so
    /// `snapshot` can't even reach it) is a normal JSON-RPC success
    /// whose result has `isError: true` — never a JSON-RPC `error`
    /// object. See `tools_call`'s own doc for why.
    #[test]
    fn tool_failure_is_iserror_not_a_protocol_error() {
        let (handle, port) = no_op_handle();
        drop(port); // simulate the owning runner having exited
        let reply = handle_line(
            &handle,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"snapshot","arguments":{}}}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert!(v.get("error").is_none());
        assert_eq!(v["result"]["isError"], json!(true));
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("control runner has exited"));
    }
}
