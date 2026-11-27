#![cfg(unix)]
//! Black-box acceptance test for `quadraui-mcp`: a real MCP client,
//! connected over a Unix-domain socket, drives a headless TUI app
//! through `initialize` → `tools/list` → `snapshot` → `act` (select a
//! list row) → `snapshot`, and asserts the selection changed in the
//! app's own model as well as in the second snapshot's JSON.
//!
//! This test plays three roles on three different threads, mirroring how
//! a real deployment splits them across processes:
//! - the "runner" thread repeatedly calls
//!   [`quadraui::tui::testing::TuiDriver::service_control`], the
//!   headless stand-in for a live `TuiRunner`'s own per-iteration call;
//! - the "server" thread runs [`quadraui_mcp::serve`] against a
//!   [`quadraui_mcp::Transport::Unix`] socket;
//! - this test's own thread is the "MCP client": a from-scratch
//!   line-delimited JSON-RPC connection over that socket, with no MCP
//!   SDK of its own, proving the wire format rather than trusting a
//!   client library's interpretation of it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use quadraui::control::channel;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    set_frame_capture, Backend, Decoration, ListItem, ListView, ListViewEvent, Reaction, Rect,
    ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};
use quadraui_mcp::Transport;
use serde_json::{json, Value};

/// A minimal `AppLogic` whose entire model is a selected row index, so
/// the test asserts on real application state, not just that some
/// message round-tripped.
struct ListApp {
    items: Vec<&'static str>,
    selected: usize,
}

impl quadraui::AppLogic for ListApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut layout = ScreenLayout::new();
        layout.push(Surface::List {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            list: ListView {
                id: WidgetId::new("rows"),
                title: None,
                items: self
                    .items
                    .iter()
                    .map(|text| ListItem {
                        text: StyledText::plain(*text),
                        icon: None,
                        detail: None,
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: self.selected,
                scroll_offset: 0,
                has_focus: true,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
                striped: false,
            },
        });
        layout.draw(backend);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::List(_, ListViewEvent::ItemActivated { idx }) = event {
            self.selected = idx;
            return Reaction::Redraw;
        }
        Reaction::Continue
    }
}

