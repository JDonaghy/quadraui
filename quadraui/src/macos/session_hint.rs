//! Diagnostic hint macOS prints to stderr when a launched app never
//! takes over as the foreground application — the "no Dock icon, keys go
//! to the terminal" failure mode [`run`][super::run] cannot prevent (its
//! own `ns_app.activate()` call site doc explains why: activation is
//! advisory on macOS 14+, not something this crate can force), but can at
//! least stop leaving silent.
//!
//! [`activation_hint`] is the pure decision function — unit-testable with
//! no live `NSApplication` — and [`install_session_hint_timer`] is the
//! one-shot `NSTimer` that calls it ~1.5s after the window opens, giving
//! AppKit time to settle before judging the app failed to activate. Opt
//! out with `QUADRAUI_NO_SESSION_HINT=1`, read by
//! [`install_session_hint_timer`] — set by the headless test lanes so a
//! `MacDriver` run (no live window server behind it) stays quiet.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSTimer};

/// Delay before the one-shot check fires. Long enough that a normal,
/// successful activation (AppKit's own launch sequence plus the first
/// `drawRect:`) has already completed on any real hardware; short enough
/// that a person watching the terminal doesn't wonder why nothing
/// happened in the failure case.
const SESSION_HINT_DELAY_SECS: f64 = 1.5;

/// Env var that silences the hint entirely — no timer is even scheduled
/// when this is set. See the module doc for why the headless test lanes
/// need this.
pub(crate) const QUADRAUI_NO_SESSION_HINT: &str = "QUADRAUI_NO_SESSION_HINT";

/// Decide whether a launched app failed to activate and, if so, build the
/// one-shot stderr hint to print.
///
/// Returns `None` once the app is either the active application or has a
/// key window — the ordinary, successful case, which covers every launch
/// context confirmed not to trigger this (a plain `Terminal.app` launch,
/// a fresh tmux server). Otherwise returns a message. When `tmux` is
/// `Some`, the wording names the one condition actually confirmed on real
/// hardware to trigger this: a tmux server that predates the current
/// login session. Without `$TMUX`, the hint is a generic one naming the
/// broader class this failure belongs to — a process detached from the
/// GUI login session some other way (ssh, launchd, or a terminal emulator
/// whose own launch model leaves its child detached the same way a stale
/// tmux server does, a hypothesis this generic branch exists to let
/// someone confirm rather than one this function assumes).
pub(crate) fn activation_hint(
    app_name: &str,
    is_active: bool,
    has_key_window: bool,
    tmux: Option<&str>,
) -> Option<String> {
    if is_active || has_key_window {
        return None;
    }
    let condition = if tmux.is_some() {
        "launched from a tmux server that predates your login session — run it from a plain \
         Terminal window, or restart tmux (`tmux kill-server`)"
    } else {
        "launched outside the current GUI login session (ssh, launchd, a stale tmux server, \
         or a terminal emulator that detaches its child process) — try launching it from a \
         plain Terminal.app window instead"
    };
    Some(format!(
        "{app_name}: macOS did not let this window activate (no Dock icon, keys go to the \
         terminal). This usually means it was {condition}."
    ))
}

/// Per-instance state for [`QuadraSessionHintTarget`] — just the name the
/// hint prints itself as (the same string [`super::run::window_title`]
/// derives).
pub(crate) struct QuadraSessionHintTargetIvars {
    app_name: String,
}

define_class!(
    /// Obj-C target for the one-shot session-activation-hint `NSTimer`.
    /// Unlike [`QuadraBlinkTarget`][super::caret_blink::QuadraBlinkTarget],
    /// this fires once and is then dropped — there is nothing left to
    /// keep ticking after the first check.
    //
    // SAFETY: created and used exclusively on the main thread via
    // `MainThreadMarker`, matching `QuadraBlinkTarget`'s own safety doc —
    // the `NSTimer` callback fires on the run loop that scheduled it
    // (`mainRunLoop`, the main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QuadraSessionHintTargetIvars]
    pub(crate) struct QuadraSessionHintTarget;

    unsafe impl NSObjectProtocol for QuadraSessionHintTarget {}

    impl QuadraSessionHintTarget {
        #[unsafe(method(check:))]
        fn check(&self, _timer: &NSTimer) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            let is_active = app.isActive();
            let has_key_window = app.keyWindow().is_some();
            let tmux = std::env::var("TMUX").ok();
            if let Some(hint) = activation_hint(
                &self.ivars().app_name,
                is_active,
                has_key_window,
                tmux.as_deref(),
            ) {
                eprintln!("{hint}");
            }
        }
    }
);

impl QuadraSessionHintTarget {
    fn new(mtm: MainThreadMarker, app_name: String) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraSessionHintTargetIvars { app_name });
        unsafe { msg_send![super(this), init] }
    }
}

/// Schedule the one-shot activation-hint `NSTimer`, unless
/// `QUADRAUI_NO_SESSION_HINT` opts out. Returns `None` when opted out, in
/// which case no timer was scheduled at all. Callers (currently
/// [`super::run::run_with`]) hold the `Some` handles for the lifetime of
/// the app the same way
/// [`super::caret_blink::install_blink_timer`]'s caller does — dropping
/// the `NSTimer` before it fires invalidates it.
pub(crate) fn install_session_hint_timer(
    mtm: MainThreadMarker,
    app_name: String,
) -> Option<(Retained<QuadraSessionHintTarget>, Retained<NSTimer>)> {
    if std::env::var_os(QUADRAUI_NO_SESSION_HINT).is_some() {
        return None;
    }
    let target = QuadraSessionHintTarget::new(mtm, app_name);
    let timer: Retained<NSTimer> = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            SESSION_HINT_DELAY_SECS,
            &target,
            sel!(check:),
            None,
            false,
        )
    };
    Some((target, timer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_when_active() {
        assert_eq!(activation_hint("app", true, false, None), None);
    }

    #[test]
    fn none_when_has_key_window() {
        assert_eq!(activation_hint("app", false, true, None), None);
    }

    #[test]
    fn none_when_both_active_and_key_window() {
        assert_eq!(
            activation_hint("app", true, true, Some("/tmp/tmux-1000/default")),
            None
        );
    }

    #[test]
    fn tmux_specific_hint_when_neither_and_tmux_set() {
        let hint = activation_hint("app", false, false, Some("/tmp/tmux-1000/default"))
            .expect("neither active nor key window, with $TMUX set, must hint");
        assert!(hint.contains("app:"));
        assert!(hint.contains("tmux"));
        assert!(hint.contains("tmux kill-server"));
    }

    #[test]
    fn generic_hint_when_neither_and_no_tmux() {
        let hint = activation_hint("app", false, false, None)
            .expect("neither active nor key window, with no $TMUX, must still hint");
        assert!(hint.contains("app:"));
        assert!(!hint.contains("tmux kill-server"));
        assert!(hint.contains("ssh"));
        assert!(hint.contains("terminal emulator"));
    }
}
