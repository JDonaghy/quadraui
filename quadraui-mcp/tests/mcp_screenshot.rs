#![cfg(unix)]
//! Black-box acceptance test for `quadraui-mcp`'s `screenshot` tool: a
//! real MCP client, connected over a Unix-domain socket, drives a
//! headless TUI app through `tools/list` (confirms `screenshot` is
//! advertised), `screenshot` (before), `act` (select a list row), then
//! `screenshot` again — and asserts the row the `act` changed is visible
//! in the *second* screenshot's text grid but not the first.
//!
//! Same three-thread shape as `tests/mcp_tui_roundtrip.rs` (runner/
//! server/client) — see that file's module doc for why each role needs
//! its own thread.

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

/// Same minimal list app as `tests/mcp_tui_roundtrip.rs`: the selected
/// row paints a `▶ ` prefix no other row has
/// (`quadraui::tui::list::draw_list`'s own tests document this), so a
/// screenshot's text grid visibly differs before/after a selection
/// change, not just its style.
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

/// One JSON-RPC round trip over `stream` — identical to the helper in
/// `tests/mcp_tui_roundtrip.rs`.
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

/// Calls the `screenshot` tool and returns its `rows` array as owned
/// `String`s — panics (failing the test) if the tool answered an error,
/// since every assertion in this test expects it to succeed.
fn screenshot_rows(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    id: i64,
) -> Vec<String> {
    let response = call(
        stream,
        reader,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "screenshot", "arguments": {}},
        }),
    );
    assert_eq!(
        response["result"]["isError"],
        json!(false),
        "screenshot call failed: {response:?}"
    );
    let shot: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
            .expect("screenshot content is a JSON Screenshot");
    assert_eq!(shot["kind"], json!("text"), "TUI answers Screenshot::Text");
    shot["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn mcp_client_screenshot_tool_shows_the_row_an_act_changed() {
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
        "quadraui-mcp-screenshot-{}-{}.sock",
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

    // `TuiDriver` is not `Send`, so it stays on this test's own thread,
    // playing the "runner" role; the MCP client moves to a background
    // thread — same split as `tests/mcp_tui_roundtrip.rs`.
    let client_done = Arc::new(AtomicBool::new(false));
    let worker_done = client_done.clone();
    let socket_path_for_client = socket_path.clone();
    let client = thread::spawn(move || -> (Vec<String>, Vec<String>) {
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

        call(
            &mut stream,
            &mut reader,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "2024-11-05"},
            }),
        );

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

        let before_rows = screenshot_rows(&mut stream, &mut reader, 3);
        assert!(
            before_rows.iter().any(|r| r.trim_end() == "  beta"),
            "row 1 (\"beta\") must not be marked selected before the act: {before_rows:?}"
        );
        assert!(
            !before_rows.iter().any(|r| r.contains("▶ beta")),
            "no row should show the selection marker before the act: {before_rows:?}"
        );

        let row_1_id = quadraui::A11yNodeId::for_widget(&WidgetId::new("rows"))
            .child("item.1")
            .as_u64();
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

        // The control channel dispatches synchronously, but a screenshot
        // reflects the most recently *painted* frame — poll until the
        // runner thread's next repaint picks the selection change up.
        let deadline = Instant::now() + Duration::from_secs(5);
        let after_rows = loop {
            let rows = screenshot_rows(&mut stream, &mut reader, 5);
            if rows.iter().any(|r| r.contains("▶ beta")) {
                break rows;
            }
            assert!(Instant::now() < deadline, "selection never became visible");
            thread::sleep(Duration::from_millis(1));
        };

        worker_done.store(true, Ordering::SeqCst);
        (before_rows, after_rows)
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while !client_done.load(Ordering::SeqCst) {
        driver.service_control(&port);
        assert!(Instant::now() < deadline, "MCP client never finished");
        thread::sleep(Duration::from_millis(1));
    }
    let (before_rows, after_rows) = client.join().expect("client thread panicked");

    assert_eq!(
        driver.app().selected,
        1,
        "the act tool must have driven the real AppLogic model, not just the screenshot text"
    );
    assert!(!before_rows.iter().any(|r| r.contains("▶ beta")));
    assert!(after_rows.iter().any(|r| r.contains("▶ beta")));

    let _ = std::fs::remove_file(&socket_path);
    // `server` is an infinite accept loop with no shutdown signal — its
    // thread leaks for the rest of the test process, which exits with
    // this test binary; nothing to join (same as `mcp_tui_roundtrip.rs`).
    drop(server);
}
