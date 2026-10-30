//! Tray / status-bar icon control for the GTK backend.
//!
//! GTK4 itself has no tray API — unlike `macos::tray`'s `NSStatusItem`
//! or `win::tray`'s `Shell_NotifyIconW`, Linux desktops standardised on
//! a D-Bus protocol instead: a tray-resident app registers a
//! `org.kde.StatusNotifierItem` object with whichever
//! `org.kde.StatusNotifierWatcher` the desktop shell is running (KDE's
//! own, GNOME's via an extension, Sway's via waybar, ...), and exposes
//! its context menu through a sibling `com.canonical.dbusmenu` object.
//! This module implements both sides on the session bus via `zbus`.
//!
//! ## Honest degrade
//!
//! [`GtkTrayState::ensure_connected`] is the single place that decides
//! whether a tray is possible right now: it tries to reach the session
//! bus and register with a watcher, and reports failure (no permanent
//! caching) on any error — unreachable session bus, no watcher running,
//! or the `RegisterStatusNotifierItem` call itself failing.
//! `GtkBackend::tray` calls this on every invocation, so a desktop that
//! gains a watcher after startup (an extension enabled mid-session) is
//! eventually picked up rather than being permanently refused. "Every
//! invocation" does not mean "every invocation blocks on the bus",
//! though: once a connection attempt has failed, further attempts are
//! throttled to once per [`RECONNECT_INTERVAL`] so a caller that polls
//! `tray()` every event-loop iteration on a desktop with no watcher at
//! all (stock GNOME, most CI runners) doesn't pay a blocking D-Bus
//! round trip on every single one of those iterations. Each attempt
//! that does go to the bus is itself bounded to [`DBUS_CALL_TIMEOUT`],
//! so a reachable-but-unresponsive bus address degrades to the same
//! honest `false` as an unreachable one instead of hanging the caller.
//! A successful [`linux::connect`] also starts a background thread that
//! watches the watcher's own bus-name ownership and re-runs
//! `RegisterStatusNotifierItem` whenever a new owner takes it, so a
//! watcher restart (plasmashell crashing/restarting, the GNOME
//! AppIndicator extension being toggled off and back on) is recovered
//! from instead of leaving the item permanently gone while
//! `set_icon`/`set_menu` keep reporting `Ok` — see
//! [`linux::spawn_watcher_restart_recovery`].
//!
//! ## Crossing to the GTK main thread
//!
//! `zbus::blocking::Connection` dispatches incoming method calls on its
//! own background thread, not GTK's main thread — so a `Clicked`/
//! `MenuItemActivated` arriving there cannot push directly into
//! `GtkBackend`'s single-threaded `events` queue. Instead it goes
//! through the exact same cross-thread seam `Backend::waker` already
//! hands out for `crate::runtime`'s user-event wake-ups: the private
//! [`TrayWake`] payload is wrapped in a [`UserPayload`] and pushed
//! through that `Arc<dyn Fn(UserPayload) + Send + Sync>` closure, then
//! [`drain_tray_wake`] — called from `GtkBackend::poll_events`/
//! `wait_events` right after draining the user-event queue — rewrites
//! any `UiEvent::User` carrying a `TrayWake` back into the real
//! `UiEvent::TrayClicked`/`UiEvent::ContextMenuItemActivated` the rest
//! of the app expects, so nothing outside this module and
//! `gtk::backend` ever sees the intermediate payload.
//!
//! ## Menu conversion
//!
//! [`to_dbusmenu_properties`] flattens a [`ContextMenu`] into the
//! `(u32, Vec<(i32, HashMap<String, OwnedValue>)>)` child list
//! `com.canonical.dbusmenu`'s `GetLayout` returns. Nested
//! [`ContextMenuItem::submenu`]s are flattened to one level — a
//! `submenu` entry becomes a non-activatable (but still visible, still
//! separator-free) label row followed immediately by its children at
//! the same depth — rather than recursing through nested `Variant`s,
//! since no quadraui consumer currently builds more than one level of
//! tray menu and dbusmenu's own recursive-through-`Variant` shape earns
//! the extra complexity only once one does.
//!
//! ## Live updates
//!
//! A `TrayService::set_icon`/`set_tooltip`/`set_menu` call takes effect
//! immediately, not just on the next property read: each one also
//! emits the matching SNI change signal (`NewIcon`/`NewToolTip`) or, for
//! the menu, bumps [`TrayShared::menu_revision`] and emits dbusmenu's
//! `LayoutUpdated` so a host that cached an earlier `GetLayout` knows to
//! re-fetch it. `Menu` itself is advertised unconditionally as
//! `/MenuBar` — see [`linux::StatusNotifierItemIface::menu`] — since SNI
//! has no `NewMenu` signal a host could react to if the path changed
//! later. `Status` starts at `"Passive"` and only [`GtkTrayState::set_icon`]'s
//! first call flips it to `"Active"`, with a one-time `NewStatus` emit —
//! see [`TrayShared::active`]'s doc for why a capability probe that
//! never calls `set_icon` must not show a permanent blank item.
//! `set_menu` also emits a `PropertiesChanged` refresh for `ItemIsMenu`,
//! which [`linux::StatusNotifierItemIface::item_is_menu`] always answers
//! live from [`TrayShared::has_menu`] — a host (KDE Plasma, waybar) that
//! read `false` before a menu was attached needs telling to re-fetch it,
//! the same reason the dbusmenu `LayoutUpdated` emit exists.
//!
//! ## Teardown
//!
//! Nothing in this module ever unregisters the `StatusNotifierItem` or
//! closes the D-Bus connection on its own; the only way the tray item
//! goes away is `GtkBackend` (and with it, `GtkTrayState`) being
//! dropped, which drops `zbus::blocking::Connection` and lets the bus
//! itself notice the peer disappeared. That is intentional — there is
//! no `TrayService::remove`/`hide` in the trait for any backend to call.

