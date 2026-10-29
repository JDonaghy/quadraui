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
/// `Serialize`/`Deserialize`, following [`crate::types::StyledSpan`] and
/// [`crate::a11y::A11yInfo`]'s precedent rather than `Notification`'s/
/// `FileDialogOptions`'s (neither of which derives serde): a config-driven
/// caller (declarative app config, a settings file) can construct one from
/// deserialized data rather than only from a struct literal. Every field
/// carries `#[serde(default)]` for the same reason those two types do —
/// `serde_json::from_str::<LoginItemOptions>("{}")` succeeds (see
/// [`tests::deserializes_from_empty_json`] below), so a partial object in
/// a settings file still deserializes, and a field a later backend
/// follow-on adds stays backward-compatible with values serialized before
/// that field existed.
///
/// `#[non_exhaustive]`: there are no consumers of this type yet, but the
/// Electron API it mirrors carries more fields (`path`, `name`,
/// `serviceName`) that a per-backend follow-on (`SMAppService`/registry/
/// XDG autostart) is likely to want; `#[non_exhaustive]` keeps a future
/// field addition additive rather than a semver-major break on a type
/// with no consumers to break today. Construct with [`LoginItemOptions::
/// new`]/[`LoginItemOptions::default`] and the `with_*` builders, mirroring
/// [`crate::a11y::A11yInfo`]'s shape.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginItemOptions {
    /// Launch the app hidden/minimized (no visible window, no Dock/taskbar
    /// flash) rather than opening its normal startup UI. Mirrors
    /// Electron's `openAsHidden` (macOS) / `args`-based hidden-launch
    /// convention on Windows.
    #[serde(default)]
    pub open_hidden: bool,
    /// Extra command-line arguments to pass on the login launch, e.g. a
    /// flag the app checks to skip onboarding UI when started this way.
    /// Empty by default — a plain launch with no extra arguments.
    ///
    /// A per-backend registration follow-on that writes this into a
    /// Windows `Run` registry value or an XDG autostart `.desktop`
    /// `Exec=` line is responsible for quoting/escaping each element for
    /// that destination's own re-parsing rules (`CreateProcess`
    /// command-line splitting; `.desktop` `%`-field codes, `;`
    /// key-separation, and embedded-newline key termination) — this type
    /// stores the arguments verbatim and validates nothing.
    #[serde(default)]
    pub args: Vec<String>,
}

impl LoginItemOptions {
    /// Default options — launch visible, with no extra arguments.
    /// Equivalent to [`LoginItemOptions::default`], spelled as a
    /// constructor for symmetry with the `with_*` builders.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`Self::open_hidden`].
    #[must_use]
    pub fn with_open_hidden(mut self, open_hidden: bool) -> Self {
        self.open_hidden = open_hidden;
        self
    }

    /// Sets [`Self::args`], *replacing* any set by a previous call — this
    /// is not an appender.
    #[must_use]
    pub fn with_args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args = args.into_iter().collect();
        self
    }
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
        let opts = LoginItemOptions::new()
            .with_open_hidden(true)
            .with_args(["--minimized".to_string(), "--no-update-check".to_string()]);
        let json = serde_json::to_string(&opts).unwrap();
        let back: LoginItemOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(opts, back);
    }

    /// `#[serde(default)]` on every field: a partial (or empty) JSON
    /// object still deserializes rather than failing with "missing
    /// field" — the same guarantee [`crate::a11y::A11yInfo`]'s own
    /// `deserializes_from_empty_json` test pins, and the property a
    /// config-driven caller (declarative app config, a settings file)
    /// actually needs from this type's serde derive.
    #[test]
    fn deserializes_from_empty_json() {
        let opts: LoginItemOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(opts, LoginItemOptions::default());
    }

    /// A partial object — only one of the two fields present — also
    /// deserializes, with the missing field falling back to its default.
    #[test]
    fn deserializes_from_partial_json() {
        let opts: LoginItemOptions = serde_json::from_str(r#"{"open_hidden":true}"#).unwrap();
        assert_eq!(opts, LoginItemOptions::new().with_open_hidden(true));
    }

    #[test]
    fn login_item_options_equality_is_field_based() {
        let a = LoginItemOptions::new()
            .with_open_hidden(true)
            .with_args(["--x".to_string()]);
        let b = LoginItemOptions::new()
            .with_open_hidden(true)
            .with_args(["--x".to_string()]);
        let c = LoginItemOptions::new()
            .with_open_hidden(false)
            .with_args(["--x".to_string()]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
