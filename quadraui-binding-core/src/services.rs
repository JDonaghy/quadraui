//! Language-neutral wrappers over every [`quadraui::PlatformServices`] seam
//! this epic's "missing in both" table names — auto-update, the power-save
//! blocker, app standard paths, launch-at-login, protocol registration,
//! the crash reporter, drag-out, and recent docs/dock menu/jump list.
//!
//! Every function here takes a `&dyn PlatformServices` (or nothing, for
//! the two free-function seams — [`resolve_app_path`] and
//! [`install_crash_handler`], which need no live app) plus plain or
//! serde-wire arguments, and returns a plain or serde-wire result mapped
//! through [`ServiceError`] — the one shared error shape
//! `quadraui::BackendError` collapses into. A binding leaf (`quadraui-py`,
//! `quadraui-node`) decodes its own host-language arguments into these
//! plain/wire shapes, calls the function here, and maps `ServiceError`'s
//! three cases onto its own exception taxonomy — the same division of
//! labor [`crate::host::pointer_shape`] already draws for cursor-shape
//! decoding, just for a whole method call instead of one enum.
//!
//! [`ServiceError::Unsupported`]/[`ServiceError::Failure`] mirror
//! `BackendError::Unsupported`/the other two `BackendError` variants
//! exactly — see that type's own doc; [`ServiceError::InvalidArgument`]
//! is this module's own addition, for a wire argument that failed to
//! decode (an unknown enum-name string, a malformed `serde_json::Value`)
//! before any service call was even attempted.
//!
//! [`update_download_and_stage`]/[`update_apply_on_quit`] and
//! [`begin_power_save_block`]/[`end_power_save_block`] each mint an opaque
//! `u64` handle rather than serializing `quadraui::StagedUpdate`/
//! `quadraui::BlockerId` across the wire — neither type derives
//! `Serialize`, and `BlockerId`'s own doc deliberately keeps its `u64`
//! private (identity only, no meaningful external form). The handle is
//! looked up in a process-global registry keyed by that `u64`, the same
//! shape [`quadraui::paths`]'s own `identifier_slot` uses for its
//! process-global app-identifier cell.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use quadraui::backend::JumpListItem;
use quadraui::{
    BackendError, BlockerId, ContextMenuItem, DragOutPayload, LoginItemOptions, PlatformServices,
    StagedUpdate, UpdateFeed, UpdateInfo, WidgetId,
};

use crate::host::{app_path_kind, menu_for_surface, power_save_kind};
use crate::{decode_value, DecodeError};

/// The shared error shape every function in this module returns —
/// collapses `quadraui::BackendError`'s three variants into the two a
/// binding's own exception taxonomy actually distinguishes
/// ([`Self::Unsupported`]: nothing implemented; [`Self::Failure`]: this
/// backend tried and it broke, or its render surface was lost), plus a
/// third, [`Self::InvalidArgument`], for a wire argument that never
/// reached a service call at all.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceError {
    /// This backend has no implementation for the seam called —
    /// `BackendError::Unsupported`.
    Unsupported,
    /// The backend attempted the call and it failed
    /// (`BackendError::PlatformFailure`), or its render surface was lost
    /// mid-call (`BackendError::SurfaceLost`) — collapsed to one message
    /// since no in-tree caller of this module needs to tell those two
    /// apart.
    Failure(String),
    /// A wire argument (an enum-name string, a `serde_json::Value`)
    /// failed to decode into the shape this function expects, caught
    /// before any [`PlatformServices`] call was made.
    InvalidArgument(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Unsupported => f.write_str("unsupported on this backend"),
            ServiceError::Failure(context) => write!(f, "platform failure: {context}"),
            ServiceError::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<BackendError> for ServiceError {
    fn from(err: BackendError) -> Self {
        match err {
            BackendError::Unsupported => ServiceError::Unsupported,
            BackendError::PlatformFailure { context } => ServiceError::Failure(context),
            BackendError::SurfaceLost => ServiceError::Failure("render surface lost".to_string()),
        }
    }
}

impl From<DecodeError> for ServiceError {
    fn from(err: DecodeError) -> Self {
        ServiceError::InvalidArgument(err.0)
    }
}

impl From<String> for ServiceError {
    fn from(err: String) -> Self {
        ServiceError::InvalidArgument(err)
    }
}

/// Mints the next globally unique handle for a registry in this module —
/// shared by [`staged_update_registry`]'s and [`power_blocker_registry`]'s
/// own counters, one `AtomicU64` each so a handle minted for one registry
/// can never collide with the other's.
fn next_handle(counter: &'static AtomicU64) -> u64 {
    counter.fetch_add(1, Ordering::Relaxed)
}

// ─── Auto-update ────────────────────────────────────────────────────────────

/// Wire-friendly mirror of [`quadraui::UpdateInfo`] — that type carries no
/// `Serialize`/`Deserialize` of its own (no consumer needed it before this
/// module), so [`update_check_for_update`] hands a binding this instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfoWire {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: String,
}

