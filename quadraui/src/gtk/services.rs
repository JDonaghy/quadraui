//! GTK implementation of [`quadraui::PlatformServices`].
//!
//! Clipboard uses `arboard` for synchronous system clipboard access
//! (GTK's native read API is async, incompatible with the sync trait).
//! `open_url` uses GIO. File dialogs use `gtk4::FileDialog`, and message
//! dialogs use `gtk4::AlertDialog` (quadraui#666) — `gtk4::MessageDialog`
//! has been deprecated since GTK 4.10, and the fleet is on 4.14.5 — both
//! behind the same nested-mainloop adapter (see [`pump_until_ready`]) so
//! the trait's synchronous signatures can be honored even though GTK4
//! only exposes async dialog APIs (#427).
//!
//! ## Notifications (issue #955)
//!
//! `send_notification` is a real `gio::Notification` sent through the
//! `gtk4::Application` the window belongs to
//! (`GtkWindowExt::application`) — no async trait shape was needed after
//! all; `gio::Application::send_notification` is itself synchronous
//! (fire-and-forget, like every other `PlatformServices` method). This
//! is the one backend [`crate::backend::BackendCaps::notifications`]
//! reports `true` for — see that field's own doc and
//! `compose::notification::notify_or_toast` for how a caller that wants
//! to work on every backend degrades on the other three.
//!
//! Action buttons and the notification body itself route back through a
//! single app-scoped `GAction` (`app.quadraui-notification-activated`,
//! [`NOTIFICATION_ACTION_ID`]), registered once per `Application` by
//! [`GtkPlatformServices::ensure_notification_action`] and never
//! per-notification — which `Notification`/action a given activation
//! belongs to travels in the `GAction`'s own `(ss)` target `Variant`
//! ([`notification_target`]/[`decode_notification_target`]), not in the
//! action's identity. The activation handler decodes that target and
//! pushes [`crate::UiEvent::NotificationActivated`] onto the same shared
//! event queue [`crate::gtk::backend::GtkBackend::poll_events`] drains —
//! wired via [`GtkPlatformServices::set_events_handle`], called once from
//! [`crate::gtk::backend::GtkBackend::new`] right after both the queue
//! and the services exist, mirroring [`GtkPlatformServices::set_window`].
//!
//! `n.icon()` maps to `gio::BytesIcon`/`gio::FileIcon` — GIO's own
//! `GLoadableIcon` machinery decodes/loads it, not this crate's image
//! pipeline, so no pixel decoding happens in-process for a notification
//! icon the way [`Backend::draw_image`][crate::Backend::draw_image] does
//! for a painted one. `n.is_silent()` has nothing to bind to: GTK4's
//! `gio::Notification` carries no sound-control property at all — the
//! desktop shell/notification daemon decides, same as
//! [`Self::show_message_dialog`]'s `opts.severity` having nowhere to go
//! on `gtk4::AlertDialog`.
//!
//! ## Tray / status-bar icon (issue #953) — deliberately not implemented
//! here
//!
//! `GtkBackend::tray` (see [`crate::backend::Backend::tray`]) is left at
//! the trait's `None` default rather than gaining a `GtkTrayService` in
//! this file. Unlike every other gap this module documents, this one
//! isn't "stubbed pending a trait shape" — it's "no GTK4 API exists at
//! all": GTK4 itself ships no status-icon widget (`GtkStatusIcon` was
//! removed in the GTK3→4 transition), so a real implementation needs
//! StatusNotifierItem over D-Bus (the `ksni` crate, or a hand-rolled
//! implementation) or `libayatana-appindicator`, and must additionally
//! report honest `Unsupported`/`None` on desktops with no SNI host
//! running at all (stock GNOME without an extension, notably) rather
//! than silently no-oping. That's a materially larger, separate piece of
//! work than the macOS (`NSStatusBar`) and Win (`Shell_NotifyIconW`)
//! implementations, which reuse existing in-tree image-decode and
//! `ContextMenu`-rendering machinery — see `macos::tray`/`win::tray`'s
//! module docs for those. Tracked as GTK follow-up, not silently
//! dropped: `Backend::tray` returning `None` here is the honest,
//! structural "not yet" this crate's docs consistently prefer over a
//! capability flag nobody set.
//!
//! ## Re-entrancy guard (#427 follow-up)
//!
//! `pump_until_ready` is called from inside `AppLogic::handle`, which
//! `quadraui::gtk::run` invokes while holding the shared
//! `Rc<RefCell<GtkBackend>>` mutably borrowed for the whole call. Pumping
//! `glib::MainContext::iteration(true)` in that state lets *any* pending
//! GLib source run — including the runner's own 33ms idle-drain timer and
//! every input event controller, all of which also do
//! `backend.borrow_mut()`. Left unguarded, that second borrow panics with
//! "already borrowed", and because it happens inside a non-unwindable GLib
//! callback frame, the panic aborts the whole process instead of
//! propagating. `pumping` (a depth counter, not a bool, so nested dialogs
//! stay guarded until the outermost pump finishes) lets those callbacks
//! detect "a dialog pump is in flight further up the stack" and no-op
//! instead of touching the backend. See `GtkBackend::pump_depth` /
//! `quadraui::gtk::run::activate`.
//!
//! The depth counter and its RAII guard are the backend-neutral
//! [`crate::desktop::ModalPumpDepth`] / [`crate::desktop::ModalPumpGuard`]
//! (#498) — extracted here first (#427) and generalised because AppKit's
//! `runModal` and Win32's `IFileOpenDialog::Show` have the identical
//! nested-pump re-entrancy hazard.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;
use gtk4::prelude::GtkWindowExt;

use crate::backend::{
    BackendError, Clipboard, FileDialogOptions, MessageDialogButton, MessageDialogChoice,
    MessageDialogOptions, Notification, RgbaImage, ServiceResult, SystemTheme,
};
use crate::desktop::{ModalPumpDepth, ModalPumpGuard};
use crate::event::UiEvent;
use crate::primitives::image::ImageSource;
use crate::types::WidgetId;
use crate::PlatformServices;

