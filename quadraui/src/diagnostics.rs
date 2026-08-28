//! Library-internal diagnostics sink (#619).
//!
//! quadraui must never write to stdout/stderr on its own: a host embedding
//! one of quadraui's backends (vimcode's TUI, in raw mode on the alternate
//! screen, is the motivating case) owns the terminal, and a stray
//! `eprintln!` from library code lands as raw bytes in the host's live
//! cell grid — bypassing ratatui entirely, so the framework never learns
//! those cells were overwritten and the text persists until something
//! forces a full redraw. See #619 for the incident this module exists to
//! prevent (the #455 unpainted-modal warning smearing over vimcode's
//! editor on every frame the popup was open).
//!
//! Instead, library code that wants to surface a diagnostic calls a
//! crate-internal `emit` function, which forwards the message to whatever
//! sink the host installed via [`set_sink`]. With no sink installed — the
//! default, and the state of every host that never calls `set_sink` —
//! diagnostics are silently dropped. This holds in debug and release
//! builds alike; unlike the old call sites, nothing here is gated on
//! `debug_assertions`.
//!
//! Hosts decide what "surfacing" means: a status line, a log file, a
//! `tracing` event, a debug panel. This module makes no assumption about
//! any of that — the sink is just `Fn(&str)`.
//!
//! # Example
//!
//! ```
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use std::sync::Arc;
//!
//! let seen = Arc::new(AtomicUsize::new(0));
//! let seen_in_sink = Arc::clone(&seen);
//! quadraui::diagnostics::set_sink(move |_msg: &str| {
//!     seen_in_sink.fetch_add(1, Ordering::SeqCst);
//! });
//!
//! // A host only ever *installs* a sink; `emit` itself is
//! // crate-internal, called from the small set of library call sites
//! // listed in #619.
//!
//! quadraui::diagnostics::clear_sink();
//! ```

use std::sync::{OnceLock, RwLock};

/// A host-installed diagnostic sink. See the module doc.
type Sink = Box<dyn Fn(&str) + Send + Sync + 'static>;

fn sink_slot() -> &'static RwLock<Option<Sink>> {
    static SLOT: OnceLock<RwLock<Option<Sink>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Install the sink that receives every future `emit` call, replacing
/// any previously installed sink.
///
/// The host owns delivery — log to a file, forward to `tracing`, append
/// to an in-app diagnostics panel, whatever fits. quadraui never touches
/// stdout/stderr on its own; a host that never calls this gets a
/// guaranteed-silent library.
///
/// # Example
///
/// ```
/// quadraui::diagnostics::set_sink(|msg: &str| {
///     eprintln!("[quadraui via host sink] {msg}");
/// });
/// # quadraui::diagnostics::clear_sink();
/// ```
pub fn set_sink<F>(sink: F)
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let mut slot = sink_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(Box::new(sink));
}

/// Remove any installed sink, restoring the default silent behaviour.
/// Mainly useful for tests that install a sink temporarily — the sink is
/// process-global, so a test that installs one and doesn't clear it can
/// leak into an unrelated test running later in the same binary.
pub fn clear_sink() {
    let mut slot = sink_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = None;
}

/// Emit a library diagnostic. Never writes anywhere itself — forwards to
/// the host's sink if one is installed via [`set_sink`], otherwise drops
/// the message on the floor.
///
/// Library-internal call sites only (not `pub`): the small, named set of
/// places listed in #619 — modal-paint tracking, vt100 panic recovery,
/// the GTK/macOS siblings of the same modal-paint check. Genuinely
/// CLI-shaped tools (examples, the GTK headless-smoke harness) print
/// directly and carry their own justified `#[allow(clippy::print_stderr)]`
/// instead of going through this sink — they *are* the host, not a
/// library embedded in one.
pub(crate) fn emit(message: impl AsRef<str>) {
    let slot = sink_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(sink) = slot.as_ref() {
        sink(message.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // These tests share one process-global sink slot, so they can't run
    // concurrently with each other without stomping on each other's
    // installed sink. `cargo test` runs `#[test]` fns in this module on
    // separate threads by default; serialise with a plain mutex rather
    // than reaching for a test-only dependency.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn default_sink_is_silent() {
        let _g = guard();
        clear_sink();
        // No sink installed: emit must not panic and must not be
        // observable — there's nothing to assert *against* here beyond
        // "this doesn't crash," which is the point.
        emit("quadraui: nobody is listening");
    }

    #[test]
    fn installed_sink_receives_the_message() {
        let _g = guard();
        let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let received_in_sink = Arc::clone(&received);
        set_sink(move |msg: &str| {
            received_in_sink.lock().unwrap().push(msg.to_string());
        });

        emit("quadraui: hello");

        assert_eq!(received.lock().unwrap().as_slice(), ["quadraui: hello"]);
        clear_sink();
    }

    #[test]
    fn clear_sink_restores_silence() {
        let _g = guard();
        let count = Arc::new(AtomicUsize::new(0));
        let count_in_sink = Arc::clone(&count);
        set_sink(move |_msg: &str| {
            count_in_sink.fetch_add(1, Ordering::SeqCst);
        });
        emit("quadraui: one");
        clear_sink();
        emit("quadraui: two — should not be counted");

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn set_sink_replaces_previous_sink() {
        let _g = guard();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let first_calls_in_sink = Arc::clone(&first_calls);
        set_sink(move |_msg: &str| {
            first_calls_in_sink.fetch_add(1, Ordering::SeqCst);
        });

        let second_calls = Arc::new(AtomicUsize::new(0));
        let second_calls_in_sink = Arc::clone(&second_calls);
        set_sink(move |_msg: &str| {
            second_calls_in_sink.fetch_add(1, Ordering::SeqCst);
        });

        emit("quadraui: routed to whichever sink is current");

        assert_eq!(first_calls.load(Ordering::SeqCst), 0);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        clear_sink();
    }
}