impl From<&UpdateInfo> for UpdateInfoWire {
    fn from(info: &UpdateInfo) -> Self {
        Self {
            version: info.version.clone(),
            notes: info.notes.clone(),
            url: info.url.clone(),
            signature: info.signature.clone(),
        }
    }
}

impl From<UpdateInfoWire> for UpdateInfo {
    fn from(wire: UpdateInfoWire) -> Self {
        UpdateInfo::new(wire.version, wire.notes, wire.url, wire.signature)
    }
}

/// Process-global registry of in-flight [`StagedUpdate`]s, keyed by the
/// opaque `u64` handle [`update_download_and_stage`] hands back — see this
/// module's own doc for why a handle, not the value itself, crosses the
/// wire.
fn staged_update_registry() -> &'static Mutex<HashMap<u64, StagedUpdate>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, StagedUpdate>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Query `feed_url` for a newer version — [`UpdateService::check_for_update`].
/// `Ok(None)` means reachable, nothing newer; `Ok(Some(_))` is the update.
pub fn update_check_for_update(
    services: &dyn PlatformServices,
    feed_url: &str,
) -> Result<Option<UpdateInfoWire>, ServiceError> {
    let feed = UpdateFeed::new(feed_url);
    let info = services.update_service().check_for_update(&feed)?;
    Ok(info.as_ref().map(UpdateInfoWire::from))
}

/// Download and stage `info` — [`UpdateService::download_and_stage`].
/// Returns an opaque handle for a later [`update_apply_on_quit`] call,
/// rather than the (non-wire) [`StagedUpdate`] itself.
pub fn update_download_and_stage(
    services: &dyn PlatformServices,
    info: UpdateInfoWire,
) -> Result<u64, ServiceError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let info: UpdateInfo = info.into();
    let staged = services.update_service().download_and_stage(&info)?;
    let handle = next_handle(&COUNTER);
    staged_update_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(handle, staged);
    Ok(handle)
}

/// Install the update staged under `handle` — [`UpdateService::apply_on_quit`].
/// `handle` is consumed: a second call with the same value reports
/// [`ServiceError::InvalidArgument`], the same "already used" story a
/// one-shot token would carry.
pub fn update_apply_on_quit(
    services: &dyn PlatformServices,
    handle: u64,
) -> Result<(), ServiceError> {
    let staged = staged_update_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&handle)
        .ok_or_else(|| {
            ServiceError::InvalidArgument(format!(
                "update_apply_on_quit: unknown or already-applied handle {handle}"
            ))
        })?;
    services.update_service().apply_on_quit(staged)?;
    Ok(())
}

// ─── Power save blocker ─────────────────────────────────────────────────────

/// Process-global registry of in-flight sleep/suspend inhibitors, keyed by
/// the opaque `u64` handle [`begin_power_save_block`] hands back —
/// [`BlockerId`] itself carries no public accessor for its inner `u64` (by
/// design — see that type's own doc), so this registry is the only way a
/// wire handle can later be exchanged back for the real id
/// [`end_power_save_block`] needs.
fn power_blocker_registry() -> &'static Mutex<HashMap<u64, BlockerId>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, BlockerId>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start a sleep/suspend inhibitor of the kind named by `kind` (decoded via
/// [`crate::host::power_save_kind`]) — [`PlatformServices::begin_power_save_block`].
/// Returns an opaque handle for a later [`end_power_save_block`] call.
pub fn begin_power_save_block(
    services: &dyn PlatformServices,
    kind: &str,
) -> Result<u64, ServiceError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let kind = power_save_kind(kind)?;
    let blocker = services.begin_power_save_block(kind)?;
    let handle = next_handle(&COUNTER);
    power_blocker_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(handle, blocker);
    Ok(handle)
}

