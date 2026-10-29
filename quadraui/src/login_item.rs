//! Launch-at-login seam — the types
//! [`crate::PlatformServices::set_login_item`]/
//! [`crate::PlatformServices::login_item_enabled`] trade in. Electron's
//! `app.setLoginItemSettings`/`app.getLoginItemSettings` counterpart.
//!
//! This module defines the backend-neutral *contract* only — registering
//! (or unregistering) this app to launch when the user logs in, and
//! reading back whether it is currently registered. It ships no real
//! implementation: [`crate::PlatformServices::set_login_item`]/
//! [`crate::PlatformServices::login_item_enabled`] both default to
//! [`BackendError::Unsupported`][crate::BackendError::Unsupported] on every
//! in-tree backend. Wiring a real registration — `SMAppService` on macOS,
//! an `HKCU\...\Run` registry value on Win-GUI, an XDG autostart `.desktop`
//! file on GTK/Linux — is a separate, backend-specific follow-on; this
//! module only has to give each of those a contract to implement against.
//!
//! TUI has no OS-level login/session-start hook to register against — a
//! terminal app launching itself at login is physically absent the same
//! way a tray icon or dock badge is — so
//! [`crate::tui::services::TuiPlatformServices`] overrides
//! [`PlatformServices::login_item_enabled`][crate::PlatformServices::login_item_enabled]
//! with an honest no-op that always reports `false` rather than leaving it
//! `Unsupported`: a caller that only wants to display current state (not
//! mutate it) gets a truthful answer instead of an error on a query that
//! has an obvious right answer on this backend. `set_login_item` stays at
//! its `Unsupported` default on TUI — there is no honest "success" value
//! for a mutation with nothing to mutate.

use serde::{Deserialize, Serialize};

/// Options for [`crate::PlatformServices::set_login_item`] — mirrors the
/// handful of fields Electron's `app.setLoginItemSettings` accepts that
/// make sense cross-platform (`openAtLogin`'s sibling knobs, not the whole
/// macOS/Windows-specific grab-bag).
///
/// `Serialize`/`Deserialize` for the same reason `Notification`/
/// `FileDialogOptions` carry it elsewhere in `backend.rs`: a config-driven
/// caller (declarative app config, a settings file) can construct one from
/// deserialized data rather than only from a struct literal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginItemOptions {
    /// Launch the app hidden/minimized (no visible window, no Dock/taskbar
    /// flash) rather than opening its normal startup UI. Mirrors
    /// Electron's `openAsHidden` (macOS) /// `args`-based hidden-launch
    /// convention on Windows.
    pub open_hidden: bool,
    /// Extra command-line arguments to pass on the login launch, e.g. a
    /// flag the app checks to skip onboarding UI when started this way.
    /// Empty by default — a plain launch with no extra arguments.
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_item_options_default_is_visible_with_no_args() {
        let opts = LoginItemOptions::default();
        assert!(!opts.open_hidden);
        assert!(opts.args.is_empty());
    }

    #[test]
    fn login_item_options_round_trips_through_json() {
        let opts = LoginItemOptions {
            open_hidden: true,
            args: vec!["--minimized".to_string(), "--no-update-check".to_string()],
        };
        let json = serde_json::to_string(&opts).unwrap();
        let back: LoginItemOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(opts, back);
    }

    #[test]
    fn login_item_options_equality_is_field_based() {
        let a = LoginItemOptions {
            open_hidden: true,
            args: vec!["--x".to_string()],
        };
        let b = LoginItemOptions {
            open_hidden: true,
            args: vec!["--x".to_string()],
        };
        let c = LoginItemOptions {
            open_hidden: false,
            args: vec!["--x".to_string()],
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
