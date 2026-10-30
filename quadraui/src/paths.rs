//! Cross-platform, app-identity-scoped filesystem locations.
//!
//! Electron parity target: `app.getPath('userData' | 'appData' | 'logs' |
//! 'temp' | 'downloads' | 'cache' | 'documents' | 'home')`. Every quadraui
//! app currently hand-rolls its own per-OS config/data/log/cache location;
//! this module gives [`PlatformServices::app_path`](crate::backend::PlatformServices::app_path)
//! one shared answer instead, backed by the cross-platform `directories`
//! crate plus `std::env::temp_dir`. Resolving these needs only a
//! filesystem and environment variables — no window, no display server,
//! no desktop session — so, like
//! [`PlatformServices::secret_store`](crate::backend::PlatformServices::secret_store),
//! this is full support on TUI, not a degrade, and no backend overrides
//! the trait's default.
//!
//! `UserData`/`AppConfig`/`AppCache`/`Logs` are scoped by an app
//! identifier that defaults to the running binary's file stem but can be
//! overridden with [`set_app_identifier`] — call it once during app
//! startup, before the first [`PlatformServices::app_path`](crate::backend::PlatformServices::app_path)
//! call, the same way a host installs [`crate::diagnostics::set_sink`]
//! before relying on it. The identifier is process-global state, mirroring
//! that module's `OnceLock<RwLock<_>>` slot.

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::backend::{BackendError, ServiceResult};

/// Which standard, per-app filesystem location to resolve. Mirrors
/// Electron's `app.getPath` name set — see the module doc.
///
/// `#[non_exhaustive]`: a future kind (e.g. a `Desktop` or `Music` folder)
/// is an additive match arm here, not a breaking one, for any downstream
/// `match` that already carries a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AppPathKind {
    /// Durable, app-scoped data (Electron's `userData`) — macOS
    /// `~/Library/Application Support/<app>`, Linux
    /// `$XDG_DATA_HOME/<app>`, Windows `%APPDATA%\<app>\data`.
    UserData,
    /// App-scoped configuration (Electron's `appData`'s config-file use,
    /// kept distinct from [`Self::UserData`] since `directories` itself
    /// distinguishes config from data on every platform).
    AppConfig,
    /// App-scoped cache — safe to delete; the OS may clear it under disk
    /// pressure (Linux: `$XDG_CACHE_HOME/<app>`).
    AppCache,
    /// App-scoped log directory. `directories` has no dedicated log
    /// location, so this is [`Self::UserData`]'s directory with a
    /// `logs` subdirectory appended — an approximation, not a literal
    /// per-OS standard the way the other kinds are.
    Logs,
    /// The OS-wide temporary directory (`std::env::temp_dir`) — not
    /// app-scoped, same as Electron's `temp`.
    Temp,
    /// The user's Downloads folder, unscoped by app identity. On Linux this
    /// is read from the XDG `user-dirs.dirs` file written by a desktop
    /// session; on a headless host where that file was never written this
    /// falls back to `<home>/Downloads` rather than reporting
    /// [`crate::backend::BackendError::Unsupported`] for an ordinary,
    /// non-degraded capability.
    Downloads,
    /// The user's Documents folder, unscoped by app identity. Same
    /// XDG-file-then-`<home>/Documents` fallback as [`Self::Downloads`].
    Documents,
    /// The user's home directory, unscoped by app identity.
    Home,
}

fn identifier_slot() -> &'static RwLock<Option<String>> {
    static SLOT: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Set the app identifier that scopes [`AppPathKind::UserData`],
/// [`AppPathKind::AppConfig`], [`AppPathKind::AppCache`] and
/// [`AppPathKind::Logs`]. Process-global, like
/// [`crate::diagnostics::set_sink`] — call it once at startup, before the
/// first [`PlatformServices::app_path`](crate::backend::PlatformServices::app_path)
/// call that needs it.
///
/// `identifier` must be a single plain directory-name component: non-empty,
/// containing no path separator (`/` or, on Windows, `\`) and not `.` or
/// `..`. An identifier that fails this check is ignored — the previously
/// set (or default) identifier stays in effect — rather than being
/// interpolated into a traversing path; this matters over FFI
/// (`qui_set_app_identifier`), where the value may come from host-supplied
/// config rather than a literal in this process's own source.
pub fn set_app_identifier(identifier: impl Into<String>) {
    let identifier = identifier.into();
    if !is_valid_identifier(&identifier) {
        return;
    }
    let mut slot = identifier_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(identifier);
}