use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use crate::backend::{BackendError, ServiceResult};
use crate::event::{MouseButton, UiEvent, UserPayload};
use crate::primitives::context_menu::{ContextMenu, ContextMenuItem};
use crate::primitives::image::ImageSource;
use crate::types::WidgetId;

/// The cross-thread marker [`UserPayload`] a D-Bus method call wraps
/// before handing it to the waker closure — see this module's doc for
/// why this detour through `UiEvent::User` is necessary at all.
/// `Copy` because both variants' payloads are, which lets
/// [`drain_tray_wake`] `downcast_ref` + copy instead of needing to
/// consume the original event to extract it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) enum TrayWake {
    Clicked(MouseButton),
    MenuItemActivated(WidgetId),
}

/// Rewrites any `UiEvent::User` wrapping a [`TrayWake`] back into the
/// real tray event it stands in for. Called from `GtkBackend::
/// poll_events`/`wait_events` right after `UserEventQueue::drain_into`
/// so every other call site keeps treating `UiEvent::User` as meaning
/// exactly what `crate::runtime`'s own wake-ups mean.
pub(crate) fn drain_tray_wake(events: &mut [UiEvent]) {
    for event in events.iter_mut() {
        let wake = match event {
            UiEvent::User(payload) => payload.downcast_ref::<TrayWake>().copied(),
            _ => None,
        };
        if let Some(wake) = wake {
            *event = match wake {
                TrayWake::Clicked(button) => UiEvent::TrayClicked { button },
                TrayWake::MenuItemActivated(id) => UiEvent::ContextMenuItemActivated(id),
            };
        }
    }
}