/// The event queue `GtkBackend::poll_events` drains — the same handle
/// `GtkBackend::events_handle` returns. Aliased so [`GtkPlatformServices::events`]'s
/// type stays under clippy's `type_complexity` threshold.
type EventQueueHandle = Rc<RefCell<VecDeque<UiEvent>>>;

/// GTK platform-services impl. Clipboard is backed by `arboard` for
/// cross-platform synchronous access. File dialogs use `gtk4::FileDialog`
/// pumped through a nested main-loop iteration (see module docs).
/// Notifications use a real `gio::Notification` (issue #955, see module
/// docs).
pub struct GtkPlatformServices {
    clipboard: GtkClipboard,
    /// Top-level window used to parent file dialogs, so they open modal
    /// to (and centered on) the app window instead of floating
    /// unparented. `None` until [`Self::set_window`] is called (and in
    /// unit tests, which never call it) — dialogs opened before that
    /// point, or in tests, still work, just without a parent.
    window: Rc<RefCell<Option<gtk4::ApplicationWindow>>>,
    /// Depth counter, `> 0` while a [`pump_until_ready`] nested-mainloop
    /// wait is in flight (possibly several, if a dialog is opened
    /// re-entrantly from inside another dialog's pump). Shared (via
    /// [`Self::pump_depth`]) with `quadraui::gtk::run`'s event
    /// controllers and idle-drain timer so they can detect the
    /// re-entrant-pump condition and skip touching the backend's
    /// `RefCell` — see the module-level re-entrancy note.
    pumping: ModalPumpDepth,
    /// The queue `GtkBackend::poll_events` drains, shared via
    /// [`Self::set_events_handle`] (issue #955) — `None` until that's
    /// called (and in unit tests, which never call it), mirroring
    /// `window`'s own "`None` until wired" shape. A notification action
    /// activated before this is set (or in a test-only
    /// `GtkPlatformServices` that never wires it) simply has nowhere to
    /// deliver [`crate::UiEvent::NotificationActivated`] — see
    /// [`Self::ensure_notification_action`].
    events: Rc<RefCell<Option<EventQueueHandle>>>,
    /// Whether [`Self::ensure_notification_action`] has already
    /// registered `app.quadraui-notification-activated` on the current
    /// `Application`. Checked first so `send_notification` doesn't pay
    /// for `ActionMap::lookup_action` on every call once it's
    /// registered.
    action_registered: Cell<bool>,
}

impl GtkPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: GtkClipboard::new(),
            window: Rc::new(RefCell::new(None)),
            pumping: ModalPumpDepth::new(),
            events: Rc::new(RefCell::new(None)),
            action_registered: Cell::new(false),
        }
    }

    /// Store the top-level window handle so file dialogs can be parented
    /// to it. Called once by `GtkBackend::set_window` right after the
    /// window is constructed.
    pub(crate) fn set_window(&self, window: gtk4::ApplicationWindow) {
        *self.window.borrow_mut() = Some(window);
    }

    /// Share the backend's event queue so a notification-action
    /// activation can push [`crate::UiEvent::NotificationActivated`]
    /// onto it (issue #955). Called once by [`crate::gtk::backend::GtkBackend::new`]
    /// right after both the queue and `self` exist — see the module doc.
    pub(crate) fn set_events_handle(&self, events: EventQueueHandle) {
        *self.events.borrow_mut() = Some(events);
    }

    /// Register the shared `app.quadraui-notification-activated`
    /// [`gio::SimpleAction`] on `app`, exactly once. Idempotent two ways:
    /// [`Self::action_registered`] short-circuits repeat calls from this
    /// `GtkPlatformServices`, and `app.lookup_action` guards against a
    /// second `GtkPlatformServices` (or a second call before the first
    /// one's `events` handle was wired — see below) sharing the same
    /// `Application`, e.g. across two windows of the same app.
    ///
    /// A no-op when [`Self::events`] hasn't been wired yet
    /// ([`Self::set_events_handle`] not called) — the next
    /// `send_notification` retries. This only happens before
    /// `GtkBackend::new` finishes, or in a unit test that constructs
    /// `GtkPlatformServices` directly and calls `send_notification`
    /// without a backend at all; a real app always has both wired before
    /// the event loop starts.
    fn ensure_notification_action(&self, app: &gtk4::Application) {
        if self.action_registered.get() {
            return;
        }
        if app.lookup_action(NOTIFICATION_ACTION_ID).is_some() {
            self.action_registered.set(true);
            return;
        }
        let Some(events) = self.events.borrow().clone() else {
            return;
        };
        let param_type = <(String, String)>::static_variant_type();
        let action = gio::SimpleAction::new(NOTIFICATION_ACTION_ID, Some(&*param_type));
        action.connect_activate(move |_action, parameter| {
            let Some(parameter) = parameter else {
                return;
            };
            let Some((tag, action_id)) = decode_notification_target(parameter) else {
                return;
            };
            events
                .borrow_mut()
                .push_back(UiEvent::NotificationActivated {
                    tag,
                    action: action_id,
                });
        });
        app.add_action(&action);
        self.action_registered.set(true);
    }

    /// Clone of the pump-depth counter (see the `pumping` field docs).
    /// `quadraui::gtk::run::activate` fetches this once, before installing
    /// any event controllers, and clones it into each closure that would
    /// otherwise call `backend.borrow_mut()` — so they can check
    /// `depth.is_pumping()` and no-op while a dialog's nested pump is live.
    pub(crate) fn pump_depth(&self) -> ModalPumpDepth {
        self.pumping.clone()
    }

    /// Concrete (non-trait-object) handle on the clipboard, so in-crate
    /// tests can call [`GtkClipboard::install_test_contents`] — the
    /// `&dyn Clipboard` returned by [`PlatformServices::clipboard`]
    /// can't reach an inherent method. Test-only (quadraui#415).
    #[cfg(test)]
    pub(crate) fn gtk_clipboard(&self) -> &GtkClipboard {
        &self.clipboard
    }
}