/// End the sleep/suspend inhibitor started under `handle` —
/// [`PlatformServices::end_power_save_block`]. `handle` is consumed, the
/// same one-shot story [`update_apply_on_quit`]'s doc describes.
pub fn end_power_save_block(
    services: &dyn PlatformServices,
    handle: u64,
) -> Result<(), ServiceError> {
    let blocker = power_blocker_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&handle)
        .ok_or_else(|| {
            ServiceError::InvalidArgument(format!(
                "end_power_save_block: unknown or already-ended handle {handle}"
            ))
        })?;
    services.end_power_save_block(blocker)?;
    Ok(())
}

// ─── App standard paths ─────────────────────────────────────────────────────

/// Resolve the standard, per-app filesystem location named by `kind`
/// (decoded via [`crate::host::app_path_kind`]) — [`quadraui::paths::resolve`].
/// A free function, like `paths::resolve` itself: no live app or
/// [`PlatformServices`] is needed.
pub fn resolve_app_path(kind: &str) -> Result<PathBuf, ServiceError> {
    let kind = app_path_kind(kind)?;
    Ok(quadraui::paths::resolve(kind)?)
}

/// Set the app identifier scoping [`resolve_app_path`]'s app-scoped kinds
/// — [`quadraui::paths::set_app_identifier`].
pub fn set_app_identifier(identifier: &str) {
    quadraui::paths::set_app_identifier(identifier);
}

/// The current app identifier — [`quadraui::paths::app_identifier`].
pub fn app_identifier() -> String {
    quadraui::paths::app_identifier()
}

// ─── Launch at login ────────────────────────────────────────────────────────

/// Register (`enabled: true`) or unregister (`enabled: false`) this app to
/// launch at login — [`PlatformServices::set_login_item`]. `open_hidden`/
/// `args` build the [`LoginItemOptions`] `opts` argument; both are ignored
/// by every real implementation when `enabled` is `false` (see that
/// method's own doc).
pub fn set_login_item(
    services: &dyn PlatformServices,
    enabled: bool,
    open_hidden: bool,
    args: Vec<String>,
) -> Result<(), ServiceError> {
    let opts = LoginItemOptions::new()
        .with_open_hidden(open_hidden)
        .with_args(args);
    services.set_login_item(enabled, opts)?;
    Ok(())
}

/// Whether this app is currently registered to launch at login —
/// [`PlatformServices::login_item_enabled`].
pub fn login_item_enabled(services: &dyn PlatformServices) -> Result<bool, ServiceError> {
    Ok(services.login_item_enabled()?)
}

// ─── Protocol registration ──────────────────────────────────────────────────

/// Register this app as the OS-level default handler for `scheme` —
/// [`PlatformServices::set_as_default_protocol_client`].
pub fn set_as_default_protocol_client(
    services: &dyn PlatformServices,
    scheme: &str,
) -> Result<(), ServiceError> {
    Ok(services.set_as_default_protocol_client(scheme)?)
}

/// Whether this app is currently the OS-level default handler for `scheme`
/// — [`PlatformServices::is_default_protocol_client`].
pub fn is_default_protocol_client(
    services: &dyn PlatformServices,
    scheme: &str,
) -> Result<bool, ServiceError> {
    Ok(services.is_default_protocol_client(scheme)?)
}

/// Unregister this app as the OS-level default handler for `scheme` —
/// [`PlatformServices::remove_as_default_protocol_client`].
pub fn remove_as_default_protocol_client(
    services: &dyn PlatformServices,
    scheme: &str,
) -> Result<(), ServiceError> {
    Ok(services.remove_as_default_protocol_client(scheme)?)
}

// ─── Crash reporter ─────────────────────────────────────────────────────────

/// Decode `cfg` into a [`quadraui::crash::CrashReporterConfig`] and install
/// the panic hook — [`quadraui::crash::install_crash_handler`]. A free
/// function, like that one: no live app or [`PlatformServices`] is needed.
pub fn install_crash_handler(cfg: serde_json::Value) -> Result<(), ServiceError> {
    let cfg =
        decode_value::<quadraui::crash::CrashReporterConfig, _>("install_crash_handler", cfg)?;
    quadraui::crash::install_crash_handler(cfg)?;
    Ok(())
}

