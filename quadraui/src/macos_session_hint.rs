//! Diagnostic for a macOS process that AppKit never actually activates:
//! the window opens behind other windows, there is no Dock icon, and
//! keystrokes keep landing in the launching terminal instead of the
//! app. AppKit prints nothing when this happens, so the person running
//! the binary has no signal beyond "it looks broken" — this module adds
//! one.
//!
//! The one confirmed trigger (found interactively on real hardware): a
//! `tmux` server started before the current GUI login session (it
//! outlives a logout/login or fast-user-switch). The same binary
//! launched from a plain `Terminal.app` window, or from a *fresh* tmux
//! server, activates normally — so the failure tracks session identity,
//! not tmux itself, and `$TMUX` being set is the best available signal
//! for naming the likely cause in the hint.
//!
//! [`activation_hint`] is the pure decision function both macOS
//! backends call from a short delayed check after launch —
//! `macos::run::QuadraAppDelegate`'s `applicationDidFinishLaunching:`
//! and `gtk::run`'s `activate_macos_process` call site. Neither backend's
//! activation sequence itself changes: this module only detects and
//! reports the failure.

use std::env;

/// Set to any non-empty value to silence this module's stderr output.
/// Headless build/test lanes that launch a macOS binary outside any GUI
/// session hit the exact condition this module diagnoses as a matter of
/// course — this keeps that expected, harmless case quiet.
pub const NO_SESSION_HINT_ENV: &str = "QUADRAUI_NO_SESSION_HINT";

/// `true` when [`NO_SESSION_HINT_ENV`] is set to anything (including an
/// empty string — presence, not content, is what's checked, matching
/// every other opt-out env var in this crate).
pub fn session_hint_disabled() -> bool {
    env::var_os(NO_SESSION_HINT_ENV).is_some()
}

/// Decide whether to print a one-shot stderr hint about this process's
/// window never activating, and what it should say.
///
/// Returns `None` once the app is active *or* has a key window — either
/// one is proof activation succeeded, even if the other hasn't caught up
/// yet by the time a caller's delayed check runs (both backends sample
/// once, ~1.5s after launch, rather than polling until both agree).
///
/// Otherwise returns one actionable line: naming tmux when `tmux` is
/// `Some` (pass the `$TMUX` environment variable's value — this
/// function stays pure and never reads the environment itself, so it is
/// trivially unit-testable against the full truth table), or a generic
/// "outside the GUI login session" hint when it's `None`.
pub fn activation_hint(
    is_active: bool,
    has_key_window: bool,
    tmux: Option<&str>,
) -> Option<String> {
    if is_active || has_key_window {
        return None;
    }
    Some(if tmux.is_some() {
        "quadraui: macOS did not let this window activate (no Dock icon, keys go to the \
         terminal). This usually means it was launched from a tmux server that predates your \
         login session — run it from a plain Terminal window, or restart tmux (`tmux \
         kill-server`)."
            .to_string()
    } else {
        "quadraui: macOS did not let this window activate (no Dock icon, keys go to the \
         terminal). This usually means it was launched outside the GUI login session (ssh, \
         launchd, stale tmux)."
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_app_gets_no_hint_regardless_of_tmux() {
        assert_eq!(activation_hint(true, false, None), None);
        assert_eq!(
            activation_hint(true, false, Some("/tmp/tmux-1000/default,123,0")),
            None
        );
    }

    #[test]
    fn key_window_alone_is_enough_even_if_not_yet_active() {
        assert_eq!(activation_hint(false, true, None), None);
        assert_eq!(
            activation_hint(false, true, Some("/tmp/tmux-1000/default,123,0")),
            None
        );
    }

    #[test]
    fn active_and_key_window_gets_no_hint() {
        assert_eq!(
            activation_hint(true, true, Some("/tmp/tmux-1000/default,123,0")),
            None
        );
    }

    #[test]
    fn neither_active_nor_key_window_without_tmux_is_generic() {
        let hint = activation_hint(false, false, None).expect("a hint is expected");
        assert!(hint.contains("outside the GUI login session"));
        assert!(!hint.contains("tmux server"));
    }

    #[test]
    fn neither_active_nor_key_window_with_tmux_names_tmux() {
        let hint = activation_hint(false, false, Some("/tmp/tmux-1000/default,123,0"))
            .expect("a hint is expected");
        assert!(hint.contains("tmux server"));
        assert!(hint.contains("tmux kill-server"));
    }

    #[test]
    fn session_hint_disabled_tracks_the_env_var() {
        // `env::set_var`/`remove_var` are process-global and this test
        // runs alongside others in the same binary — scope the mutation
        // narrowly and restore it, matching this crate's other
        // env-var-gated tests (e.g. `gtk::run`'s `smoke_config_from_env_*`).
        let had = env::var_os(NO_SESSION_HINT_ENV);
        env::set_var(NO_SESSION_HINT_ENV, "1");
        assert!(session_hint_disabled());
        env::remove_var(NO_SESSION_HINT_ENV);
        assert!(!session_hint_disabled());
        if let Some(value) = had {
            env::set_var(NO_SESSION_HINT_ENV, value);
        }
    }
}
