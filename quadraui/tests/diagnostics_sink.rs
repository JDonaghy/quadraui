//! Library diagnostics are routed to a sink instead of directly to stderr.
//!
//! `UnpaintedModalApp` reproduces the unpainted modal defect class:
//! it registers a modal in the [`quadraui::Backend`]'s `ModalStack`
//! (making it hit-testable) but never paints it. The detector in
//! `TuiBackend::end_frame` fires every frame the modal stays open.
#![cfg(feature = "tui")]

use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, Reaction, Rect, UiEvent, WidgetId};
use std::sync::{Arc, Mutex, OnceLock};

/// [`quadraui::diagnostics`]'s sink is process-global, and `cargo test`
/// runs the `#[test]` fns below on separate threads by default. Without
/// serialising, one test's `set_sink` can race another's `clear_sink` /
/// assertion window. Both tests in this file acquire this before touching
/// the sink.
fn sink_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct UnpaintedModalApp;

impl AppLogic for UnpaintedModalApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        // Register a modal so it's hit-testable — but never call any
        // `draw_*` for it, so `ModalStack::mark_painted` is never reached.
        backend.modal_stack_handle().borrow_mut().push(
            WidgetId::new("unpainted_modal"),
            Rect {
                x: 2.0,
                y: 2.0,
                width: 10.0,
                height: 3.0,
            },
        );
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// With no sink installed, the unpainted modal diagnostic must not
/// reach the rendered screen in either debug or release builds.
#[test]
fn unpainted_modal_diagnostic_never_reaches_the_screen() {
    let _guard = sink_test_guard();
    quadraui::diagnostics::clear_sink();

    let driver = TuiDriver::new(UnpaintedModalApp, 40, 10);

    let screen = driver.screen();
    assert!(
        !driver.screen_contains("quadraui:"),
        "a library diagnostic must never be painted onto the host's screen \
         (quadraui#619):\n{screen}"
    );
}

/// The detector still fires when a sink is installed, routing the
/// unpainted modal warning to the sink instead of stderr.
#[test]
fn unpainted_modal_diagnostic_still_reaches_an_installed_sink() {
    let _guard = sink_test_guard();
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_in_sink = Arc::clone(&received);
    quadraui::diagnostics::set_sink(move |msg: &str| {
        received_in_sink.lock().unwrap().push(msg.to_string());
    });

    let driver = TuiDriver::new(UnpaintedModalApp, 40, 10);

    let messages = received.lock().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unpainted_modal") && m.contains("quadraui#455")),
        "the #455 diagnostic should still be generated and delivered to an \
         installed sink, just never printed directly: {messages:?}\nscreen:\n{}",
        driver.screen()
    );
    drop(messages);

    quadraui::diagnostics::clear_sink();
}
