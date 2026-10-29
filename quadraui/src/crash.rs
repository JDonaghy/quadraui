//! Crash reporter seam — Electron's `crashReporter` counterpart.
//!
//! A quadraui app's only failure mode today is `std::panic`'s default
//! behaviour: a message on stderr that a host owning the terminal (vimcode's
//! TUI, running raw-mode on the alternate screen) usually can't show, and
//! nothing durable left behind once the process exits. [`install_crash_handler`]
//! installs a `std::panic` hook that, on top of whatever hook was already
//! installed (chained, not replaced — the default "thread panicked" message
//! still prints), writes a structured [`CrashRecord`] — message, thread
//! name, backtrace, app version, caller-supplied metadata — to disk as JSON
//! before unwinding continues.
//!
//! This module ships the backend-neutral panic hook and on-disk record
//! only. Native minidump capture and symbol upload to [`CrashReporterConfig::
//! upload_endpoint`] are per-platform follow-ons this module does not
//! attempt — `upload_endpoint` and [`CrashReporterConfig::metadata`] are
//! captured into every [`CrashRecord`] so a later follow-on has them to work
//! with, but nothing in this module performs any network I/O.
//!
//! Compiled unconditionally, like `paths`/`login_item`/`power`/`update`: a
//! panic hook is identical logic on every backend, so this is a free
//! function rather than a new [`crate::PlatformServices`] method — there is
//! no backend-varying implementation to override.

use std::backtrace::Backtrace;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::backend::{BackendError, ServiceResult};
use crate::paths::AppPathKind;

/// Configuration for [`install_crash_handler`]. Mirrors the handful of
/// Electron `crashReporter.start` options that make sense without native
/// minidump/upload support: where to write crash records, what app version
/// to stamp on them, and arbitrary extra metadata to carry along.
///
/// `Serialize`/`Deserialize`, following [`crate::login_item::LoginItemOptions`]'s
/// precedent: a config-driven caller can construct one from deserialized
/// data rather than only from a struct literal. `#[non_exhaustive]` for the
/// same reason — native minidump/upload follow-ons are likely to want more
/// fields later, and this type has no consumers to break today.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CrashReporterConfig {
    /// The app version stamped on every [`CrashRecord`]. Falls back to
    /// `"unknown"` in the written record when unset — see
    /// [`CrashRecord::app_version`].
    #[serde(default)]
    pub app_version: Option<String>,
    /// Where a later follow-on would upload crash records. Carried into
    /// every [`CrashRecord`] but never dereferenced by this module — no
    /// network I/O happens here.
    #[serde(default)]
    pub upload_endpoint: Option<String>,
    /// Arbitrary extra fields (build id, user id, feature flags, …) to
    /// stamp on every [`CrashRecord`].
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Directory to write crash records into. When `None` (the default),
    /// [`install_crash_handler`] resolves it via
    /// [`crate::paths::resolve`]`(`[`AppPathKind::Logs`]`)`, which requires a
    /// backend feature (`tui`/`gtk`/`macos`/`win`) and
    /// [`crate::paths::set_app_identifier`] to have been called first. A
    /// caller that wants a fixed, backend-independent location — or that is
    /// building with no backend feature enabled at all — sets this
    /// directly.
    #[serde(default)]
    pub log_dir: Option<PathBuf>,
}

impl CrashReporterConfig {
    /// Default config — no app version, no upload endpoint, no metadata,
    /// and [`Self::log_dir`] resolved from [`crate::paths`] at handler
    /// install time.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`Self::app_version`].
    #[must_use]
    pub fn with_app_version(mut self, app_version: impl Into<String>) -> Self {
        self.app_version = Some(app_version.into());
        self
    }

    /// Sets [`Self::upload_endpoint`].
    #[must_use]
    pub fn with_upload_endpoint(mut self, upload_endpoint: impl Into<String>) -> Self {
        self.upload_endpoint = Some(upload_endpoint.into());
        self
    }

    /// Sets [`Self::metadata`], *replacing* any set by a previous call —
    /// this is not an appender.
    #[must_use]
    pub fn with_metadata(mut self, metadata: impl IntoIterator<Item = (String, String)>) -> Self {
        self.metadata = metadata.into_iter().collect();
        self
    }