impl Default for GtkPlatformServices {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for GtkPlatformServices {
    fn clipboard(&self) -> &dyn Clipboard {
        &self.clipboard
    }

    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let dialog = build_file_dialog(&opts, None);
        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.open(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        pump_until_ready(&result, &self.pumping)
            .ok()
            .and_then(|file| gtk4::prelude::FileExt::path(&file))
    }

    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let initial_name = opts.initial_filename.clone();
        let dialog = build_file_dialog(&opts, initial_name.as_deref());
        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.save(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        pump_until_ready(&result, &self.pumping)
            .ok()
            .and_then(|file| gtk4::prelude::FileExt::path(&file))
    }

    /// `gtk4::FileDialog::select_folder` (quadraui#935) — the exact call
    /// vimcode#815 removed when it gave up its native "Open Folder" panel
    /// for lack of this primitive. Same builder, same nested-pump adapter
    /// as the file dialogs above.
    fn show_folder_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let dialog = build_file_dialog(&opts, None);
        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        pump_until_ready(&result, &self.pumping)
            .ok()
            .and_then(|file| gtk4::prelude::FileExt::path(&file))
    }

    /// `gtk4::AlertDialog::choose()` (quadraui#666) — see the module docs
    /// for why `AlertDialog`, not the deprecated `MessageDialog`. Driven
    /// through the same [`pump_until_ready`] + `pump_depth` guard the
    /// file dialogs use above; this does **not** hand-roll a second
    /// nested loop.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        let order = hig_button_order(&opts.buttons);
        let labels: Vec<&str> = order
            .iter()
            .map(|&i| opts.buttons[i].label.as_str())
            .collect();

        let dialog = gtk4::AlertDialog::default();
        dialog.set_message(&opts.title);
        dialog.set_detail(&opts.body);
        dialog.set_modal(true);
        dialog.set_buttons(&labels);
        // `opts.severity` is intentionally unread here: gtk4-rs 0.7.3's
        // `AlertDialog` (`auto/alert_dialog.rs`) exposes no icon-setting
        // API at all, so there is nothing to map it onto. Not a bug —
        // `MessageDialogOptions::severity`'s doc already says "backends
        // *may* use this" — but worth calling out so the next reader
        // doesn't go looking for where it's supposed to go.
        if let Some(pos) = order.iter().position(|&i| opts.buttons[i].is_cancel) {
            dialog.set_cancel_button(pos as i32);
        }
        if let Some(pos) = order.iter().position(|&i| opts.buttons[i].is_default) {
            dialog.set_default_button(pos as i32);
        }

        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.choose(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        let idx = pump_until_ready(&result, &self.pumping).ok()?;
        let pos = usize::try_from(idx).ok()?;
        let orig = *order.get(pos)?;
        Some(opts.buttons[orig].id.clone())
    }

    /// `gio::Notification` sent through the window's owning
    /// `gtk4::Application` (issue #955) — see the module doc for the full
    /// design (action routing, icon decode, the `silent` gap). A no-op
    /// when there is no window yet, or the window has no `Application`
    /// (a `GtkPlatformServices` used outside `quadraui::gtk::run`, e.g.
    /// directly in a unit test) — the same "nothing to parent this to
    /// yet" degrade `show_file_open_dialog` et al. already have via
    /// `self.window.borrow().clone()`.
    fn send_notification(&self, n: Notification) {
        let Some(window) = self.window.borrow().clone() else {
            return;
        };
        let Some(app) = window.application() else {
            return;
        };
        self.ensure_notification_action(&app);

        let notification = gio::Notification::new(&n.title);
        notification.set_body(Some(&n.body));
        notification.set_priority(if n.urgent {
            gio::NotificationPriority::Urgent
        } else {
            gio::NotificationPriority::Normal
        });
        if let Some(icon_source) = n.icon() {
            notification.set_icon(&decode_gio_icon(icon_source));
        }

        let tag = n.tag();
        notification.set_default_action_and_target_value(
            NOTIFICATION_ACTION_DETAILED,
            Some(&notification_target(tag, None)),
        );
        for (id, label) in n.actions() {
            notification.add_button_with_target_value(
                label,
                NOTIFICATION_ACTION_DETAILED,
                Some(&notification_target(tag, Some(id.as_str()))),
            );
        }

        // `gio::Application::send_notification`'s `id` both dedupes
        // repeat calls (a second `send_notification` with the same id
        // replaces the on-screen notification instead of stacking a
        // second one) and is what a later
        // `gio::Application::withdraw_notification(id)` would target —
        // `Notification::with_tag`'s doc names this as the intended use.
        // Untagged notifications each get a fresh id from the counter
        // below so they never collide with each other.
        let owned_tag;
        let notif_id: &str = match tag {
            Some(tag) => tag,
            None => {
                owned_tag = format!(
                    "quadraui-notification-{}",
                    NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed)
                );
                &owned_tag
            }
        };
        app.send_notification(Some(notif_id), &notification);
    }

    fn open_url(&self, url: &str) {
        let _ =
            gtk4::gio::AppInfo::launch_default_for_uri(url, None::<&gtk4::gio::AppLaunchContext>);
    }

    /// quadraui#952: `gtk4::Settings`' dark-preference + theme-name
    /// properties. `Settings::default()` returns `None` only when GTK has
    /// no default display connection at all (headless dev box, CI) — the
    /// same condition this module's own `require_gtk` test helper skips
    /// its dialog tests on — so that's the one case this reports
    /// `Unsupported` rather than a guessed value.
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        let settings = gtk4::Settings::default().ok_or(BackendError::Unsupported)?;
        let dark = settings.is_gtk_application_prefer_dark_theme();
        let theme_name = settings.gtk_theme_name();
        Ok(system_theme_from_gtk_settings(dark, theme_name.as_deref()))
    }

    fn platform_name(&self) -> &'static str {
        "gtk"
    }
}

// ── Notifications (issue #955) ───────────────────────────────────────────

/// Counter backing an untagged notification's `gio::Application::send_notification`
/// id — a fresh value per call, so two untagged notifications never
/// collide and replace each other the way two calls with the same `tag`
/// deliberately do. Mirrors `win::services::NEXT_NOTIFICATION_ID`'s exact
/// role for the Win-GUI balloon-tip path.
static NEXT_NOTIFICATION_ID: AtomicU64 = AtomicU64::new(1);