// ─── Drag-out ────────────────────────────────────────────────────────────────

/// Start an OS-level drag of `files`/`text` out of this app's window —
/// [`PlatformServices::begin_drag_out`]. At least one of `files`/`text`
/// must be given; an empty payload is rejected the same way
/// [`PlatformServices::begin_drag_out`]'s own doc pins for every backend.
pub fn begin_drag_out(
    services: &dyn PlatformServices,
    files: Vec<String>,
    text: Option<String>,
) -> Result<(), ServiceError> {
    let mut payload = DragOutPayload::new().with_files(files.into_iter().map(PathBuf::from));
    if let Some(text) = text {
        payload = payload.with_text(text);
    }
    Ok(services.begin_drag_out(payload)?)
}

// ─── Recent docs / dock menu / jump list ───────────────────────────────────

/// Add `path` to the OS "recent documents" list —
/// [`PlatformServices::add_recent_document`].
pub fn add_recent_document(
    services: &dyn PlatformServices,
    path: &str,
) -> Result<(), ServiceError> {
    Ok(services.add_recent_document(Path::new(path))?)
}

/// Clear the OS "recent documents" list —
/// [`PlatformServices::clear_recent_documents`].
pub fn clear_recent_documents(services: &dyn PlatformServices) -> Result<(), ServiceError> {
    Ok(services.clear_recent_documents()?)
}

/// Attach (or replace) this app's Dock icon menu, decoding `items` into
/// `Vec<`[`ContextMenuItem`]`>` and building the
/// [`quadraui::ContextMenu`] via [`crate::host::menu_for_surface`] —
/// [`PlatformServices::set_dock_menu`].
pub fn set_dock_menu(
    services: &dyn PlatformServices,
    items: serde_json::Value,
) -> Result<(), ServiceError> {
    let items: Vec<ContextMenuItem> = decode_value("set_dock_menu", items)?;
    let menu = menu_for_surface(WidgetId::new("qui-dock-menu"), items);
    Ok(services.set_dock_menu(&menu)?)
}