/// Whether `identifier` is safe to interpolate as a single path component —
/// see [`set_app_identifier`]'s doc for the contract.
fn is_valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier != "."
        && identifier != ".."
        && !identifier.chars().any(std::path::is_separator)
}

/// Clear any identifier set via [`set_app_identifier`], restoring the
/// default. Mainly useful for tests — the identifier is process-global,
/// so a test that sets one and doesn't clear it can leak into an
/// unrelated test running later in the same binary.
pub fn clear_app_identifier() {
    let mut slot = identifier_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = None;
}

/// The current app identifier: whatever [`set_app_identifier`] last set,
/// or — if nothing was ever set — the running binary's file stem (its file
/// name without extension), or the fixed literal `"quadraui-app"` if even
/// that can't be determined (e.g. `std::env::current_exe` failing). Never
/// empty.
pub fn app_identifier() -> String {
    let slot = identifier_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(id) = slot.as_ref() {
        return id.clone();
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "quadraui-app".to_string())
}

/// Serialises every test that touches the process-global identifier slot
/// via [`set_app_identifier`]/[`clear_app_identifier`] — `cargo test`
/// runs `#[test]` fns on separate threads by default, and two tests
/// mutating the same global concurrently would each risk observing (or
/// clobbering) the other's identifier. Mirrors
/// [`crate::diagnostics::test_guard`]'s shape exactly.
///
/// `pub(crate)`, not private: `tui::services`'s `app_path_tests` module
/// also sets/clears the identifier (pinning
/// [`crate::backend::PlatformServices::app_path`]'s default body end to
/// end through [`crate::tui::services::TuiPlatformServices`]) and must
/// serialise against this module's own tests through the same lock.
///
/// Gated the same as `mod tests` below, in-module its only caller: with
/// none of the four backend features enabled this function would
/// otherwise be compiled but unreachable, and `-D warnings` turns that
/// dead-code lint into a hard build failure. `tui::services`'s caller is
/// itself gated on `feature = "tui"`, one of these four, so the
/// reachability story composes.
#[cfg(test)]
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Resolve `kind` using the `directories` crate (app-scoped kinds and
/// [`AppPathKind::Home`]/[`AppPathKind::Documents`]/[`AppPathKind::Downloads`])
/// plus `std::env::temp_dir` ([`AppPathKind::Temp`]). The one
/// implementation [`PlatformServices::app_path`](crate::backend::PlatformServices::app_path)'s
/// default vends on every backend — see that method's doc and this
/// module's doc for why no backend needs its own override.
///
/// `pub`, not `pub(crate)`: resolving a standard path needs no `Backend`
/// instance and no window, so a caller that only wants this one
/// capability — `quadraui-ffi`'s C ABI included — can call it directly
/// instead of standing up a whole [`crate::backend::Backend`] just to
/// reach [`crate::backend::PlatformServices::app_path`] through it.
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
pub fn resolve(kind: AppPathKind) -> ServiceResult<PathBuf> {
    match kind {
        AppPathKind::Temp => Ok(std::env::temp_dir()),
        AppPathKind::Home => directories::UserDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or(BackendError::Unsupported),
        AppPathKind::Documents => {
            let dirs = directories::UserDirs::new().ok_or(BackendError::Unsupported)?;
            Ok(dirs
                .document_dir()
                .map(Into::into)
                .unwrap_or_else(|| dirs.home_dir().join("Documents")))
        }
        AppPathKind::Downloads => {
            let dirs = directories::UserDirs::new().ok_or(BackendError::Unsupported)?;
            Ok(dirs
                .download_dir()
                .map(Into::into)
                .unwrap_or_else(|| dirs.home_dir().join("Downloads")))
        }
        AppPathKind::UserData => project_dirs().map(|dirs| dirs.data_dir().to_path_buf()),
        AppPathKind::AppConfig => project_dirs().map(|dirs| dirs.config_dir().to_path_buf()),
        AppPathKind::AppCache => project_dirs().map(|dirs| dirs.cache_dir().to_path_buf()),
        AppPathKind::Logs => project_dirs().map(|dirs| dirs.data_dir().join("logs")),
    }
}

/// `ProjectDirs` scoped by [`app_identifier`]. Empty qualifier and
/// organization strings are explicitly documented as supported by
/// `directories` — quadraui apps have no reverse-DNS qualifier or
/// organization name of their own to supply, only the one app
/// identifier.
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
fn project_dirs() -> ServiceResult<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", &app_identifier()).ok_or(BackendError::Unsupported)
}

