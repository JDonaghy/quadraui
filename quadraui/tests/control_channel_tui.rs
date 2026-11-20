#![cfg(all(feature = "tui", feature = "control"))]
//! Black-box acceptance test for `quadraui::control`'s TUI wiring.
//!
//! Drives a small `AppLogic` list app through [`TuiDriver`] while a
//! background thread uses a [`quadraui::control::ControlHandle`] to
//! snapshot the app's accessibility tree, act a click on a list row, and
//! snapshot again — proving the control channel can both read and drive
//! a running app from a thread that does not own its event loop, through
//! the exact same `render_frame`/`dispatch_event` production path every
//! other `TuiDriver` test exercises.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use quadraui::control::{channel, ControlError};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    set_frame_capture, A11yAction, A11yNodeId, Backend, Decoration, ListItem, ListView,
    ListViewEvent, Reaction, Rect, ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};

/// A minimal `AppLogic` whose entire model is a selected row index, so a
/// test can assert on real application state — not just that some event
/// arrived.
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

/// Find the node with id `target` anywhere under `node` (a snapshot's
/// `"root"` value, or one of its `"children"`), by walking the same JSON
/// shape `A11yTree`'s `Serialize` impl produces.
fn find_node(node: &serde_json::Value, target: u64) -> Option<serde_json::Value> {
    if node.get("id").and_then(serde_json::Value::as_u64) == Some(target) {
        return Some(node.clone());
    }
    node.get("children")?
        .as_array()?
        .iter()
        .find_map(|child| find_node(child, target))
}

/// End-to-end round trip: a background thread snapshots, acts a click on
/// list row 1, then polls snapshot until the change is visible (the
/// control channel dispatches synchronously but a fresh accessibility
/// tree still needs a repaint, serviced by this test's own thread playing
/// the runner's role — exactly what `tui::run::TuiRunner::attach_control`
/// wires into the live loop). Asserts the selection changed both in the
/// app's own model and in the second snapshot's JSON.
#[test]
fn background_thread_drives_the_app_through_the_control_channel() {
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

    let row_1 = A11yNodeId::for_widget(&WidgetId::new("rows")).child("item.1");
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = done.clone();
    let worker = thread::spawn(move || -> (serde_json::Value, serde_json::Value) {
        let before = handle.snapshot().expect("first snapshot");
        handle
            .act(row_1, A11yAction::Click, None)
            .expect("act click on row 1");

        // The act's events already dispatched synchronously (see
        // `quadraui::control`'s module doc), but the accessibility tree
        // is built from the most recently *painted* frame — poll until
        // the runner thread's next repaint picks the change up, bounded
        // so a real regression fails fast instead of hanging.
        let deadline = Instant::now() + Duration::from_secs(5);
        let after = loop {
            let snap = handle.snapshot().expect("snapshot while polling");
            let row = find_node(&snap["root"], row_1.as_u64()).expect("row 1 present");
            if row["selected"] == serde_json::json!(true) {
                break snap;
            }
            assert!(Instant::now() < deadline, "selection never became visible");
            thread::sleep(Duration::from_millis(1));
        };
        worker_done.store(true, Ordering::SeqCst);
        (before, after)
    });

    // Play the runner's role: service the control port and repaint on
    // `Reaction::Redraw`, in a loop, the same way
    // `tui::run::TuiRunner::run_one` does once per iteration — until the
    // worker reports it's done or a generous deadline passes.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
        let (backend, app) = driver.parts_mut();
        let reaction = port.service(app, backend);
        if reaction == Reaction::Redraw {
            driver.render();
        }
        thread::sleep(Duration::from_millis(1));
    }
    let (before, after) = worker.join().expect("worker thread panicked");

    assert_eq!(
        driver.app().selected,
        1,
        "clicking row 1 through the control channel must update the app's own model"
    );

    let row_1_before = find_node(&before["root"], row_1.as_u64()).expect("row 1 present before");
    assert_eq!(
        row_1_before["selected"],
        serde_json::json!(false),
        "row 1 must start out unselected"
    );
    let row_1_after = find_node(&after["root"], row_1.as_u64()).expect("row 1 present after");
    assert_eq!(
        row_1_after["selected"],
        serde_json::json!(true),
        "row 1 must be selected in the second snapshot"
    );
}

/// A request against a runner that has already exited (its
/// [`quadraui::control::ControlPort`] dropped, same as a `TuiRunner` that
/// tore itself down) comes back as a clean [`ControlError::RunnerGone`],
/// never a hang or a panic.
#[test]
fn request_times_out_cleanly_once_the_runner_is_gone() {
    let app = ListApp {
        items: vec!["only"],
        selected: 0,
    };
    let driver = TuiDriver::new(app, 20, 5);
    let waker = driver.backend().waker();
    let (handle, port) = channel(waker);
    drop(port);
    drop(driver);

    let err = handle.snapshot().expect_err("the runner is gone");
    assert!(matches!(err, ControlError::RunnerGone));
}

/// Acting on a node id that does not exist in the tree is a safe, logged
/// no-op (see `quadraui::a11y::route_action`'s contract) — the control
/// channel must not panic, and still answers success.
#[test]
fn act_on_unknown_node_id_does_not_panic() {
    let app = ListApp {
        items: vec!["only"],
        selected: 0,
    };
    let mut driver = TuiDriver::new(app, 20, 5);
    {
        let (backend, _app) = driver.parts_mut();
        set_frame_capture(backend, true);
    }
    driver.render();

    let waker = driver.backend().waker();
    let (handle, port) = channel(waker);
    let bogus = A11yNodeId::for_key("does-not-exist-anywhere");

    let worker = thread::spawn(move || handle.act(bogus, A11yAction::Click, None));
    loop {
        let (backend, app) = driver.parts_mut();
        port.service(app, backend);
        if worker.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    worker
        .join()
        .expect("worker thread panicked")
        .expect("acting on an unknown node id is a safe no-op, not an error");
}