    /// Sets [`Self::log_dir`], bypassing [`crate::paths::resolve`] entirely.
    #[must_use]
    pub fn with_log_dir(mut self, log_dir: impl Into<PathBuf>) -> Self {
        self.log_dir = Some(log_dir.into());
        self
    }
}

/// The on-disk, JSON-serialised record [`install_crash_handler`]'s panic
/// hook writes for each panic. One file per panic, named
/// `crash-<unix_nanos>-<pid>.json` inside the configured directory, so
/// concurrent panics (e.g. from two threads at once) never collide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrashRecord {
    /// Nanoseconds since the Unix epoch when the panic was observed.
    pub unix_nanos: u128,
    /// The name of the thread that panicked (`<unnamed>` when the thread
    /// has none).
    pub thread: String,
    /// The panic payload, formatted as text. Covers both `panic!("literal")`
    /// (a `&'static str` payload) and `panic!("{}", x)` (a `String`
    /// payload); anything else downcasts to a fixed placeholder string.
    pub message: String,
    /// `file:line:column` of the panic site, when the standard library
    /// supplied one (it always does on every target this crate builds for).
    pub location: Option<String>,
    /// A captured backtrace, formatted via `Debug`. Captured
    /// unconditionally via [`Backtrace::force_capture`] rather than
    /// [`Backtrace::capture`], so a record is useful regardless of whether
    /// the host process set `RUST_BACKTRACE`.
    pub backtrace: String,
    /// [`CrashReporterConfig::app_version`] at install time, or `"unknown"`
    /// when that was `None`.
    pub app_version: String,
    /// [`CrashReporterConfig::upload_endpoint`] at install time.
    pub upload_endpoint: Option<String>,
    /// [`CrashReporterConfig::metadata`] at install time.
    pub metadata: BTreeMap<String, String>,
}

/// Install a `std::panic` hook that writes a [`CrashRecord`] to disk for
/// every panic, then chains into whatever hook was previously installed
/// (so the default "thread '…' panicked at …" stderr message, or any other
/// host-installed hook, still runs).
///
/// Resolves the write-target directory from [`CrashReporterConfig::log_dir`]
/// when set, otherwise from [`crate::paths::resolve`]`(`[`AppPathKind::Logs`]`)`
/// — which returns [`BackendError::Unsupported`] unless a backend feature
/// (`tui`/`gtk`/`macos`/`win`) is enabled and
/// [`crate::paths::set_app_identifier`] has already been called. Creates the
/// directory (and its ancestors) up front so that error surfaces at install
/// time rather than silently inside the panic hook later.
///
/// The panic hook itself is best-effort: a failure to write the record
/// (disk full, permissions, directory removed after install) is discarded
/// rather than risking a second panic inside a panic hook.
pub fn install_crash_handler(cfg: CrashReporterConfig) -> ServiceResult<()> {
    let dir = match &cfg.log_dir {
        Some(dir) => dir.clone(),
        None => crate::paths::resolve(AppPathKind::Logs)?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| BackendError::PlatformFailure {
        context: format!(
            "install_crash_handler: create_dir_all({}): {e}",
            dir.display()
        ),
    })?;

    let app_version = cfg
        .app_version
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let upload_endpoint = cfg.upload_endpoint.clone();
    let metadata = cfg.metadata.clone();

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_crash_record(
            &dir,
            &app_version,
            upload_endpoint.as_deref(),
            &metadata,
            info,
        );
        previous(info);
    }));
    Ok(())
}

/// Builds a [`CrashRecord`] from `info` and writes it to `dir`. Discards any
/// I/O error — see [`install_crash_handler`]'s doc for why.
fn write_crash_record(
    dir: &Path,
    app_version: &str,
    upload_endpoint: Option<&str>,
    metadata: &BTreeMap<String, String>,
    info: &std::panic::PanicHookInfo<'_>,
) {
    let unix_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let thread = std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_string();
    let record = CrashRecord {
        unix_nanos,
        thread,
        message: panic_message(info),
        location: info.location().map(ToString::to_string),
        backtrace: Backtrace::force_capture().to_string(),
        app_version: app_version.to_string(),
        upload_endpoint: upload_endpoint.map(ToString::to_string),
        metadata: metadata.clone(),
    };

    let Ok(json) = serde_json::to_vec_pretty(&record) else {
        return;
    };
    let path = dir.join(format!("crash-{unix_nanos}-{}.json", std::process::id()));
    let _ = std::fs::write(path, json);
}

