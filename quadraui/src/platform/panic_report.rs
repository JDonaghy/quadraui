// ─────────────────────────────────────────────────────────────────────
// Caught-panic reporting dedup
// ─────────────────────────────────────────────────────────────────────
//
// `macos::run`'s `drawRect:`/responder-method bodies and `win::run`'s
// `wndproc` both hand a Rust callback directly to a C ABI that cannot
// unwind — an uncaught panic there escalates straight to `abort()`
// instead of staying a recoverable panic. Both bracket every such body —
// or, for `win::run`, the shared `dispatch_event`/`render_frame` helpers
// every message ultimately funnels through — in
// `std::panic::catch_unwind(AssertUnwindSafe(..))` and report a caught
// panic through [`report_caught_panic_once`] rather than calling
// `crate::diagnostics::emit` directly, so a rasteriser gap re-entered on
// every redraw is reported once per process, not once per frame.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
pub(crate) fn report_caught_panic_once(site: &str, payload: &(dyn std::any::Any + Send)) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Dedup key is `site` alone, not `site` + message: the goal is "don't
    // spam the same reachable panic on every repaint," not "distinguish
    // every possible payload string" — a `todo!()` at a fixed call site
    // always carries the same message anyway.
    if !seen.insert(site.to_string()) {
        return;
    }
    drop(seen);

    let message = panic_payload_message(payload);
    crate::diagnostics::emit(format!(
        "quadraui: panic caught at {site} — frame/event skipped, process kept alive ({message})"
    ));
}

/// Best-effort text for a `catch_unwind` payload. `Any` gives us nothing
/// beyond a downcast; the two shapes `panic!`/`todo!`/`unwrap()` actually
/// produce are a `&'static str` literal or a `String` from a `format!`ed
/// message, so those are the only two cases worth naming.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(all(
    test,
    any(feature = "win", all(feature = "macos", target_os = "macos"))
))]
mod panic_report_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Distinct `site` per test — the dedup registry is a process-global
    /// static shared by every test in this binary, so a colliding label
    /// between tests (or a re-run of the same test) would make the
    /// second call a silent no-op and the assertion below flaky.
    fn unique_site(tag: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("desktop::tests::{tag}#{n}")
    }

    /// Installs a recording sink and returns it. Callers must hold
    /// [`crate::diagnostics::test_guard`] for the duration — the sink
    /// slot is a process-global shared with `diagnostics.rs`'s own tests.
    fn install_recording_sink() -> Arc<Mutex<Vec<String>>> {
        let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let received_in_sink = Arc::clone(&received);
        crate::diagnostics::set_sink(move |msg: &str| {
            received_in_sink.lock().unwrap().push(msg.to_string());
        });
        received
    }

    #[test]
    fn first_report_at_a_site_reaches_the_sink() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("first_report");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 1);
        assert!(received.lock().unwrap()[0].contains(&site));
        assert!(received.lock().unwrap()[0].contains("boom"));
    }

    #[test]
    fn repeated_reports_at_the_same_site_are_reported_once() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("repeated_reports");
        let received = install_recording_sink();
        for _ in 0..5 {
            let payload: Box<dyn std::any::Any + Send> = Box::new("same panic every frame");
            report_caught_panic_once(&site, payload.as_ref());
        }
        crate::diagnostics::clear_sink();
        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "five panics at the same site must produce exactly one report, \
             not one per frame"
        );
    }

    #[test]
    fn distinct_sites_each_report_independently() {
        let _g = crate::diagnostics::test_guard();
        let site_a = unique_site("distinct_a");
        let site_b = unique_site("distinct_b");
        let received = install_recording_sink();
        let payload_a: Box<dyn std::any::Any + Send> = Box::new("a");
        let payload_b: Box<dyn std::any::Any + Send> = Box::new("b");
        report_caught_panic_once(&site_a, payload_a.as_ref());
        report_caught_panic_once(&site_b, payload_b.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn string_payload_message_is_included() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("string_payload");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new(format!("formatted {}", 42));
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert!(received.lock().unwrap()[0].contains("formatted 42"));
    }

    #[test]
    fn non_string_payload_does_not_panic_the_reporter() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("non_string_payload");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new(42i32);
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 1);
    }
}
