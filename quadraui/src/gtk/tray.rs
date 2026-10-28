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
//! bus and register with a watcher, and reports failure (no caching) on
//! any error — unreachable session bus, no watcher running, or the
//! `RegisterStatusNotifierItem` call itself failing. `GtkBackend::tray`
//! calls this on every invocation, so a desktop that gains a watcher
//! after startup (an extension enabled mid-session) is picked up on the
//! next call rather than being permanently refused.
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
//! `submenu` entry becomes a disabled separator-free label row followed
//! immediately by its children at the same depth — rather than
//! recursing through nested `Variant`s, since no quadraui consumer
//! currently builds more than one level of tray menu and dbusmenu's own
//! recursive-through-`Variant` shape earns the extra complexity only
//! once one does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
#[derive(Default)]
struct TrayShared {
    tooltip: String,
    /// Raw ARGB32 pixmaps in the `(width, height, bytes)` shape
    /// `org.kde.StatusNotifierItem`'s `IconPixmap` property expects:
    /// network byte order (big-endian), 4 bytes per pixel, alpha first.
    icon: Vec<(i32, i32, Vec<u8>)>,
    menu: Option<ContextMenu>,
}

impl TrayShared {
    fn has_menu(&self) -> bool {
        self.menu.is_some()
    }
}

/// Per-[`super::backend::GtkBackend`] tray state. `connection` is
/// `None` until [`Self::ensure_connected`] first succeeds; on Linux
/// without a reachable `org.kde.StatusNotifierWatcher` it stays `None`
/// forever, and `GtkBackend::tray` reports that honestly as `None`
/// rather than caching a false "connected".
#[derive(Default)]
pub(crate) struct GtkTrayState {
    #[cfg(target_os = "linux")]
    connection: Option<zbus::blocking::Connection>,
    shared: Arc<Mutex<TrayShared>>,
}

impl GtkTrayState {
    /// Attempts to reach the session bus and register a
    /// `StatusNotifierItem`/`dbusmenu` pair with whatever watcher is
    /// running, if not already connected. Returns whether a tray is
    /// usable right now. Never caches a failure — see this module's
    /// doc for why a later call after this one may still succeed.
    pub(crate) fn ensure_connected(
        &mut self,
        waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    ) -> bool {
        #[cfg(target_os = "linux")]
        {
            if self.connection.is_some() {
                return true;
            }
            match connect(Arc::clone(&self.shared), waker) {
                Ok(conn) => {
                    self.connection = Some(conn);
                    true
                }
                Err(_) => false,
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
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.icon = vec![pixmap];
        Ok(())
    }

    pub(crate) fn set_tooltip(&mut self, tooltip: &str) -> ServiceResult<()> {
        self.require_connected()?;
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.tooltip = tooltip.to_string();
        Ok(())
    }

    pub(crate) fn set_menu(&mut self, menu: &ContextMenu) -> ServiceResult<()> {
        self.require_connected()?;
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        // An empty `ContextMenu` detaches, same as every other
        // `TrayService::set_menu` implementation — see that trait's
        // own doc.
        shared.menu = if menu.items.is_empty() {
            None
        } else {
            Some(menu.clone())
        };
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
}

/// Flattens an `ImageSource` to a single ARGB32 pixmap for
/// `IconPixmap`. Best-effort: a `GtkTrayState` with no decodable icon
/// simply advertises an empty pixmap list rather than failing the
/// whole tray — most SNI hosts fall back to `IconName`/a placeholder
/// when `IconPixmap` is empty.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn decode_icon(source: &ImageSource) -> Option<(i32, i32, Vec<u8>)> {
    let bytes = match source {
        ImageSource::Bytes(bytes) => bytes.clone(),
        ImageSource::Path(path) => std::fs::read(path).ok()?,
    };
    let image = image_crate_decode(&bytes)?;
    Some(image)
}

/// Decodes raw image bytes into `IconPixmap`'s wire shape without a
/// decoder dependency of its own: GTK already links `gdk-pixbuf`
/// (`super::image::load_pixbuf` uses it for every other image
/// primitive), so this reuses that rather than adding a second decode
/// path for the one pixel format this module needs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn image_crate_decode(bytes: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
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
        let n_channels = if has_alpha { 4 } else { 3 };
        let rowstride = pixbuf.rowstride();
        let src = pixbuf.pixel_bytes()?;
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let row_start = (y * rowstride) as usize;
            for x in 0..width {
                let px = row_start + (x * n_channels) as usize;
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

        #[zbus(property)]
        fn status(&self) -> String {
            "Active".to_string()
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

        #[zbus(property)]
        fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let path = if shared.has_menu() { "/MenuBar" } else { "/" };
            zbus::zvariant::ObjectPath::try_from(path)
                .expect("static path is valid")
                .into()
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
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
            Ok((0, (0, root_props, children?)))
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
            props.insert(
                "enabled".to_string(),
                OwnedValue::try_from(Value::from(!item.disabled)).expect("bool converts"),
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
    /// session bus, no watcher, registration refused — bubbles up as
    /// `Err`, which [`super::GtkTrayState::ensure_connected`] turns into
    /// an honest `false` rather than a half-registered connection.
    pub(super) fn connect(
        shared: Arc<Mutex<TrayShared>>,
        waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    ) -> zbus::Result<Connection> {
        let connection = Connection::session()?;
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
        watcher.call_method("RegisterStatusNotifierItem", &(unique_name,))?;
        Ok(connection)
    }
}

#[cfg(target_os = "linux")]
use linux::connect;

#[cfg(test)]
mod tests {
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
    /// runner) and a full desktop session with one running.
    #[cfg(target_os = "linux")]
    fn watcher_is_reachable() -> bool {
        use zbus::blocking::{fdo::DBusProxy, Connection};

        let Ok(conn) = Connection::session() else {
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
        // must not drop back to `false` on its own.
        let mut state = GtkTrayState::default();
        let waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_| {});
        let first = state.ensure_connected(Arc::clone(&waker));
        let second = state.ensure_connected(waker);
        if first {
            assert!(
                second,
                "a once-established tray connection must stay connected"
            );
        }
    }
}