/// Bare name [`gio::SimpleAction::new`]/`ActionMap::add_action` register
/// under — no `app.` prefix, since that's implied by which action group
/// (`Application`'s own) it's added to.
const NOTIFICATION_ACTION_ID: &str = "quadraui-notification-activated";

/// The same action, `app.`-prefixed, as
/// [`gio::Notification::set_default_action_and_target_value`]/
/// `add_button_with_target_value` expect — those two are NOT "detailed
/// action name" parsers (no `::target` suffix syntax); they take a plain
/// action name scoped by group prefix, which for an app-registered
/// action is always `app.`.
const NOTIFICATION_ACTION_DETAILED: &str = "app.quadraui-notification-activated";

/// Encode `(tag, action)` as the `(ss)` `Variant` the shared
/// notification action's target carries — empty string standing in for
/// `None` on each side (see [`decode_notification_target`]'s doc for the
/// resulting edge case). A tuple over a custom `GVariant` dict/struct
/// because `(ss)` is exactly two fixed fields, no future third one
/// anticipated, and glib's tuple `ToVariant`/`FromVariant` impls need no
/// extra ceremony to round-trip through.
fn notification_target(tag: Option<&str>, action: Option<&str>) -> glib::Variant {
    (tag.unwrap_or_default(), action.unwrap_or_default()).to_variant()
}

/// Inverse of [`notification_target`] — decodes a notification action's
/// activation `Variant` back into `(tag, action)`. `None` on a
/// non-`(ss)` payload (shouldn't happen: this crate is the only writer
/// of this action's target), rather than panicking on a malformed
/// `Variant` from a source this module doesn't control.
///
/// An empty string on either side round-trips as `None` — so a
/// `Notification` tagged `""` (rather than `with_tag` never called) or
/// an action registered with `WidgetId::new("")` is indistinguishable
/// from having no tag/action at all. Both are degenerate inputs no
/// caller in this crate constructs; documented here rather than guarded
/// against, matching this module's existing posture on similarly
/// unreachable-in-practice edge cases (e.g. `hig_button_order`'s
/// declared-order fallback).
fn decode_notification_target(
    variant: &glib::Variant,
) -> Option<(Option<String>, Option<WidgetId>)> {
    let (tag, action) = variant.get::<(String, String)>()?;
    let tag = if tag.is_empty() { None } else { Some(tag) };
    let action = if action.is_empty() {
        None
    } else {
        Some(WidgetId::new(action))
    };
    Some((tag, action))
}

/// Decode a [`ImageSource`] into a `gio::Icon` for
/// [`gio::Notification::set_icon`]. GIO does its own format sniffing
/// (`GLoadableIcon`) for `Bytes`; `Path` hands the file path straight to
/// the notification daemon via `gio::FileIcon` rather than this process
/// reading the bytes itself — the daemon needs the file to still exist
/// when it gets around to rendering the notification either way, so
/// there's no correctness difference, only one less read in this
/// process.
fn decode_gio_icon(source: &ImageSource) -> gio::Icon {
    match source {
        ImageSource::Bytes(bytes) => {
            gio::BytesIcon::new(&glib::Bytes::from_owned(bytes.clone())).upcast()
        }
        ImageSource::Path(path) => gio::FileIcon::new(&gio::File::for_path(path)).upcast(),
    }
}

/// Pure mapping from `gtk4::Settings`' two theme properties to
/// [`SystemTheme`] — split out from `system_theme` so it's unit-testable
/// without a live GTK display, mirroring this module's
/// `hig_button_order`/`native_button_order`-style helpers.
///
/// No accent-colour source: GTK4 itself exposes none (that lives on
/// libadwaita's `AdwStyleManager`, which this crate doesn't depend on), so
/// `accent` is always `None` here.
fn system_theme_from_gtk_settings(dark: bool, theme_name: Option<&str>) -> SystemTheme {
    let high_contrast = theme_name
        .map(|name| name.to_ascii_lowercase().contains("highcontrast"))
        .unwrap_or(false);
    SystemTheme {
        dark,
        accent: None,
        high_contrast,
    }
}