/// One JSON-RPC round trip over `stream`: writes `request` as a line,
/// reads one line back, parses it as JSON. Panics (failing the test)
/// well before the overall process timeout if the server never answers.
fn call(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>, request: &Value) -> Value {
    let mut line = serde_json::to_string(request).expect("request serializes");
    line.push('\n');
    stream.write_all(line.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let mut response_line = String::new();
    reader.read_line(&mut response_line).expect("read response");
    assert!(
        !response_line.is_empty(),
        "server closed the connection before answering {request}"
    );
    serde_json::from_str(&response_line)
        .unwrap_or_else(|e| panic!("response {response_line:?} is not valid JSON: {e}"))
}

/// Finds the node with id `target` anywhere under `node` (a snapshot's
/// `"root"` value, or one of its `"children"`), walking the same JSON
/// shape `A11yTree`'s `Serialize` impl produces.
fn find_node(node: &Value, target: u64) -> Option<Value> {
    if node.get("id").and_then(Value::as_u64) == Some(target) {
        return Some(node.clone());
    }
    node.get("children")?
        .as_array()?
        .iter()
        .find_map(|child| find_node(child, target))
}

#[test]
fn mcp_client_over_unix_socket_drives_a_headless_tui_app() {
    let app = ListApp {
        items: vec!["alpha", "beta", "gamma"],
        selected: 0,
    };
    let mut driver = TuiDriver::new(app, 40, 10);
    {
        let (backend, _app) = driver.parts_mut();
        assert!(
            set_frame_capture(backend, true),
            "the TUI backend offers a frame-capture slot"
        );
    }
    driver.render();

    let waker = driver.backend().waker();
    let (handle, port) = channel(waker);

    let socket_path = std::env::temp_dir().join(format!(
        "quadraui-mcp-roundtrip-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);

    let server_socket_path = socket_path.clone();
    let server =
        thread::spawn(move || quadraui_mcp::serve(handle, Transport::Unix(server_socket_path)));

    // `TuiDriver` is not `Send` (it owns an `Rc`-based ratatui terminal),
    // so it has to stay on this test's own thread — the "MCP client"
    // role moves to a background thread instead, and this thread plays
    // the runner's role: service the control port in a loop, the same
    // way `tui::run::TuiRunner::run_one` would once per iteration, until
    // the client thread reports it's done.
    let client_done = Arc::new(AtomicBool::new(false));
    let worker_done = client_done.clone();
    let socket_path_for_client = socket_path.clone();
    let client = thread::spawn(move || -> (Value, Value) {
        // Dial the server's socket, retrying past both halves of its
        // startup: the file not existing yet, and `bind()` creating the
        // path a syscall before `listen()` puts it in a state that
        // accepts connections — a connect in that gap fails with
        // `ConnectionRefused` even though the path is already present.
        // Retrying the connect itself (not just polling for the path)
        // covers both.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match UnixStream::connect(&socket_path_for_client) {
                Ok(stream) => break stream,
                Err(e) => {
                    assert!(
                        Instant::now() < deadline,
                        "never connected to the MCP socket: {e}"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
            }
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone the connected stream"));

        let init = call(
            &mut stream,
            &mut reader,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "2024-11-05"},
            }),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], json!("quadraui-mcp"));

        let listed = call(
            &mut stream,
            &mut reader,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        );
        let tool_names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            tool_names,
            vec!["snapshot", "act", "key", "type", "screenshot"]
        );

        let before = call(
            &mut stream,
            &mut reader,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "snapshot", "arguments": {}},
            }),
        );
        assert_eq!(before["result"]["isError"], json!(false));
        let before_tree: Value =
            serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap())
                .expect("snapshot content is a JSON tree");

        let row_1_id = quadraui::A11yNodeId::for_widget(&WidgetId::new("rows"))
            .child("item.1")
            .as_u64();
        let row_1_before = find_node(&before_tree["root"], row_1_id).expect("row 1 present before");
        assert_eq!(row_1_before["selected"], json!(false));

        let acted = call(
            &mut stream,
            &mut reader,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "act",
                    "arguments": {"node": row_1_id, "action": "click"},
                },
            }),
        );
        assert_eq!(
            acted["result"]["isError"],
            json!(false),
            "act call failed: {acted:?}"
        );

        // The control channel dispatches synchronously, but the
        // accessibility tree snapshot comes from the most recently
        // *painted* frame — poll `snapshot` until the runner thread's
        // next repaint picks the change up.
        let deadline = Instant::now() + Duration::from_secs(5);
        let after_tree = loop {
            let after = call(
                &mut stream,
                &mut reader,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": {"name": "snapshot", "arguments": {}},
                }),
            );
            let tree: Value =
                serde_json::from_str(after["result"]["content"][0]["text"].as_str().unwrap())
                    .expect("snapshot content is a JSON tree");
            let row = find_node(&tree["root"], row_1_id).expect("row 1 present after");
            if row["selected"] == json!(true) {
                break tree;
            }
            assert!(Instant::now() < deadline, "selection never became visible");
            thread::sleep(Duration::from_millis(1));
        };

        worker_done.store(true, Ordering::SeqCst);
        (before_tree, after_tree)
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while !client_done.load(Ordering::SeqCst) {
        driver.service_control(&port);
        assert!(Instant::now() < deadline, "MCP client never finished");
        thread::sleep(Duration::from_millis(1));
    }
    let (before_tree, after_tree) = client.join().expect("client thread panicked");

    assert_eq!(
        driver.app().selected,
        1,
        "the act tool must have driven the real AppLogic model, not just the snapshot JSON"
    );
    let row_1_id = quadraui::A11yNodeId::for_widget(&WidgetId::new("rows"))
        .child("item.1")
        .as_u64();
    assert_eq!(
        find_node(&before_tree["root"], row_1_id).unwrap()["selected"],
        json!(false)
    );
    assert_eq!(
        find_node(&after_tree["root"], row_1_id).unwrap()["selected"],
        json!(true)
    );

    let _ = std::fs::remove_file(&socket_path);
    // `server` is an infinite accept loop with no shutdown signal (see
    // `transport::serve_unix`) — its thread leaks for the rest of the
    // test process, which exits with this test binary; nothing to join.
    drop(server);
}