/// Best-effort text for a panic payload. Covers the two shapes the standard
/// `panic!` macro actually produces (`&'static str` for a literal message,
/// `String` for a formatted one); anything else — `std::panic::panic_any`
/// with a non-string payload — downcasts to neither and falls back to a
/// fixed placeholder.
fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serialises every test in this module: `install_crash_handler`
    /// mutates the process-global panic hook, so two tests installing one
    /// concurrently would each risk chaining onto (or clobbering) the
    /// other's hook.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn crash_records_in(dir: &Path) -> Vec<CrashRecord> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if let Ok(record) = serde_json::from_slice::<CrashRecord>(&bytes) {
                out.push(record);
            }
        }
        out
    }

    #[test]
    fn config_builders_round_trip_through_json() {
        let cfg = CrashReporterConfig::new()
            .with_app_version("1.2.3")
            .with_upload_endpoint("https://example.test/crashes")
            .with_metadata([("build".to_string(), "nightly".to_string())])
            .with_log_dir("/tmp/example-does-not-need-to-exist");
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CrashReporterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn deserializes_from_empty_json() {
        let cfg: CrashReporterConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, CrashReporterConfig::default());
    }

    #[test]
    fn install_without_log_dir_or_backend_feature_is_unsupported_or_ok() {
        let _g = guard();
        let previous = std::panic::take_hook();
        let result = install_crash_handler(CrashReporterConfig::new());
        // Restore whatever was installed before this test ran, regardless
        // of outcome, so later tests (in this module or elsewhere) never
        // observe this hook.
        std::panic::set_hook(previous);
        // With no `log_dir` and no backend feature enabled, `paths::resolve`
        // returns `Unsupported`; with a backend feature enabled (`tui` in
        // this crate's own test matrix) it may resolve a real directory
        // and succeed instead. Either is correct — this only pins that no
        // other error shape leaks out.
        match result {
            Ok(()) => {}
            Err(BackendError::Unsupported) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    /// The acceptance scenario from the issue: a panic induced in a child
    /// thread, with the handler installed against an explicit `log_dir`,
    /// writes a crash record containing the panic message and a backtrace.
    #[test]
    fn panic_in_child_thread_writes_crash_record_with_message_and_backtrace() {
        let _g = guard();
        let dir = std::env::temp_dir().join(format!(
            "quadraui-crash-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let previous = std::panic::take_hook();
        install_crash_handler(
            CrashReporterConfig::new()
                .with_app_version("9.9.9")
                .with_log_dir(dir.clone()),
        )
        .unwrap();

        let handle = std::thread::Builder::new()
            .name("crash-test-thread".to_string())
            .spawn(|| {
                panic!("induced test panic for crash reporter acceptance");
            })
            .unwrap();
        let join_result = handle.join();

        std::panic::set_hook(previous);
        assert!(join_result.is_err(), "child thread was expected to panic");

        let records = crash_records_in(&dir);
        assert_eq!(records.len(), 1, "expected exactly one crash record");
        let record = &records[0];
        assert_eq!(record.thread, "crash-test-thread");
        assert!(
            record
                .message
                .contains("induced test panic for crash reporter acceptance"),
            "message was: {}",
            record.message
        );
        assert!(!record.backtrace.is_empty());
        assert_eq!(record.app_version, "9.9.9");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_and_upload_endpoint_round_trip_into_the_record() {
        let _g = guard();
        let dir = std::env::temp_dir().join(format!(
            "quadraui-crash-test-meta-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let previous = std::panic::take_hook();
        install_crash_handler(
            CrashReporterConfig::new()
                .with_upload_endpoint("https://example.test/crashes")
                .with_metadata([("build".to_string(), "nightly".to_string())])
                .with_log_dir(dir.clone()),
        )
        .unwrap();

        let handle = std::thread::spawn(|| {
            panic!("second induced test panic");
        });
        let _ = handle.join();

        std::panic::set_hook(previous);

        let records = crash_records_in(&dir);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(
            record.upload_endpoint.as_deref(),
            Some("https://example.test/crashes")
        );
        assert_eq!(
            record.metadata.get("build").map(String::as_str),
            Some("nightly")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
