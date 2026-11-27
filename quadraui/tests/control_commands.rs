#![cfg(all(feature = "tui", feature = "control"))]
//! Black-box acceptance test for `quadraui::control`'s "Level 2" named
//! commands (see that module's "Levels" doc section).
//!
//! `SelectCountApp` registers `select(index)` and `count()` against a
//! [`quadraui::control::CommandRegistry`] stashed on itself, wired into
//! [`quadraui::AppLogic::command_specs`]/[`quadraui::AppLogic::run_command`].
//! A background thread drives it two ways:
//! - [`quadraui::control::ControlHandle::command`]/`list_commands` over
//!   the real control channel — the exact calls `quadraui-mcp`'s
//!   `tools/call`/`tools/list` make against a running app; and
//! - [`quadraui::control::cli::run`] directly against a second, freshly
//!   constructed instance of the same app — no control channel, no event
//!   loop, no backend at all.
//!
//! Both paths answer the same JSON for the same command/args, and the
//! control-channel path's `select` is visible in the next accessibility
//! snapshot, exactly like a UI-level [`quadraui::control::ControlRequest::Act`]
//! would be — proving Level 2 shares the same "answers come from the
//! most recently painted frame" contract Level 1 already has.
//!
//! This test's own thread plays the runner's role by calling
//! [`TuiDriver::service_control`] in a loop, the headless equivalent of
//! what `tui::run::TuiRunner::run_one` does once per `step`/`pump`
//! iteration for a channel attached via `attach_control` — the same
//! approximation `tests/control_channel_tui.rs` documents for Level 1.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use quadraui::control::{channel, CommandError, CommandRegistry, CommandSpec};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    set_frame_capture, A11yNodeId, Backend, Decoration, ListItem, ListView, ListViewEvent,
    Reaction, Rect, ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};
use serde_json::{json, Value};

/// An `AppLogic` whose model is a selected row index, with `select` and
/// `count` registered against its own [`CommandRegistry`] — so a test
/// can assert on real application state, not just that some reply
/// round-tripped.
struct SelectCountApp {
    items: Vec<&'static str>,
    selected: usize,
    commands: CommandRegistry<SelectCountApp>,
}

impl SelectCountApp {
    fn new() -> Self {
        let mut commands = CommandRegistry::new();
        commands.register(
            "select",
            "Selects an item by index.",
            json!({
                "type": "object",
                "properties": {"index": {"type": "integer"}},
                "required": ["index"],
            }),
            |app: &mut SelectCountApp, args: Value| {
                let index = args
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| CommandError::InvalidArgs("\"index\" is required".into()))?
                    as usize;
                if index >= app.items.len() {
                    return Err(CommandError::Failed(format!("index {index} out of range")));
                }
                app.selected = index;
                Ok(json!({"selected": index}))
            },
        );
        commands.register(
            "count",
            "Returns the number of items.",
            json!({"type": "object", "properties": {}}),
            |app: &mut SelectCountApp, _args: Value| Ok(json!({"count": app.items.len()})),
        );
        Self {
            items: vec!["alpha", "beta", "gamma"],
            selected: 0,
            commands,
        }
    }
}

impl quadraui::AppLogic for SelectCountApp {
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

    fn command_specs(&self) -> Vec<CommandSpec> {
        self.commands.specs()
    }

    fn run_command(&mut self, name: &str, args: Value) -> Result<Value, CommandError> {
        // `CommandRegistry::run`'s documented dance: swap the registry
        // out so its closures can take `&mut self` without aliasing the
        // field that holds them, then swap it back.
        let mut commands = std::mem::take(&mut self.commands);
        let result = commands.run(self, name, args);
        self.commands = commands;
        result
    }
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

/// The control channel lists `select`/`count` (`tools/list`'s own
/// source) and runs `select` against the live app, with the same JSON
/// result [`quadraui::control::cli::run`] gets against a fresh instance
/// of the same app — and the control-channel `select`'s effect is
/// visible in the next accessibility snapshot.
#[test]
fn command_requests_list_and_run_against_the_live_app_matching_the_cli_helper() {
    let mut driver = TuiDriver::new(SelectCountApp::new(), 40, 10);
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

    let row_1 = A11yNodeId::for_widget(&WidgetId::new("rows")).child("item.1");
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = done.clone();
    let worker = thread::spawn(move || -> (Vec<String>, Value, Value, Value) {
        let specs = handle.list_commands().expect("list_commands");
        let names: Vec<String> = specs.into_iter().map(|s| s.name).collect();

        let before = handle.snapshot().expect("snapshot before select");

        let control_value = handle
            .command("select", json!({"index": 1}))
            .expect("select over the control channel");

        // The command dispatches synchronously, but the accessibility
        // tree is built from the most recently *painted* frame — poll
        // until this test's own thread (playing the runner's role
        // below) repaints.
        let deadline = Instant::now() + Duration::from_secs(5);
        let after = loop {
            let snap = handle.snapshot().expect("snapshot while polling");
            let row = find_node(&snap["root"], row_1.as_u64()).expect("row 1 present");
            if row["selected"] == json!(true) {
                break snap;
            }
            assert!(Instant::now() < deadline, "selection never became visible");
            thread::sleep(Duration::from_millis(1));
        };

        worker_done.store(true, Ordering::SeqCst);
        (names, before, control_value, after)
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !done.load(Ordering::SeqCst) {
        driver.service_control(&port);
        assert!(Instant::now() < deadline, "worker thread never finished");
        thread::sleep(Duration::from_millis(1));
    }
    let (names, before, control_value, after) = worker.join().expect("worker thread panicked");

    assert_eq!(names, vec!["select", "count"]);

    let row_1_before = find_node(&before["root"], row_1.as_u64()).expect("row 1 present before");
    assert_eq!(row_1_before["selected"], json!(false));
    let row_1_after = find_node(&after["root"], row_1.as_u64()).expect("row 1 present after");
    assert_eq!(
        row_1_after["selected"],
        json!(true),
        "select's effect must be visible in the next accessibility snapshot"
    );
    assert_eq!(
        driver.app().selected,
        1,
        "the control-channel command must mutate the real app's model"
    );

    // The CLI helper runs the exact same command/args against a fresh
    // instance of the same app, with no control channel, event loop, or
    // backend at all — and gets the same JSON the control channel did.
    let mut cli_app = SelectCountApp::new();
    let cli_value = quadraui::control::cli::run_result(
        &mut cli_app,
        &["select".to_string(), r#"{"index": 1}"#.to_string()],
    )
    .expect("cli helper runs select");
    assert_eq!(cli_app.selected, 1);
    assert_eq!(
        control_value, cli_value,
        "the control channel and the CLI helper must answer identical JSON \
         for the same command and args"
    );
}

/// An unregistered command name over the control channel answers
/// [`quadraui::control::ControlError::Failed`] (wrapping
/// `CommandError::NotFound`'s message), not a panic or a hang — the same
/// contract every other `ControlRequest` variant documents.
#[test]
fn unknown_command_over_the_control_channel_is_a_clean_error() {
    let mut driver = TuiDriver::new(SelectCountApp::new(), 20, 5);
    let waker = driver.backend().waker();
    let (handle, port) = channel(waker);

    let worker = thread::spawn(move || handle.command("bogus", json!({})));
    loop {
        driver.service_control(&port);
        if worker.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let err = worker
        .join()
        .expect("worker thread panicked")
        .expect_err("bogus is not a registered command");
    assert!(err.to_string().contains("bogus"));
}