/// State shared between the `GtkBackend`-owned side of this module and
/// the D-Bus interface objects running on `zbus`'s background thread —
/// every field here is read or written from both.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Default)]
struct TrayShared {
    tooltip: String,
    /// Raw ARGB32 pixmaps in the `(width, height, bytes)` shape
    /// `org.kde.StatusNotifierItem`'s `IconPixmap` property expects:
    /// network byte order (big-endian), 4 bytes per pixel, alpha first.
    icon: Vec<(i32, i32, Vec<u8>)>,
    menu: Option<ContextMenu>,
    /// Bumped by every `set_menu` call and returned as `GetLayout`'s
    /// revision number, so a host that caches the layout it last fetched
    /// knows — once it also sees `LayoutUpdated` carrying this value —
    /// that a re-fetch would return something different.
    menu_revision: u32,
    /// Whether [`GtkTrayState::set_icon`] has ever been called. Backs the
    /// `Status` property: `false` reports `"Passive"`, matching
    /// `TrayService::set_icon`'s contract that a host which never calls
    /// `set_icon` shows nothing, the same lazy-creation behaviour
    /// `macos::tray`'s `NSStatusItem` gets for free. There is no way back
    /// to `false` — once an icon has been shown there is no
    /// `TrayService::remove`/`hide` to revert it (see this module's
    /// "Teardown" doc section).
    active: bool,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl TrayShared {
    fn has_menu(&self) -> bool {
        self.menu.is_some()
    }
}

/// Floor on how often [`GtkTrayState::ensure_connected`] re-attempts a
/// failed connection — see this module's "Honest degrade" doc section.
/// Deliberately short: this is a jank guard against polling every
/// event-loop iteration, not a real backoff, so a watcher that appears
/// mid-session is still picked up within a couple of seconds.
#[cfg(target_os = "linux")]
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

/// Ceiling on a single D-Bus method call made while probing for or
/// registering with a watcher (`linux::connect`, `tests::
/// watcher_is_reachable`). `zbus::blocking::Connection::session()`'s
/// shortcut constructor leaves this unbounded, so a stale or
/// half-alive bus address (a socket that accepts a connection but
/// never answers, which this module has hit on sandboxed/containerized
/// hosts) would otherwise hang the caller indefinitely rather than
/// reporting the honest `false` this module promises. Short enough that
/// a hung peer doesn't visibly stall `GtkBackend::tray`/the test suite,
/// long enough that it doesn't misreport a merely slow — but live —
/// watcher as absent.
#[cfg(target_os = "linux")]
const DBUS_CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-[`super::backend::GtkBackend`] tray state. `connection` is
/// `None` until [`Self::ensure_connected`] first succeeds; on Linux
/// without a reachable `org.kde.StatusNotifierWatcher` it stays `None`
/// forever, and `GtkBackend::tray` reports that honestly as `None`
/// rather than caching a false "connected".
#[derive(Default)]
pub(crate) struct GtkTrayState {
    #[cfg(target_os = "linux")]
    connection: Option<zbus::blocking::Connection>,
    /// When the last *failed* connection attempt happened, so
    /// [`Self::ensure_connected`] can throttle retries to
    /// [`RECONNECT_INTERVAL`] instead of hitting the session bus on
    /// every call. `None` means "never attempted, or last attempt
    /// succeeded" — there is nothing to throttle in either case.
    #[cfg(target_os = "linux")]
    last_failed_attempt: Option<Instant>,
    shared: Arc<Mutex<TrayShared>>,
}

impl GtkTrayState {
    /// Attempts to reach the session bus and register a
    /// `StatusNotifierItem`/`dbusmenu` pair with whatever watcher is
    /// running, if not already connected. Returns whether a tray is
    /// usable right now. Never caches a failure *permanently* — see
    /// this module's doc for why a later call after this one may still
    /// succeed — but does throttle how often a failure is retried; see
    /// [`RECONNECT_INTERVAL`].
    pub(crate) fn ensure_connected(
        &mut self,
        waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    ) -> bool {
        #[cfg(target_os = "linux")]
        {
            if self.connection.is_some() {
                return true;
            }
            if let Some(last) = self.last_failed_attempt {
                if last.elapsed() < RECONNECT_INTERVAL {
                    return false;
                }
            }
            match connect(Arc::clone(&self.shared), waker) {
                Ok(conn) => {
                    self.connection = Some(conn);
                    self.last_failed_attempt = None;
                    true
                }
                Err(_) => {
                    self.last_failed_attempt = Some(Instant::now());
                    false
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = waker;
            false
        }
    }

    pub(crate) fn set_icon(&mut self, icon: ImageSource) -> ServiceResult<()> {
        self.require_connected()?;
        let pixmap = decode_icon(&icon).ok_or_else(|| BackendError::PlatformFailure {
            context: "GTK tray: failed to decode icon".to_string(),
        })?;
        let became_active = {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.icon = vec![pixmap];
            let was_active = shared.active;
            shared.active = true;
            !was_active
        };
        self.emit_item_signal("NewIcon");
        // Only the first `set_icon` flips `Status` from `Passive` to
        // `Active` — see `TrayShared::active`'s doc — so this only fires
        // once per item, matching the one-shot lazy-creation contract
        // rather than re-announcing `Active` on every icon change.
        if became_active {
            self.emit_new_status("Active");
        }
        Ok(())
    }

    pub(crate) fn set_tooltip(&mut self, tooltip: &str) -> ServiceResult<()> {
        self.require_connected()?;
        {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.tooltip = tooltip.to_string();
        }
        self.emit_item_signal("NewToolTip");
        self.emit_item_signal("NewTitle");
        Ok(())
    }

    pub(crate) fn set_menu(&mut self, menu: &ContextMenu) -> ServiceResult<()> {
        self.require_connected()?;
        let revision = {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            // An empty `ContextMenu` detaches, same as every other
            // `TrayService::set_menu` implementation — see that trait's
            // own doc.
            shared.menu = if menu.items.is_empty() {
                None
            } else {
                Some(menu.clone())
            };
            shared.menu_revision = shared.menu_revision.wrapping_add(1);
            shared.menu_revision
        };
        self.emit_layout_updated(revision);
        // `ItemIsMenu` tracks `TrayShared::has_menu` live (see
        // `linux::StatusNotifierItemIface::item_is_menu`), so a host that
        // already read it once — e.g. at registration, before this menu
        // was attached — needs telling to re-fetch it, the same reason
        // `emit_item_signal`/`emit_layout_updated` exist for the other
        // properties this module can change after registration.
        self.emit_item_is_menu_changed();
        Ok(())
    }

    fn require_connected(&self) -> ServiceResult<()> {
        #[cfg(target_os = "linux")]
        {
            if self.connection.is_some() {
                return Ok(());
            }
        }
        Err(BackendError::Unsupported)
    }

    /// Emits a no-argument `org.kde.StatusNotifierItem` change signal
    /// (`NewIcon`, `NewToolTip`, `NewTitle`, ...) telling a listening
    /// host to re-fetch the property it names, rather than relying on
    /// it polling or happening to `GetAll` again. Best-effort: a failed
    /// emit (connection dropped mid-call) is not surfaced as an error
    /// since the property itself is already updated and the next
    /// successful `Get`/`GetAll` will see it regardless.
    #[cfg(target_os = "linux")]
    fn emit_item_signal(&self, member: &str) {
        if let Some(connection) = &self.connection {
            let _ = connection.emit_signal(
                Option::<zbus::names::BusName<'_>>::None,
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
                member,
                &(),
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn emit_item_signal(&self, _member: &str) {}

    /// Emits `org.kde.StatusNotifierItem`'s `NewStatus(STRING status)` —
    /// unlike `NewIcon`/`NewToolTip`/`NewTitle` this signal carries the
    /// new value itself rather than telling the host to re-fetch it, per
    /// the SNI spec, so it cannot reuse [`Self::emit_item_signal`].
    #[cfg(target_os = "linux")]
    fn emit_new_status(&self, status: &str) {
        if let Some(connection) = &self.connection {
            let _ = connection.emit_signal(
                Option::<zbus::names::BusName<'_>>::None,
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
                "NewStatus",
                &(status,),
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn emit_new_status(&self, _status: &str) {}

    /// Emits the standard `org.freedesktop.DBus.Properties.
    /// PropertiesChanged` signal for `ItemIsMenu`, telling a host that
    /// already read the property once that it has changed — see
    /// [`Self::set_menu`]'s call site for why this, and not `NewStatus`'s
    /// dedicated-signal shape, is the right tool here: `ItemIsMenu` is a
    /// plain property with no SNI-specific change signal of its own.
    #[cfg(target_os = "linux")]
    fn emit_item_is_menu_changed(&self) {
        if let Some(connection) = &self.connection {
            let is_menu = self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .has_menu();
            let changed: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
                std::collections::HashMap::from([(
                    "ItemIsMenu",
                    zbus::zvariant::Value::from(is_menu),
                )]);
            let invalidated: Vec<&str> = Vec::new();
            let _ = connection.emit_signal(
                Option::<zbus::names::BusName<'_>>::None,
                "/StatusNotifierItem",
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &("org.kde.StatusNotifierItem", changed, invalidated),
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn emit_item_is_menu_changed(&self) {}

    /// Emits `com.canonical.dbusmenu`'s `LayoutUpdated(revision, parent)`
    /// for the root menu (`parent == 0`) so a host that cached the
    /// layout from an earlier `GetLayout` knows to call it again.
    #[cfg(target_os = "linux")]
    fn emit_layout_updated(&self, revision: u32) {
        if let Some(connection) = &self.connection {
            let _ = connection.emit_signal(
                Option::<zbus::names::BusName<'_>>::None,
                "/MenuBar",
                "com.canonical.dbusmenu",
                "LayoutUpdated",
                &(revision, 0_i32),
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn emit_layout_updated(&self, _revision: u32) {}
}

/// Flattens an `ImageSource` to a single ARGB32 pixmap for
/// `IconPixmap`. Returns `None` on anything that fails to decode — a
/// missing path, unreadable file, or a format `gdk-pixbuf` can't parse
/// — which [`GtkTrayState::set_icon`] turns into
/// [`BackendError::PlatformFailure`] rather than silently leaving the
/// icon blank, matching `macos::tray::set_icon`/`win::tray::set_icon`
/// and [`crate::backend::TrayService::set_icon`]'s own doc.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn decode_icon(source: &ImageSource) -> Option<(i32, i32, Vec<u8>)> {
    let bytes = match source {
        ImageSource::Bytes(bytes) => bytes.clone(),
        ImageSource::Path(path) => std::fs::read(path).ok()?,
    };
    decode_argb32(&bytes)
}

/// Decodes raw image bytes into `IconPixmap`'s wire shape without a
/// decoder dependency of its own: GTK already links `gdk-pixbuf`
/// (`super::image::load_pixbuf` uses it for every other image
/// primitive), so this reuses that rather than adding a second decode
/// path for the one pixel format this module needs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn decode_argb32(bytes: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
    #[cfg(target_os = "linux")]
    {
        use gtk4::gdk_pixbuf::Pixbuf;
        use gtk4::gio::MemoryInputStream;
        use gtk4::glib::Bytes as GBytes;

        let gbytes = GBytes::from(bytes);
        let stream = MemoryInputStream::from_bytes(&gbytes);
        let pixbuf = Pixbuf::from_stream(&stream, None::<&gtk4::gio::Cancellable>).ok()?;
        let width = pixbuf.width();
        let height = pixbuf.height();
        let has_alpha = pixbuf.has_alpha();
        let n_channels: i64 = if has_alpha { 4 } else { 3 };
        let rowstride = pixbuf.rowstride();
        let src = pixbuf.pixel_bytes()?;
        // Widen every dimension to `i64` before multiplying: GTK's own
        // `i32` width/height/rowstride can overflow a 32-bit multiply on
        // a sufficiently large decoded pixbuf, which panics in debug
        // builds instead of just wrapping.
        let width_i64 = i64::from(width);
        let height_i64 = i64::from(height);
        let rowstride_i64 = i64::from(rowstride);
        let capacity = usize::try_from(width_i64 * height_i64 * 4).unwrap_or(0);
        let mut out = Vec::with_capacity(capacity);
        for y in 0..height_i64 {
            let row_start = usize::try_from(y * rowstride_i64).ok()?;
            for x in 0..width_i64 {
                let px = row_start + usize::try_from(x * n_channels).ok()?;
                let (r, g, b, a) = if has_alpha {
                    (src[px], src[px + 1], src[px + 2], src[px + 3])
                } else {
                    (src[px], src[px + 1], src[px + 2], 255)
                };
                out.extend_from_slice(&[a, r, g, b]);
            }
        }
        Some((width, height, out))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bytes;
        None
    }
}

/// Plain-text label for a dbusmenu row — `StyledText` carries per-span
/// colour/weight dbusmenu has no property for, so this collapses it to
/// its text content, the same simplification `ContextMenuItem::label`'s
/// non-GTK-specific call sites already accept.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn plain_label(item: &ContextMenuItem) -> String {
    item.label
        .spans
        .iter()
        .map(|span| span.text.as_str())
        .collect()
}

/// Flattens a [`ContextMenu`]'s items into dbusmenu's child-row shape:
/// one `(id, properties)` pair per item, in order, with nested
/// `submenu`s flattened in place rather than nested — see this module's
/// doc for why. `id` is the item's position in this flattened list
/// (`0` is reserved for the implicit root `GetLayout` always returns),
/// recovered by [`widget_id_for_dbus_id`] when a click arrives.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn flatten_items(items: &[ContextMenuItem], out: &mut Vec<ContextMenuItem>) {
    for item in items {
        out.push(item.clone());
        if let Some(submenu) = &item.submenu {
            flatten_items(submenu, out);
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn widget_id_for_dbus_id(flattened: &[ContextMenuItem], dbus_id: i32) -> Option<WidgetId> {
    let idx = usize::try_from(dbus_id).ok()?;
    flattened.get(idx.checked_sub(1)?).and_then(|item| item.id)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::fdo;
    use zbus::zvariant::{OwnedValue, Value};

    /// `org.kde.StatusNotifierItem`'s `ToolTip` property shape: icon name,
    /// icon pixmap data, title and description.
    type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

    /// `com.canonical.dbusmenu`'s `GetLayout` return shape: the revision
    /// number, paired with the root node `(id, properties, children)`.
    type DbusMenuLayout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

    /// Real `org.kde.StatusNotifierItem` object. Every property here is
    /// read fresh from `shared` on each D-Bus `Get`/`GetAll`, so a
    /// `TrayService::set_*` call takes effect on the next property read
    /// without this module needing to emit `PropertiesChanged` itself
    /// for the common case — hosts poll or re-fetch properties on
    /// `NewIcon`/`NewToolTip`, which is more than this first pass wires
    /// up; see the module-level scope note in the tray.rs module doc.
    pub(super) struct StatusNotifierItemIface {
        pub(super) shared: Arc<Mutex<TrayShared>>,
        pub(super) waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl StatusNotifierItemIface {
        #[zbus(property)]
        fn category(&self) -> String {
            "ApplicationStatus".to_string()
        }

        #[zbus(property)]
        fn id(&self) -> String {
            "quadraui".to_string()
        }

        #[zbus(property)]
        fn title(&self) -> String {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.tooltip.clone()
        }

        /// `"Passive"` until the first [`super::GtkTrayState::set_icon`]
        /// call, then `"Active"` — see `TrayShared::active`'s doc. A host
        /// that probes `Backend::tray()` without ever calling `set_icon`
        /// (a capability probe) must not show a permanent blank item;
        /// reporting `Passive` is how this module keeps that promise
        /// since GTK, unlike `macos::tray`, has no way to defer
        /// registering with the watcher at all.
        #[zbus(property)]
        fn status(&self) -> String {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if shared.active { "Active" } else { "Passive" }.to_string()
        }

        #[zbus(property)]
        fn icon_name(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.icon.clone()
        }

        #[zbus(property)]
        fn tool_tip(&self) -> ToolTip {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                String::new(),
                Vec::new(),
                shared.tooltip.clone(),
                String::new(),
            )
        }

        /// Always `/MenuBar`, whether or not a menu has been set yet.
        /// SNI has no `NewMenu` signal, so a host that read this
        /// property once at registration — before the first
        /// `TrayService::set_menu` call lands — would otherwise never
        /// learn a menu became available later; [`DbusMenuIface`] is
        /// already registered at that path unconditionally (see
        /// [`connect`]) and simply reports an empty `GetLayout` until a
        /// menu is set, so advertising the path early costs nothing.
        #[zbus(property)]
        fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
            zbus::zvariant::ObjectPath::try_from("/MenuBar")
                .expect("static path is valid")
                .into()
        }

        /// Mirrors [`TrayShared::has_menu`] live: `true` once
        /// [`super::GtkTrayState::set_menu`] has attached a non-empty
        /// menu. A host (KDE Plasma, waybar) that sees `true` opens the
        /// dbusmenu on left-click instead of calling `Activate` —
        /// hard-coding this `false` left those hosts with no way to open
        /// the menu and no `TrayClicked` either, since [`Self::
        /// push_click`] drops `Activate` whenever a menu is attached.
        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            self.shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .has_menu()
        }

        fn activate(&self, x: i32, y: i32) {
            let _ = (x, y);
            self.push_click(MouseButton::Left);
        }

        fn secondary_activate(&self, x: i32, y: i32) {
            let _ = (x, y);
            self.push_click(MouseButton::Right);
        }

        fn context_menu(&self, x: i32, y: i32) {
            let _ = (x, y);
            self.push_click(MouseButton::Right);
        }

        fn scroll(&self, delta: i32, orientation: String) {
            let _ = (delta, orientation);
        }
    }

    impl StatusNotifierItemIface {
        fn push_click(&self, button: MouseButton) {
            let has_menu = self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .has_menu();
            // `TrayClicked` only fires while no menu is attached — a
            // real SNI host opens the menu itself instead of calling
            // `Activate`/`SecondaryActivate` once a `Menu` property is
            // set, but this guard keeps that contract even against a
            // host that calls both.
            if !has_menu {
                (self.waker)(UserPayload::new(TrayWake::Clicked(button)));
            }
        }
    }

    /// Real `com.canonical.dbusmenu` object backing the `Menu` property
    /// above. `about_to_show` always reports `false` (no lazy submenu
    /// population — [`flatten_items`] already expanded everything
    /// up front).
    pub(super) struct DbusMenuIface {
        pub(super) shared: Arc<Mutex<TrayShared>>,
        pub(super) waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    }

    #[zbus::interface(name = "com.canonical.dbusmenu")]
    impl DbusMenuIface {
        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn text_direction(&self) -> String {
            "ltr".to_string()
        }

        #[zbus(property)]
        fn status(&self) -> String {
            "normal".to_string()
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> Vec<String> {
            Vec::new()
        }

        /// Ignores `parent_id`/`recursion_depth` and always returns the
        /// whole flattened tree rooted at `0`: deliberate, since every
        /// real dbusmenu host calls `GetLayout(0, -1, [])` on first
        /// fetch and this module never advertises more than the one
        /// level [`flatten_items`] already expands, so there is no
        /// partial subtree a host could usefully ask for instead.
        fn get_layout(
            &self,
            parent_id: i32,
            recursion_depth: i32,
            property_names: Vec<String>,
        ) -> fdo::Result<DbusMenuLayout> {
            let _ = (parent_id, recursion_depth, property_names);
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let items = match &shared.menu {
                Some(menu) => menu.items.clone(),
                None => Vec::new(),
            };
            let mut flattened = Vec::new();
            flatten_items(&items, &mut flattened);
            let children: fdo::Result<Vec<OwnedValue>> = flattened
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let props = item_properties(item);
                    let child: (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>) =
                        (idx as i32 + 1, props, Vec::new());
                    OwnedValue::try_from(Value::from(child))
                        .map_err(|e| fdo::Error::Failed(e.to_string()))
                })
                .collect();
            let root_props: HashMap<String, OwnedValue> = HashMap::from([(
                "children-display".to_string(),
                OwnedValue::try_from(Value::from("submenu")).expect("string converts"),
            )]);
            let revision = shared.menu_revision;
            Ok((revision, (0, root_props, children?)))
        }

        fn get_group_properties(
            &self,
            ids: Vec<i32>,
            property_names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            let _ = property_names;
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let items = match &shared.menu {
                Some(menu) => menu.items.clone(),
                None => Vec::new(),
            };
            let mut flattened = Vec::new();
            flatten_items(&items, &mut flattened);
            ids.into_iter()
                .filter_map(|id| {
                    let idx = usize::try_from(id).ok()?.checked_sub(1)?;
                    flattened.get(idx).map(|item| (id, item_properties(item)))
                })
                .collect()
        }

        fn about_to_show(&self, id: i32) -> bool {
            let _ = id;
            false
        }

        fn event(&self, id: i32, event_id: String, data: Value<'_>, timestamp: u32) {
            let _ = (data, timestamp);
            if event_id != "clicked" {
                return;
            }
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let items = match &shared.menu {
                Some(menu) => menu.items.clone(),
                None => return,
            };
            drop(shared);
            let mut flattened = Vec::new();
            flatten_items(&items, &mut flattened);
            if let Some(widget_id) = widget_id_for_dbus_id(&flattened, id) {
                (self.waker)(UserPayload::new(TrayWake::MenuItemActivated(widget_id)));
            }
        }
    }

    /// Builds one dbusmenu row's property map. A `submenu` parent row —
    /// already flattened in place by [`flatten_items`], immediately
    /// followed by its own children — is reported `enabled: false`
    /// regardless of `item.disabled`: [`flatten_items`] gives it no
    /// `about_to_show`/nested-`Variant` children of its own to open, so
    /// leaving it activatable would fire
    /// `UiEvent::ContextMenuItemActivated` for a row that is really just
    /// a group heading, diverging from `macos::tray`/`win::tray` where a
    /// submenu parent is never itself clickable.
    fn item_properties(item: &ContextMenuItem) -> HashMap<String, OwnedValue> {
        let mut props = HashMap::new();
        let kind = if item.is_separator() {
            "separator"
        } else {
            "standard"
        };
        props.insert(
            "type".to_string(),
            OwnedValue::try_from(Value::from(kind)).expect("string converts"),
        );
        if !item.is_separator() {
            props.insert(
                "label".to_string(),
                OwnedValue::try_from(Value::from(plain_label(item))).expect("string converts"),
            );
            let enabled = !item.disabled && item.submenu.is_none();
            props.insert(
                "enabled".to_string(),
                OwnedValue::try_from(Value::from(enabled)).expect("bool converts"),
            );
            if let Some(checked) = item.checked {
                props.insert(
                    "toggle-type".to_string(),
                    OwnedValue::try_from(Value::from("checkmark")).expect("string converts"),
                );
                props.insert(
                    "toggle-state".to_string(),
                    OwnedValue::try_from(Value::from(if checked { 1_i32 } else { 0_i32 }))
                        .expect("int converts"),
                );
            }
        }
        props
    }

    /// Registers both interfaces on a fresh session-bus connection and
    /// asks the watcher to pick the item up. Any failure here — no
    /// session bus, no watcher, registration refused, or a method call
    /// outliving [`super::DBUS_CALL_TIMEOUT`] — bubbles up as `Err`,
    /// which [`super::GtkTrayState::ensure_connected`] turns into an
    /// honest `false` rather than a half-registered connection or an
    /// indefinite hang.
    pub(super) fn connect(
        shared: Arc<Mutex<TrayShared>>,
        waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    ) -> zbus::Result<Connection> {
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(super::DBUS_CALL_TIMEOUT)
            .build()?;
        connection.object_server().at(
            "/StatusNotifierItem",
            StatusNotifierItemIface {
                shared: Arc::clone(&shared),
                waker: Arc::clone(&waker),
            },
        )?;
        connection.object_server().at(
            "/MenuBar",
            DbusMenuIface {
                shared: Arc::clone(&shared),
                waker: Arc::clone(&waker),
            },
        )?;
        let watcher = zbus::blocking::Proxy::new(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )?;
        let unique_name = connection
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        watcher.call_method("RegisterStatusNotifierItem", &(unique_name.clone(),))?;
        spawn_watcher_restart_recovery(watcher, unique_name);
        Ok(connection)
    }

    /// Watches `org.kde.StatusNotifierWatcher`'s own bus-name ownership
    /// on a dedicated background thread for as long as `watcher`'s
    /// connection stays alive, and re-runs `RegisterStatusNotifierItem`
    /// every time a new owner takes the name. A plasmashell restart, or
    /// toggling GNOME's AppIndicator extension off and back on, both
    /// show up this way — without this, `connect`'s one-time
    /// registration is only ever seen by the watcher instance that was
    /// running when it ran, so a later restart loses the icon for the
    /// rest of the process's life even though `set_icon`/`set_menu` keep
    /// reporting `Ok`. Best-effort: if `receive_owner_changed` itself
    /// fails to subscribe, this silently does nothing rather than
    /// failing `connect` over a recovery path that was never load-
    /// bearing for the initial registration.
    fn spawn_watcher_restart_recovery(
        watcher: zbus::blocking::Proxy<'static>,
        unique_name: String,
    ) {
        let Ok(owner_changes) = watcher.receive_owner_changed() else {
            return;
        };
        std::thread::spawn(move || {
            // Each item is `None` for an owner change that releases the
            // name (nothing to register with) and `Some` for a new
            // owner taking it — only the latter means a watcher is
            // there to register with again. The iterator itself stops
            // once the underlying connection is dropped, which is this
            // thread's own exit condition — there is nothing left to
            // recover for.
            for new_owner in owner_changes {
                if new_owner.is_some() {
                    let _ =
                        watcher.call_method("RegisterStatusNotifierItem", &(unique_name.clone(),));
                }
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::primitives::context_menu::ContextMenuPlacement;
        use crate::types::StyledText;
        use crate::ContextMenu;

        fn item(label: &str) -> ContextMenuItem {
            ContextMenuItem {
                id: Some(WidgetId::new(label)),
                label: StyledText::plain(label),
                detail: None,
                disabled: false,
                key_equivalent: None,
                checked: None,
                submenu: None,
            }
        }

        fn string_prop(props: &HashMap<String, OwnedValue>, key: &str) -> String {
            String::try_from(props.get(key).expect("property present").clone())
                .expect("string-valued property")
        }

        fn bool_prop(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
            bool::try_from(props.get(key).expect("property present").clone())
                .expect("bool-valued property")
        }

        #[test]
        fn item_properties_reports_separator_without_label_or_enabled() {
            let separator = ContextMenuItem {
                id: None,
                ..item("unused-label")
            };
            assert!(separator.is_separator());

            let props = item_properties(&separator);

            assert_eq!(string_prop(&props, "type"), "separator");
            assert!(!props.contains_key("label"));
            assert!(!props.contains_key("enabled"));
        }

        #[test]
        fn item_properties_reports_plain_item_label_and_enabled_state() {
            let enabled = item("Copy");
            let props = item_properties(&enabled);
            assert_eq!(string_prop(&props, "type"), "standard");
            assert_eq!(string_prop(&props, "label"), "Copy");
            assert!(bool_prop(&props, "enabled"));

            let mut disabled = item("Paste");
            disabled.disabled = true;
            let props = item_properties(&disabled);
            assert!(!bool_prop(&props, "enabled"));

            let mut checked = item("Word Wrap");
            checked.checked = Some(true);
            let props = item_properties(&checked);
            assert_eq!(string_prop(&props, "toggle-type"), "checkmark");
        }

        /// Directly covers the doc rationale on [`item_properties`]: a
        /// submenu-parent row reports `enabled: false` even when
        /// `item.disabled` is `false`, matching that it is a group
        /// heading rather than something a dbusmenu host should treat
        /// as clickable.
        #[test]
        fn item_properties_disables_a_submenu_parent_row_regardless_of_disabled_flag() {
            let mut parent = item("More");
            parent.disabled = false;
            parent.submenu = Some(vec![item("child")]);

            let props = item_properties(&parent);

            assert!(!bool_prop(&props, "enabled"));
        }

        fn shared_with_menu(menu: Option<ContextMenu>) -> Arc<Mutex<TrayShared>> {
            let mut shared = TrayShared::default();
            shared.menu = menu;
            Arc::new(Mutex::new(shared))
        }

        fn menu_bar(items: Vec<ContextMenuItem>) -> ContextMenu {
            ContextMenu {
                id: WidgetId::new("test:tray-menu"),
                items,
                selected_idx: usize::MAX,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
            }
        }

        #[test]
        fn get_layout_reflects_the_current_menu_and_revision() {
            let shared = shared_with_menu(Some(menu_bar(vec![item("one"), item("two")])));
            {
                let mut locked = shared.lock().unwrap_or_else(|e| e.into_inner());
                locked.menu_revision = 7;
            }
            let iface = DbusMenuIface {
                shared: Arc::clone(&shared),
                waker: Arc::new(|_| {}),
            };

            let (revision, (root_id, _root_props, children)) = iface
                .get_layout(0, -1, Vec::new())
                .expect("get_layout succeeds");

            assert_eq!(revision, 7);
            assert_eq!(root_id, 0);
            assert_eq!(children.len(), 2);
        }

        #[test]
        fn get_layout_is_empty_when_no_menu_is_set() {
            let shared = shared_with_menu(None);
            let iface = DbusMenuIface {
                shared,
                waker: Arc::new(|_| {}),
            };

            let (_revision, (_root_id, _root_props, children)) = iface
                .get_layout(0, -1, Vec::new())
                .expect("get_layout succeeds");

            assert!(children.is_empty());
        }

        #[test]
        fn status_notifier_item_menu_property_is_always_menu_bar() {
            let iface = StatusNotifierItemIface {
                shared: shared_with_menu(None),
                waker: Arc::new(|_| {}),
            };

            assert_eq!(iface.menu().as_str(), "/MenuBar");
        }

        /// A bare capability probe (`backend.tray()` with `set_icon`
        /// never called) must report `Passive`, not `Active` — see
        /// `TrayShared::active`'s doc. Once `set_icon` flips it, there is
        /// no way back.
        #[test]
        fn status_is_passive_until_set_icon_then_active() {
            let shared = shared_with_menu(None);
            let iface = StatusNotifierItemIface {
                shared: Arc::clone(&shared),
                waker: Arc::new(|_| {}),
            };

            assert_eq!(iface.status(), "Passive");

            shared.lock().unwrap_or_else(|e| e.into_inner()).active = true;

            assert_eq!(iface.status(), "Active");
        }

        /// `ItemIsMenu` mirrors whether a menu is currently attached, not
        /// a fixed value — a KDE/waybar host reads this to decide
        /// whether left-click opens the dbusmenu or sends `Activate`.
        #[test]
        fn item_is_menu_mirrors_whether_a_menu_is_attached() {
            let without_menu = StatusNotifierItemIface {
                shared: shared_with_menu(None),
                waker: Arc::new(|_| {}),
            };
            assert!(!without_menu.item_is_menu());

            let with_menu = StatusNotifierItemIface {
                shared: shared_with_menu(Some(menu_bar(vec![item("one")]))),
                waker: Arc::new(|_| {}),
            };
            assert!(with_menu.item_is_menu());
        }
    }
}

#[cfg(target_os = "linux")]
use linux::connect;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::StyledText;

    fn item(label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(label)),
            label: StyledText::plain(label),
            detail: None,
            disabled: false,
            key_equivalent: None,
            checked: None,
            submenu: None,
        }
    }

    #[test]
    fn flatten_items_preserves_order_and_inlines_submenus() {
        let mut nested = item("child");
        nested.submenu = None;
        let mut parent = item("parent");
        parent.submenu = Some(vec![item("child-a"), item("child-b")]);
        let items = vec![item("first"), parent, item("last")];

        let mut out = Vec::new();
        flatten_items(&items, &mut out);

        let labels: Vec<String> = out.iter().map(plain_label).collect();
        assert_eq!(
            labels,
            vec!["first", "parent", "child-a", "child-b", "last"]
        );
    }

    #[test]
    fn widget_id_for_dbus_id_recovers_the_original_item() {
        let items = vec![item("alpha"), item("beta")];
        let mut flattened = Vec::new();
        flatten_items(&items, &mut flattened);

        assert_eq!(
            widget_id_for_dbus_id(&flattened, 1),
            Some(WidgetId::new("alpha"))
        );
        assert_eq!(
            widget_id_for_dbus_id(&flattened, 2),
            Some(WidgetId::new("beta"))
        );
        assert_eq!(widget_id_for_dbus_id(&flattened, 0), None);
        assert_eq!(widget_id_for_dbus_id(&flattened, 99), None);
    }

    #[test]
    fn drain_tray_wake_rewrites_clicked_and_menu_payloads() {
        let mut events = vec![
            UiEvent::User(UserPayload::new(TrayWake::Clicked(MouseButton::Left))),
            UiEvent::User(UserPayload::new(TrayWake::MenuItemActivated(
                WidgetId::new("item"),
            ))),
            UiEvent::User(UserPayload::new(7_i32)),
        ];

        drain_tray_wake(&mut events);

        assert!(matches!(
            events[0],
            UiEvent::TrayClicked {
                button: MouseButton::Left
            }
        ));
        assert!(matches!(
            events[1],
            UiEvent::ContextMenuItemActivated(id) if id == WidgetId::new("item")
        ));
        assert!(matches!(events[2], UiEvent::User(_)));
    }

    /// Probes the session bus directly for a running
    /// `org.kde.StatusNotifierWatcher`, independent of
    /// [`GtkTrayState::ensure_connected`]. Used as ground truth so the
    /// tests below hold on both a bare host with no watcher (a typical CI
    /// runner) and a full desktop session with one running. `pub(crate)`
    /// so `gtk::backend`'s own tests reuse this instead of keeping a
    /// second copy.
    #[cfg(target_os = "linux")]
    pub(crate) fn watcher_is_reachable() -> bool {
        use zbus::blocking::fdo::DBusProxy;

        // Bounded the same way `linux::connect` is — see
        // `DBUS_CALL_TIMEOUT`'s doc — so this probe can't itself hang on
        // a stale bus address and make the test it backs flaky.
        let Ok(conn) = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.method_timeout(DBUS_CALL_TIMEOUT).build())
        else {
            return false;
        };
        let Ok(proxy) = DBusProxy::new(&conn) else {
            return false;
        };
        let Ok(name) = "org.kde.StatusNotifierWatcher".try_into() else {
            return false;
        };
        proxy.name_has_owner(name).unwrap_or(false)
    }

    #[test]
    fn ensure_connected_reports_watcher_availability_honestly() {
        // `ensure_connected` must neither panic nor pretend to succeed: on
        // a host with no `org.kde.StatusNotifierWatcher` reachable it
        // reports `false`, and on a host with one running (e.g. a full
        // desktop session) it reports `true` by actually registering with
        // it. Either way it must match reality rather than a fixed
        // assumption about the environment.
        let mut state = GtkTrayState::default();
        let waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_| {});

        #[cfg(target_os = "linux")]
        {
            let expected = watcher_is_reachable();
            assert_eq!(
                state.ensure_connected(waker),
                expected,
                "ensure_connected() must agree with a direct probe of watcher availability"
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(!state.ensure_connected(waker));
        }
    }

    #[test]
    fn ensure_connected_never_regresses_once_established() {
        // Once a connection is established it is cached: a second call
        // must not drop back to `false` on its own. Asserting equality
        // rather than a one-sided implication also keeps this test
        // meaningful on a CI runner with no watcher reachable at all
        // (`first == false`): repeated calls there must stay `false`
        // too, rather than this test passing vacuously in exactly the
        // environment it most needs to run in.
        let mut state = GtkTrayState::default();
        let waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_| {});
        let first = state.ensure_connected(Arc::clone(&waker));
        let second = state.ensure_connected(waker);
        assert_eq!(
            second, first,
            "ensure_connected must be stable across repeated calls when nothing in the environment changed"
        );
    }
}