/// Fallback for a build with none of the `tui`/`gtk`/`macos`/`win`
/// features enabled — the `directories` crate isn't even a dependency in
/// that configuration (see its Cargo.toml gate), so there is nothing to
/// resolve; every kind honestly reports [`BackendError::Unsupported`]
/// rather than the crate failing to build at all. Mirrors
/// `KeyringSecretStore`'s own no-feature fallback in `backend.rs`.
#[cfg(not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")))]
pub fn resolve(_kind: AppPathKind) -> ServiceResult<PathBuf> {
    Err(BackendError::Unsupported)
}

#[cfg(test)]
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
mod tests {
    use super::*;

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        super::test_guard()
    }

    #[test]
    fn every_kind_resolves_to_a_non_empty_path() {
        let _g = guard();
        clear_app_identifier();
        set_app_identifier("quadraui-paths-test");

        let scoped_by_identifier = [
            AppPathKind::UserData,
            AppPathKind::AppConfig,
            AppPathKind::AppCache,
            AppPathKind::Logs,
        ];

        for kind in [
            AppPathKind::UserData,
            AppPathKind::AppConfig,
            AppPathKind::AppCache,
            AppPathKind::Logs,
            AppPathKind::Temp,
            AppPathKind::Downloads,
            AppPathKind::Documents,
            AppPathKind::Home,
        ] {
            let resolved = resolve(kind);
            assert!(
                resolved.is_ok(),
                "{kind:?} should resolve on this platform, got {resolved:?}"
            );
            let resolved = resolved.unwrap();
            assert!(
                !resolved.as_os_str().is_empty(),
                "{kind:?} resolved to an empty path"
            );
            if scoped_by_identifier.contains(&kind) {
                assert!(
                    resolved.to_string_lossy().contains("quadraui-paths-test"),
                    "{kind:?} should be scoped by the app identifier, got {resolved:?}"
                );
            }
        }

        clear_app_identifier();
    }

    #[test]
    fn app_identifier_scopes_user_data_and_config() {
        let _g = guard();
        clear_app_identifier();

        set_app_identifier("quadraui-app-one");
        let data_one = resolve(AppPathKind::UserData).unwrap();
        let config_one = resolve(AppPathKind::AppConfig).unwrap();

        set_app_identifier("quadraui-app-two");
        let data_two = resolve(AppPathKind::UserData).unwrap();
        let config_two = resolve(AppPathKind::AppConfig).unwrap();

        assert_ne!(data_one, data_two);
        assert_ne!(config_one, config_two);

        clear_app_identifier();
    }

    #[test]
    fn default_identifier_is_stable_and_non_empty() {
        let _g = guard();
        clear_app_identifier();
        let id = app_identifier();
        assert!(!id.is_empty());
        assert_eq!(id, app_identifier());
        clear_app_identifier();
    }

    #[test]
    fn set_app_identifier_rejects_path_traversal_and_empty_identifiers() {
        let _g = guard();
        clear_app_identifier();

        set_app_identifier("quadraui-valid-identifier");
        assert_eq!(app_identifier(), "quadraui-valid-identifier");

        for rejected in ["", ".", "..", "../escape", "a/b"] {
            set_app_identifier(rejected);
            assert_eq!(
                app_identifier(),
                "quadraui-valid-identifier",
                "{rejected:?} should have been rejected, leaving the prior identifier in place"
            );
        }

        clear_app_identifier();
    }
}