/// Build a `gtk4::FileDialog` from the backend-agnostic
/// [`FileDialogOptions`]. `initial_name` is passed separately (rather
/// than read off `opts.initial_filename`) because it only applies to
/// the save dialog — `show_file_open_dialog` calls this with `None`.
fn build_file_dialog(opts: &FileDialogOptions, initial_name: Option<&str>) -> gtk4::FileDialog {
    let dialog = gtk4::FileDialog::new();
    if let Some(ref title) = opts.title {
        dialog.set_title(title);
    }
    if let Some(ref dir) = opts.initial_dir {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    if let Some(name) = initial_name {
        dialog.set_initial_name(Some(name));
    }
    if !opts.filters.is_empty() {
        let filters = gio::ListStore::new::<gtk4::FileFilter>();
        for (name, extensions) in &opts.filters {
            let filter = gtk4::FileFilter::new();
            filter.set_name(Some(name));
            for ext in extensions {
                filter.add_suffix(ext);
            }
            filters.append(&filter);
        }
        dialog.set_filters(Some(&filters));
    }
    dialog
}

/// Nested-mainloop adapter: `gtk4::FileDialog::open`/`save` and
/// `gtk4::AlertDialog::choose` (quadraui#666) are async-only (GTK4
/// dropped the blocking `FileChooserDialog` API and never had a blocking
/// alert), but [`PlatformServices::show_file_open_dialog`] /
/// `show_file_save_dialog` / `show_message_dialog` are synchronous
/// across every backend (the signature macOS's `NSOpenPanel::runModal` /
/// `NSAlert::runModal` and Win32's `comdlg32` / `MessageBoxEx` map onto
/// directly). This closes the gap the same way GTK itself closes it
/// internally for modal dialogs (e.g. the legacy `gtk_dialog_run`): pump
/// `glib::MainContext::iteration(true)` — which blocks until *some*
/// source is ready and dispatches it — in a loop until the async
/// operation's completion callback has stashed a result in `result`.
/// Generic over the result type so every blocking-dialog call site
/// (file open/save, message/alert) shares this one pump instead of each
/// hand-rolling its own nested loop.
///
/// # Re-entrancy note
///
/// Pumping the main loop here lets *any* pending GLib source run,
/// including redraw/timer/other-widget callbacks that would normally
/// wait their turn — the same way a native nested loop (or GTK's old
/// `gtk_dialog_run`) does. If one of those callbacks itself triggers
/// another dialog, the inner call's `iteration(true)` loop will also
/// service the outer dialog's completion source, which is safe but
/// means dialogs can resolve out of call order. Apps should avoid
/// opening a second dialog from inside a callback that runs while one is
/// already open.
///
/// Critically, this is called while `quadraui::gtk::run`'s caller (an
/// `AppLogic::handle` invocation) still holds the shared
/// `GtkBackend`'s `RefCell` mutably borrowed. `pumping` — bumped for the
/// duration of this call — is how the runner's other backend-touching
/// callbacks (idle-drain timer, input controllers, draw func) detect
/// that and skip their own `backend.borrow_mut()` instead of panicking
/// on a double-borrow (#427).
fn pump_until_ready<T>(result: &Rc<RefCell<Option<T>>>, pumping: &ModalPumpDepth) -> T {
    let _guard = ModalPumpGuard::new(pumping);
    let ctx = glib::MainContext::default();
    while result.borrow().is_none() {
        ctx.iteration(true);
    }
    result
        .borrow_mut()
        .take()
        .expect("loop above only exits once `result` is Some")
}

/// GNOME HIG button ordering for `gtk4::AlertDialog::set_buttons`: the
/// cancel button (if any) goes first (leftmost); the default/primary
/// button (if any) goes last (rightmost); every other button keeps its
/// original relative order in between. Returns indices into `buttons`,
/// in the order to hand to `set_buttons` — `show_message_dialog` maps
/// the chosen index back through this same `Vec` to recover the
/// original [`MessageDialogButton::id`].
///
/// A button with both `is_default` and `is_cancel` set (a single-button
/// "OK" dialog) is treated as the cancel slot here — see below, both
/// `set_cancel_button` and `set_default_button` are still pointed at its
/// position afterward regardless of which bucket placed it.
fn hig_button_order(buttons: &[MessageDialogButton]) -> Vec<usize> {
    let mut cancel_idx = None;
    let mut default_idx = None;
    let mut middle = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        if b.is_cancel && cancel_idx.is_none() {
            cancel_idx = Some(i);
        } else if b.is_default && default_idx.is_none() {
            default_idx = Some(i);
        } else {
            middle.push(i);
        }
    }
    let mut order = Vec::with_capacity(buttons.len());
    order.extend(cancel_idx);
    order.extend(middle);
    if let Some(d) = default_idx {
        order.push(d);
    }
    order
}

// The RAII bump/decrement for the shared pump-depth counter used to be
// defined here as `PumpGuard`; extracted to the backend-neutral
// `crate::desktop::ModalPumpGuard` by #498 — see that module's doc.

/// In-memory stand-in for the two OS selections, installed by
/// [`GtkClipboard::install_test_contents`] so unit tests can exercise the
/// clipboard-paste code paths **without** depending on whatever the host
/// running the tests happens to have on its real clipboard (quadraui#415).
///
/// Test-only on purpose: without it, a test asserting "nothing to paste"
/// passes on a headless box (where `arboard::Clipboard::new()` fails) and
/// fails on any developer machine or CI runner with a live display and a
/// non-empty clipboard — a genuinely flaky, environment-dependent
/// assertion rather than a statement about quadraui's behaviour.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct TestClipboardContents {
    /// Contents of the CLIPBOARD selection — what Ctrl-V / Ctrl-Shift-V
    /// reads. `None` models "nothing has been copied".
    pub clipboard: Option<String>,
    /// Contents of the PRIMARY selection — what middle-click reads.
    /// `None` models "nothing is selected anywhere".
    pub primary: Option<String>,
}

/// System clipboard via `arboard`. The handle is kept alive for the
/// process lifetime so Linux clipboard serving threads persist.
pub struct GtkClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
    /// When `Some`, every read/write below is served from this in-memory
    /// fake instead of the OS, making clipboard-dependent unit tests
    /// deterministic on any host. Only ever populated by
    /// [`Self::install_test_contents`]; production builds don't compile
    /// the field at all.
    #[cfg(test)]
    test_contents: RefCell<Option<TestClipboardContents>>,
}

impl GtkClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
            #[cfg(test)]
            test_contents: RefCell::new(None),
        }
    }

    /// Swap this clipboard over to an in-memory fake seeded with
    /// `contents`. Every subsequent read and write goes to the fake and
    /// the OS clipboard is left untouched — so a test can assert on the
    /// "there is something to paste" and "there is nothing to paste"
    /// branches independently of the host (quadraui#415).
    #[cfg(test)]
    pub(crate) fn install_test_contents(&self, contents: TestClipboardContents) {
        *self.test_contents.borrow_mut() = Some(contents);
    }

    /// OS-level PRIMARY-selection read, split out from the trait method
    /// so the trait method itself stays uncfg'd (and therefore honours
    /// [`Self::install_test_contents`] on every target).
    ///
    /// Gated to the same `cfg` `arboard` itself uses for `GetExtLinux`
    /// (see `arboard::lib`). Windows/macOS have no PRIMARY-selection
    /// concept, and quadraui's `gtk` feature only ships a Linux backend
    /// in practice, so there the OS read is simply `None`.
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    ))]
    fn read_os_primary_selection(&self) -> Option<String> {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        self.inner
            .borrow_mut()
            .as_mut()?
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok()
    }

    /// Non-Linux counterpart of [`Self::read_os_primary_selection`] —
    /// there is no PRIMARY selection to read.
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    )))]
    fn read_os_primary_selection(&self) -> Option<String> {
        None
    }
}

