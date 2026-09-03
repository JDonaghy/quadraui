//! Regression coverage for #619: library diagnostics must never reach the
//! host's screen.
//!
//! `UnpaintedModalApp` deliberately reproduces the #455 defect class this
//! bug report is about — it registers a modal in the [`quadraui::Backend`]'s
//! `ModalStack` (making it hit-testable) but never paints it, which is
//! exactly the shape of vimcode's `editor_hover` popup: pushed onto the
//! stack, painted through a path (`draw_rich_text_popup` there) that
//! doesn't call `mark_painted`. That drives `TuiBackend::end_frame`'s #455
//! detector every frame — before #619 that meant an `eprintln!` landing
//! directly on the host's screen, on every single frame the modal stayed
//! open.
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
        // This is the exact "registered but invisible" shape #455 detects.
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

/// The headline regression: with no sink installed (the default — and the
/// state of every host that never opts in), the #455 diagnostic must never
/// reach the rendered screen, in debug builds (where the detector is
/// compiled in) same as release.
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

/// The detector must still be doing its job — #619 is "route it to a
/// sink," not "delete it." A host that opts in by installing a sink must
/// still see the #455 warning; the fix only changes *where* it goes, not
/// *whether* it exists.
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