/// Set this app's Windows taskbar jump list, decoding `items` into
/// `Vec<`[`JumpListItem`]`>` — [`PlatformServices::set_jump_list`].
pub fn set_jump_list(
    services: &dyn PlatformServices,
    items: serde_json::Value,
) -> Result<(), ServiceError> {
    let items: Vec<JumpListItem> = decode_value("set_jump_list", items)?;
    Ok(services.set_jump_list(&items)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::backend::{
        Clipboard, FileDialogOptions, MessageDialogChoice, MessageDialogOptions, Notification,
    };

    /// Serialises this module's [`install_crash_handler`] tests: that
    /// function mutates the process-global panic hook, and two tests
    /// installing one concurrently would each risk chaining onto (or
    /// clobbering) the other's — the same hazard `quadraui::crash`'s own
    /// `#[cfg(test)]` module guards against, for the same reason.
    fn crash_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Default)]
    struct FakeClipboard;

    impl Clipboard for FakeClipboard {
        fn read_text(&self) -> Option<String> {
            None
        }
        fn write_text(&self, _text: &str) {}
    }

    #[derive(Default)]
    struct FakePlatformServices {
        clipboard: FakeClipboard,
    }

    impl PlatformServices for FakePlatformServices {
        fn clipboard(&self) -> &dyn Clipboard {
            &self.clipboard
        }

        fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }

        fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }

        fn show_folder_open_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }

        fn show_message_dialog(&self, _opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
            None
        }

        fn send_notification(&self, _n: Notification) {}

        fn open_url(&self, _url: &str) {}

        fn platform_name(&self) -> &'static str {
            "fake"
        }
    }

    // ── Auto-update ─────────────────────────────────────────────────────

    #[test]
    fn update_check_for_update_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            update_check_for_update(&services, "https://example.test/appcast.xml"),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn update_download_and_stage_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        let info = UpdateInfoWire {
            version: "1.0.0".to_string(),
            notes: String::new(),
            url: "https://example.test/v1".to_string(),
            signature: "sig".to_string(),
        };
        assert_eq!(
            update_download_and_stage(&services, info),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn update_apply_on_quit_rejects_an_unknown_handle() {
        let services = FakePlatformServices::default();
        assert_eq!(
            update_apply_on_quit(&services, u64::MAX),
            Err(ServiceError::InvalidArgument(
                "update_apply_on_quit: unknown or already-applied handle 18446744073709551615"
                    .to_string()
            ))
        );
    }

    // ── Power save blocker ──────────────────────────────────────────────

    #[test]
    fn begin_power_save_block_decodes_a_known_kind_and_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            begin_power_save_block(&services, "prevent_display_sleep"),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn begin_power_save_block_rejects_an_unknown_kind_without_calling_the_service() {
        let services = FakePlatformServices::default();
        let err = begin_power_save_block(&services, "not_a_real_kind").unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }

    #[test]
    fn end_power_save_block_rejects_an_unknown_handle() {
        let services = FakePlatformServices::default();
        let err = end_power_save_block(&services, 42).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }

    // ── App standard paths ──────────────────────────────────────────────

    #[test]
    fn resolve_app_path_round_trips_a_real_kind_to_a_non_empty_path() {
        let path = resolve_app_path("temp").unwrap();
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn resolve_app_path_rejects_an_unknown_kind() {
        let err = resolve_app_path("not_a_real_kind").unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }

    #[test]
    fn set_app_identifier_and_app_identifier_round_trip() {
        set_app_identifier("qbc-services-test-identifier");
        assert_eq!(app_identifier(), "qbc-services-test-identifier");
        quadraui::paths::clear_app_identifier();
    }

    // ── Launch at login ──────────────────────────────────────────────────

    #[test]
    fn set_login_item_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            set_login_item(&services, true, false, Vec::new()),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn login_item_enabled_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            login_item_enabled(&services),
            Err(ServiceError::Unsupported)
        );
    }

    // ── Protocol registration ───────────────────────────────────────────

    #[test]
    fn protocol_client_methods_default_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            set_as_default_protocol_client(&services, "myapp"),
            Err(ServiceError::Unsupported)
        );
        assert_eq!(
            is_default_protocol_client(&services, "myapp"),
            Err(ServiceError::Unsupported)
        );
        assert_eq!(
            remove_as_default_protocol_client(&services, "myapp"),
            Err(ServiceError::Unsupported)
        );
    }

    // ── Crash reporter ───────────────────────────────────────────────────

    #[test]
    fn install_crash_handler_round_trips_with_an_explicit_log_dir() {
        let _g = crash_guard();
        let dir =
            std::env::temp_dir().join(format!("qbc-services-crash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let previous = std::panic::take_hook();
        let cfg = serde_json::json!({ "log_dir": dir.to_string_lossy() });
        let result = install_crash_handler(cfg);
        std::panic::set_hook(previous);
        assert_eq!(result, Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_crash_handler_rejects_a_malformed_config() {
        let err = install_crash_handler(serde_json::json!("not an object")).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }

    // ── Drag-out ──────────────────────────────────────────────────────────

    #[test]
    fn begin_drag_out_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            begin_drag_out(&services, vec!["/tmp/a.txt".to_string()], None),
            Err(ServiceError::Unsupported)
        );
    }

    // ── Recent docs / dock menu / jump list ─────────────────────────────

    #[test]
    fn add_recent_document_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            add_recent_document(&services, "/tmp/a.txt"),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn clear_recent_documents_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        assert_eq!(
            clear_recent_documents(&services),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn set_dock_menu_decodes_items_and_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        let items = serde_json::json!([{ "label": { "spans": [] } }]);
        assert_eq!(
            set_dock_menu(&services, items),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn set_dock_menu_rejects_a_malformed_items_value() {
        let services = FakePlatformServices::default();
        let err = set_dock_menu(&services, serde_json::json!("not a list")).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }

    #[test]
    fn set_jump_list_decodes_items_and_defaults_to_unsupported() {
        let services = FakePlatformServices::default();
        let items = serde_json::json!([{ "title": "Recent file" }]);
        assert_eq!(
            set_jump_list(&services, items),
            Err(ServiceError::Unsupported)
        );
    }

    #[test]
    fn set_jump_list_rejects_a_malformed_items_value() {
        let services = FakePlatformServices::default();
        let err = set_jump_list(&services, serde_json::json!("not a list")).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidArgument(_)));
    }
}