impl Clipboard for GtkClipboard {
    fn read_text(&self) -> Option<String> {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow().as_ref() {
                return fake.clipboard.clone();
            }
        }
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow_mut().as_mut() {
                fake.clipboard = Some(text.to_string());
                return;
            }
        }
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }

    /// PRIMARY selection (middle-click paste source) — only meaningful on
    /// X11/Wayland, where `arboard`'s Linux extension trait exposes it as
    /// a distinct selection from CLIPBOARD (quadraui#415). See
    /// [`GtkClipboard::read_os_primary_selection`] for the per-target
    /// split.
    fn read_primary_selection(&self) -> Option<String> {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow().as_ref() {
                return fake.primary.clone();
            }
        }
        self.read_os_primary_selection()
    }

    /// Decoded RGBA pixels via `arboard::Clipboard::get_image` (issue
    /// #954). Not covered by [`Self::install_test_contents`]'s fake —
    /// [`TestClipboardContents`] only ever modeled text — so this always
    /// goes to the real OS clipboard, same as production.
    fn read_image(&self) -> ServiceResult<RgbaImage> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_ARBOARD_HANDLE_CONTEXT.to_string(),
        })?;
        let img = cb
            .get_image()
            .map_err(|e| map_arboard_error("arboard::get_image", e))?;
        Ok(RgbaImage {
            width: img.width as u32,
            height: img.height as u32,
            pixels: img.bytes.into_owned(),
        })
    }

    fn write_image(&self, image: &RgbaImage) -> ServiceResult<()> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_ARBOARD_HANDLE_CONTEXT.to_string(),
        })?;
        let data = arboard::ImageData {
            width: image.width as usize,
            height: image.height as usize,
            bytes: std::borrow::Cow::Borrowed(&image.pixels),
        };
        cb.set_image(data)
            .map_err(|e| map_arboard_error("arboard::set_image", e))
    }

    fn write_html(&self, html: &str, alt_text: &str) -> ServiceResult<()> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_ARBOARD_HANDLE_CONTEXT.to_string(),
        })?;
        cb.set_html(html, Some(alt_text))
            .map_err(|e| map_arboard_error("arboard::set_html", e))
    }

    fn read_file_list(&self) -> ServiceResult<Vec<PathBuf>> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_ARBOARD_HANDLE_CONTEXT.to_string(),
        })?;
        cb.get()
            .file_list()
            .map_err(|e| map_arboard_error("arboard::get().file_list()", e))
    }

    fn clear(&self) -> ServiceResult<()> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_ARBOARD_HANDLE_CONTEXT.to_string(),
        })?;
        cb.clear()
            .map_err(|e| map_arboard_error("arboard::clear", e))
    }
}

/// Context string used when there is no live `arboard::Clipboard` handle
/// at all (construction failed — e.g. headless, no display) rather than a
/// native call on a real handle failing.
const NO_ARBOARD_HANDLE_CONTEXT: &str = "arboard::Clipboard::new (no clipboard available)";

/// Map an `arboard::Error` from a named native call into a
/// [`BackendError::PlatformFailure`] (issue #954). `arboard::Error` has
/// no "unsupported" variant of its own — every arm here (including
/// `ContentNotAvailable`, e.g. "clipboard has no image right now") is a
/// real outcome of a call this backend *does* implement, so
/// `PlatformFailure` — not `BackendError::Unsupported` — is the honest
/// mapping; see `BackendError::Unsupported`'s own doc for why that
/// variant is reserved for "this backend has no implementation" instead.
fn map_arboard_error(call: &str, err: arboard::Error) -> BackendError {
    BackendError::PlatformFailure {
        context: format!("{call}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::prelude::ListModelExt;
    use std::sync::OnceLock;

    // ── Clipboard image/html/file-list/clear (#954) ────────────────────

    /// Real OS-clipboard round trip through every method #954 added to
    /// [`GtkClipboard`] — needs no `gtk4::init()` (arboard talks to the
    /// display server directly, independent of GTK), but does need a
    /// live clipboard session, which a headless CI runner (no X11/
    /// Wayland) genuinely doesn't have. Skips gracefully there rather
    /// than failing the crate's test run over an environment gap this
    /// test isn't trying to cover — same posture as
    /// `build_file_dialog_behaviors`'s display-optional skip below, for
    /// the same "can't assert about a resource that isn't there" reason.
    /// One `#[test]` fn, not several, so parallel test threads don't
    /// fight over the one real systemwide clipboard.
    #[allow(clippy::print_stderr)]
    #[test]
    fn clipboard_image_html_file_list_and_clear_round_trip() {
        let svc = GtkPlatformServices::new();
        let cb = svc.clipboard();

        let image = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                255, 255, 0, 255, // yellow
            ],
        };
        if let Err(e) = cb.write_image(&image) {
            eprintln!("skipping: no live OS clipboard in this environment ({e:?})");
            return;
        }
        let read_back = cb
            .read_image()
            .expect("read_image should see what write_image just wrote");
        assert_eq!(read_back.width, image.width);
        assert_eq!(read_back.height, image.height);
        assert_eq!(
            read_back.pixels.len(),
            (image.width * image.height * 4) as usize
        );

        cb.write_html("<b>hi</b>", "hi").expect(
            "write_html should succeed once write_image already proved the clipboard is live",
        );

        let tmp = std::env::temp_dir().join("quadraui-954-gtk-clipboard-test-file.txt");
        std::fs::write(&tmp, b"quadraui#954").expect("write temp file");
        arboard::Clipboard::new()
            .expect("a second arboard handle should also see the live clipboard")
            .set()
            .file_list(&[&tmp])
            .expect("seeding the clipboard with a file list should succeed");
        let files = cb
            .read_file_list()
            .expect("read_file_list should see the seeded file");
        assert_eq!(
            files
                .iter()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
                .collect::<Vec<_>>(),
            vec![tmp.canonicalize().expect("temp file should exist")]
        );
        let _ = std::fs::remove_file(&tmp);

        cb.write_text("some text #954");
        assert_eq!(cb.read_text(), Some("some text #954".to_string()));
        cb.clear()
            .expect("clear should succeed once the clipboard has proven live");
        assert_eq!(cb.read_text(), None);
    }

    /// Constructing a real `gtk4::FileDialog` requires `gtk4::init()` to
    /// have succeeded, which needs a display connection (X11/Wayland).
    /// CI and headless dev boxes don't have one, so these tests degrade
    /// to a no-op skip there rather than fail — mirrors how the crate's
    /// other GTK-object tests avoid needing a live display (see
    /// `gtk::tab_bar` — Cairo `ImageSurface` tests intentionally sidestep
    /// this same problem). Cached in a `OnceLock` since `gtk4::init()`
    /// after a failed first attempt keeps failing.
    ///
    /// # Thread affinity (#460)
    ///
    /// gtk4-rs's thread-affinity guard is a per-thread flag
    /// (`IS_MAIN_THREAD`, a `thread_local!` in gtk4-rs's `rt.rs`) set only
    /// on whichever thread's call actually ran `gtk_init_check()`. Rust's
    /// default test harness runs every `#[test]` fn on its own spawned OS
    /// thread, so if more than one `#[test]` fn calls `require_gtk()`,
    /// only the winner of this `OnceLock` race — one specific thread —
    /// is ever GTK-safe; every other test's thread reads the cached
    /// `true` without ever calling `gtk4::init()` itself, then panics
    /// with "GTK may only be used from the main thread" the moment it
    /// touches a real GTK object. gtk4-rs ships `#[gtk4::test]` /
    /// `test_synced` for exactly this (marshal every GTK-touching test
    /// onto one dedicated worker thread) — not used here because its
    /// `Lazy` worker-thread init calls `gtk4::init().expect(...)`
    /// unconditionally, with no graceful-skip path, which would turn a
    /// headless dev box's "no display" case into a hard panic instead of
    /// the skip this helper is designed to give it. So instead, every
    /// `#[test]` fn that needs a real `gtk4::FileDialog` lives in
    /// [`build_file_dialog_behaviors`] below — one function body, hence
    /// one OS thread, guaranteeing it's the same thread that (first) runs
    /// `gtk4::init()`. Do **not** add another separate `#[test]` fn that
    /// calls `require_gtk()` and then constructs a real GTK object —
    /// fold its assertions into `build_file_dialog_behaviors` instead, or
    /// this panic comes back.
    fn require_gtk() -> bool {
        static INIT: OnceLock<bool> = OnceLock::new();
        *INIT.get_or_init(|| gtk4::init().is_ok())
    }

    /// Covers every `build_file_dialog` behavior that requires
    /// constructing a real `gtk4::FileDialog`. Deliberately one `#[test]`
    /// fn (not three) — see the thread-affinity note on [`require_gtk`]
    /// (#460): splitting these across separate `#[test]` fns lets the
    /// Rust test harness run them on different OS threads, which
    /// deterministically panics every thread except whichever one wins
    /// the `require_gtk()` / `gtk4::init()` race.
    // #619: exempt from the crate-wide `print_stderr` deny. `cargo test`
    // output, not code that runs inside a host's live UI session — this
    // line only ever executes on a headless dev box with no display,
    // where it's the reason the test reports "ok" without covering
    // anything.
    #[allow(clippy::print_stderr)]
    #[test]
    fn build_file_dialog_behaviors() {
        if !require_gtk() {
            eprintln!("skipping: GTK failed to initialize (no display available)");
            return;
        }

        // The dialog builder must apply every `FileDialogOptions` field
        // GTK has a settable property for, so a future maintainer
        // changing the wiring can't silently drop one (e.g. forgetting
        // `set_filters`).
        {
            let opts = FileDialogOptions {
                title: Some("Open File".to_string()),
                initial_dir: Some(PathBuf::from("/tmp")),
                initial_filename: None,
                filters: vec![("Rust files".to_string(), vec!["rs".to_string()])],
            };
            let dialog = build_file_dialog(&opts, None);
            assert_eq!(dialog.title(), "Open File");
            assert_eq!(
                gtk4::prelude::FileExt::path(&dialog.initial_folder().unwrap()),
                Some(PathBuf::from("/tmp"))
            );
            let filters = dialog.filters().expect("filters should be set");
            assert_eq!(filters.n_items(), 1);
        }

        // `initial_name` only applies when explicitly passed (the
        // save-dialog path) — `show_file_open_dialog` always passes
        // `None` regardless of `opts.initial_filename`, since that field
        // is documented save-only.
        {
            let opts = FileDialogOptions {
                initial_filename: Some("untitled.txt".to_string()),
                ..Default::default()
            };
            let without = build_file_dialog(&opts, None);
            assert_eq!(without.initial_name(), None);

            let with = build_file_dialog(&opts, opts.initial_filename.as_deref());
            assert_eq!(with.initial_name().as_deref(), Some("untitled.txt"));
        }

        // No filters configured → `set_filters` must not be called (a
        // `Some` empty list would still change the dialog's
        // filter-picker UI).
        {
            let dialog = build_file_dialog(&FileDialogOptions::default(), None);
            assert!(dialog.filters().is_none());
        }
    }

    /// `set_window` stores the handle used to parent future dialogs.
    /// Nothing observable to assert without a real window, but this
    /// guards against a panic/wrong-field regression in the setter.
    #[test]
    fn set_window_is_none_until_called() {
        let services = GtkPlatformServices::new();
        assert!(services.window.borrow().is_none());
    }

    // Regression test for #427 ("depth counter stays positive until the
    // outermost re-entrant pump unwinds") used to live here directly
    // against a local `PumpGuard`; that behavior is now covered once,
    // backend-neutrally, by `crate::desktop`'s own `modal_pump_tests`
    // (#498). The test below stays GTK-specific: it pins that
    // `GtkPlatformServices::pump_depth()`'s returned handle really does
    // share state with the services' own counter (same `Rc`, not an
    // independent clone).

    /// `GtkPlatformServices::pump_depth()` — the handle
    /// `quadraui::gtk::run::activate` clones into its event
    /// controllers — must observe mutations `pump_until_ready` makes
    /// through the services' own copy (same underlying counter via
    /// `ModalPumpDepth`'s `Rc`, not an independent one).
    #[test]
    fn pump_depth_handle_observes_guard_mutations_on_the_services_copy() {
        let services = GtkPlatformServices::new();
        let handle = services.pump_depth();
        assert_eq!(handle.get(), 0);
        let _guard = ModalPumpGuard::new(&services.pumping);
        assert_eq!(
            handle.get(),
            1,
            "handle must observe the mutation made through services.pumping"
        );
    }

    // ── hig_button_order (quadraui#666) ────────────────────────────────

    fn msg_btn(id: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
        MessageDialogButton {
            id: crate::types::WidgetId::new(id),
            label: id.to_string(),
            is_default,
            is_cancel,
        }
    }

    #[test]
    fn hig_button_order_puts_cancel_first_and_default_last() {
        let buttons = vec![
            msg_btn("save", true, false),
            msg_btn("dont-save", false, false),
            msg_btn("cancel", false, true),
        ];
        // Declared order is default, other, cancel — HIG order must
        // reshuffle to cancel, other, default.
        assert_eq!(hig_button_order(&buttons), vec![2, 1, 0]);
    }

    #[test]
    fn hig_button_order_preserves_middle_button_relative_order() {
        let buttons = vec![
            msg_btn("cancel", false, true),
            msg_btn("a", false, false),
            msg_btn("b", false, false),
            msg_btn("save", true, false),
        ];
        assert_eq!(hig_button_order(&buttons), vec![0, 1, 2, 3]);
    }

    #[test]
    fn hig_button_order_single_button_both_default_and_cancel() {
        // A single "OK" button dialog: is_cancel is checked first, so it
        // lands in the cancel slot — but `show_message_dialog` still
        // points both `set_cancel_button` and `set_default_button` at
        // its position since `is_default` also reads true on it.
        let buttons = vec![msg_btn("ok", true, true)];
        assert_eq!(hig_button_order(&buttons), vec![0]);
    }

    #[test]
    fn hig_button_order_no_default_or_cancel_keeps_declared_order() {
        let buttons = vec![msg_btn("a", false, false), msg_btn("b", false, false)];
        assert_eq!(hig_button_order(&buttons), vec![0, 1]);
    }

    // ── system_theme_from_gtk_settings (quadraui#952) ───────────────────

    #[test]
    fn system_theme_from_gtk_settings_reports_dark() {
        let theme = system_theme_from_gtk_settings(true, Some("Adwaita-dark"));
        assert!(theme.dark);
        assert_eq!(theme.accent, None);
        assert!(!theme.high_contrast);
    }

    #[test]
    fn system_theme_from_gtk_settings_reports_light() {
        let theme = system_theme_from_gtk_settings(false, Some("Adwaita"));
        assert!(!theme.dark);
        assert!(!theme.high_contrast);
    }

    #[test]
    fn system_theme_from_gtk_settings_detects_high_contrast_case_insensitively() {
        assert!(system_theme_from_gtk_settings(false, Some("HighContrast")).high_contrast);
        assert!(system_theme_from_gtk_settings(false, Some("HighContrastInverse")).high_contrast);
        assert!(system_theme_from_gtk_settings(true, Some("highcontrast")).high_contrast);
    }

    #[test]
    fn system_theme_from_gtk_settings_no_theme_name_is_not_high_contrast() {
        assert!(!system_theme_from_gtk_settings(true, None).high_contrast);
    }

    // ── Notification target encode/decode + icon decode (issue #955) ────
    //
    // Pure functions — no `gtk4::init()`/display needed (unlike
    // `build_file_dialog_behaviors` above), so these run as ordinary
    // `#[test]` fns with no `require_gtk()` guard. The full
    // `send_notification` → real `gio::Notification` → on-screen click
    // path needs a live desktop notification daemon and is covered by
    // `examples/gtk_platform_services.rs`'s manual smoke test instead.

    #[test]
    fn notification_target_round_trips_tag_and_action() {
        let variant = notification_target(Some("build-status"), Some("open-problems"));
        assert_eq!(
            decode_notification_target(&variant),
            Some((
                Some("build-status".to_string()),
                Some(WidgetId::new("open-problems"))
            ))
        );
    }

    #[test]
    fn notification_target_no_tag_or_action_decodes_to_none_none() {
        let variant = notification_target(None, None);
        assert_eq!(decode_notification_target(&variant), Some((None, None)));
    }

    #[test]
    fn notification_target_tag_only() {
        let variant = notification_target(Some("t"), None);
        assert_eq!(
            decode_notification_target(&variant),
            Some((Some("t".to_string()), None))
        );
    }

    #[test]
    fn decode_gio_icon_bytes_produces_a_bytes_icon() {
        let icon = decode_gio_icon(&ImageSource::Bytes(vec![1, 2, 3]));
        assert!(icon.is::<gio::BytesIcon>());
    }

    #[test]
    fn decode_gio_icon_path_produces_a_file_icon() {
        let icon = decode_gio_icon(&ImageSource::Path(PathBuf::from(
            "/tmp/quadraui-955-icon.png",
        )));
        assert!(icon.is::<gio::FileIcon>());
    }

    /// No window wired ⇒ `send_notification` returns before touching any
    /// GTK/GIO object that would need a live display — safe to run
    /// headlessly, unlike `build_file_dialog_behaviors`.
    #[test]
    fn send_notification_without_a_window_is_a_no_op() {
        let services = GtkPlatformServices::new();
        services.send_notification(Notification::new("t", "b"));
    }

    #[test]
    fn notification_action_ids_agree_on_the_app_prefix() {
        // `NOTIFICATION_ACTION_DETAILED` must be exactly
        // `NOTIFICATION_ACTION_ID` with `"app."` prepended — a drift
        // between the two would mean `send_notification`'s buttons
        // target an action name that doesn't match what
        // `ensure_notification_action` actually registers, and clicks
        // would silently do nothing.
        assert_eq!(
            NOTIFICATION_ACTION_DETAILED,
            format!("app.{NOTIFICATION_ACTION_ID}")
        );
    }
}
