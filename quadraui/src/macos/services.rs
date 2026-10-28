//! macOS implementation of [`quadraui::PlatformServices`].
//!
//! - **Clipboard** → `arboard` (NSPasteboard under the hood; the sync
//!   API matches the [`Clipboard`] trait exactly, and shares one
//!   implementation with the TUI and GTK backends).
//! - **File dialogs** → `NSOpenPanel` / `NSSavePanel`, sync
//!   `runModal`. Apps must invoke these from event handlers running
//!   on the main thread (the `MacBackend` run loop guarantees this).
//! - **Notifications** → `UNUserNotificationCenter`
//!   (`objc2-user-notifications`) when the process is a bundled `.app`
//!   ([`NSBundle::mainBundle`] reports a `CFBundleIdentifier` —
//!   [`should_use_notification_center`]), falling back to
//!   `osascript -e 'display notification ...'` otherwise:
//!   `UNUserNotificationCenter` requires that bundle identifier and user
//!   authorization, neither of which an unbundled CLI host has.
//!
//!   On the `UNUserNotificationCenter` path, [`Notification`]'s `title`,
//!   `body`, `is_silent`, `actions`, and `tag` all round-trip:
//!   `actions` become `UNNotificationAction`s grouped under a
//!   deterministic `UNNotificationCategory` ([`category_identifier`],
//!   accumulated across calls in [`MacPlatformServices`] since
//!   `setNotificationCategories` replaces rather than merges the
//!   registered set), `tag` (or a synthetic identifier when absent —
//!   [`request_identifier`]) becomes the `UNNotificationRequest`
//!   identifier, and activating the notification or one of its actions
//!   delivers [`crate::UiEvent::NotificationActivated`] through
//!   [`NotificationEventSink`] — macOS joins GTK as a backend that
//!   variant fires on. `icon` is the one field still dropped: an icon
//!   needs a `UNNotificationAttachment` pointing at a file on disk, and
//!   this crate's `objc2-user-notifications` feature list doesn't enable
//!   that gated API (see `Cargo.toml`'s comment there).
//!
//!   The `osascript` path is unchanged: `is_silent` is the only
//!   [`Notification`] field with anywhere to go (see
//!   [`MacPlatformServices::send_notification`]'s doc), and `icon`,
//!   `actions`, and `tag` are silently dropped there too. The
//!   AppleScript-building helpers
//!   ([`crate::desktop::display_notification_script`]/
//!   `crate::desktop::applescript_escape`) live in `crate::desktop` so
//!   `tui::services` can share the identical logic for its own macOS
//!   notification fallback leg.
//! - **`open_url`/`open_url_result`** → `open <url>` via
//!   [`crate::desktop::open_with_default`] — the
//!   crate-wide opener shared with `tui::services`/`win::services`.
//!   Equivalent to `NSWorkspace.open(_:)` without needing AppKit
//!   initialisation. `open_url_result` overrides the trait's `Ok(())`
//!   default and reports whether `open` genuinely spawned instead of
//!   `open_url`'s discarded outcome.
//! - **`shell.*` parity** → `reveal_in_file_manager` uses
//!   `NSWorkspace::activateFileViewerSelectingURLs`; `open_path` reuses
//!   the same `open_with_default` call `open_url_result` makes; `move_to_trash`
//!   delegates to [`crate::desktop::move_to_trash`] — the cross-platform
//!   `trash` crate, not a hand-rolled `NSFileManager
//!   -trashItemAtURL:resultingItemURL:error:` — see that function's doc
//!   for why every backend shares it; `beep` is the bare `NSBeep` C
//!   function.
//! - **Displays** → `displays` reads `NSScreen::screens()`;
//!   `cursor_screen_point` reads `NSEvent::mouseLocation()`. See each
//!   method's own doc for the exact field mapping and coordinate-system
//!   note.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, ProtocolObject};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSBeep, NSColor, NSColorSpace,
    NSEvent, NSOpenPanel, NSSavePanel, NSScreen, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSRect, NSSet,
    NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::backend::{
    BackendError, Clipboard, Display, FileDialogOptions, MessageDialogButton, MessageDialogChoice,
    MessageDialogOptions, Notification, RgbaImage, ServiceResult, SystemTheme,
};
use crate::event::{Point, Rect, UiEvent};
use crate::primitives::dialog::DialogSeverity;
use crate::types::{Color, WidgetId};
use crate::PlatformServices;

/// `NSModalResponseOK` — the user clicked Open / Save.
const NS_MODAL_RESPONSE_OK: isize = 1;

/// Prefix marking a `UNNotificationRequest` identifier as synthetic
/// (generated by [`request_identifier`] because the [`Notification`] had
/// no caller-supplied `tag`), rather than a real tag that should
/// round-trip through [`UiEvent::NotificationActivated`]'s `tag` field.
///
/// A caller-supplied tag that happens to start with the same string is
/// misclassified as synthetic by [`tag_from_request_identifier`] — an
/// accepted, documented corner case rather than a bug to design around.
const SYNTHETIC_TAG_PREFIX: &str = "quadraui-notification-";

/// The `UNNotificationRequest` identifier for a given [`Notification`]:
/// its `tag` verbatim when present (so re-sending the same tag replaces
/// the prior delivered notification), or a fresh synthetic identifier
/// derived from `fresh_id` otherwise (so untagged notifications never
/// collide with each other).
fn request_identifier(tag: Option<&str>, fresh_id: u64) -> String {
    match tag {
        Some(tag) => tag.to_string(),
        None => format!("{SYNTHETIC_TAG_PREFIX}{fresh_id}"),
    }
}

/// Inverse of [`request_identifier`]: recovers the `tag` to report on
/// [`UiEvent::NotificationActivated`] from the `UNNotificationRequest`
/// identifier the delegate callback receives. A synthetic identifier
/// (one this module generated because there was no real tag) reports
/// `None`, matching what [`Notification::tag`] returned when the
/// notification was sent.
fn tag_from_request_identifier(request_id: &str) -> Option<String> {
    if request_id.starts_with(SYNTHETIC_TAG_PREFIX) {
        None
    } else {
        Some(request_id.to_string())
    }
}

/// Maps a `UNNotificationResponse::actionIdentifier()` to the
/// [`UiEvent::NotificationActivated`] `action` field: `None` when the
/// user activated the notification body itself or dismissed it
/// (AppKit's `UNNotificationDefaultActionIdentifier` /
/// `UNNotificationDismissActionIdentifier` sentinels), `Some` with the
/// matching [`WidgetId`] when a specific action button was pressed.
fn activated_action(
    action_identifier: &str,
    default_id: &str,
    dismiss_id: &str,
) -> Option<WidgetId> {
    if action_identifier == default_id || action_identifier == dismiss_id {
        None
    } else {
        Some(WidgetId::new(action_identifier))
    }
}

/// Whether to route [`MacPlatformServices::send_notification`] through
/// `UNUserNotificationCenter` rather than falling back to `osascript`.
/// `UNUserNotificationCenter` requires the process to be a bundled
/// `.app` with a `CFBundleIdentifier` — an unbundled CLI host (no
/// `bundle_identifier`) can't use it and always gets `osascript`.
fn should_use_notification_center(bundle_identifier: Option<&str>) -> bool {
    bundle_identifier.is_some()
}

/// Deterministic `UNNotificationCategory` identifier for a given set of
/// action ids: notifications that declare the same actions, in the same
/// order, share one category, since `setNotificationCategories` replaces
/// rather than merges the whole registered set and
/// [`MacPlatformServices`] needs a stable key to accumulate categories
/// across calls (see `notification_categories`'s field doc). The unit
/// separator (`\u{1f}`) can't appear inside a [`WidgetId`]'s interned
/// string, so it's safe to join on.
fn category_identifier(action_ids: &[&str]) -> String {
    if action_ids.is_empty() {
        "quadraui-category-none".to_string()
    } else {
        format!("quadraui-category:{}", action_ids.join("\u{1f}"))
    }
}

/// Delivers [`UiEvent::NotificationActivated`] events from the
/// `UNUserNotificationCenterDelegate` callback (which can fire on any
/// thread AppKit chooses) into [`MacBackend`][super::backend::MacBackend]'s
/// event queue, nudging the run loop awake via `notify` the same way
/// [`super::backend::MacBackend::waker`] nudges it for `UserPayload`s —
/// see that method's doc for the GCD mechanism `notify` is expected to
/// perform. Cloning a `NotificationEventSink` shares the same queue and
/// `notify` closure; `MacPlatformServices` keeps exactly one, handed to
/// it once by `MacBackend::new`.
#[derive(Clone)]
pub(crate) struct NotificationEventSink {
    notifications: Arc<Mutex<VecDeque<UiEvent>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl NotificationEventSink {
    pub(crate) fn new(
        notifications: Arc<Mutex<VecDeque<UiEvent>>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            notifications,
            notify,
        }
    }

    /// Queue `event` and nudge the run loop awake so
    /// `MacBackend::poll_events`/`wait_events` sees it promptly instead
    /// of waiting for the next unrelated wake.
    pub(crate) fn deliver(&self, event: UiEvent) {
        self.notifications
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
        (self.notify)();
    }
}

/// Per-instance state for [`QuadraNotificationDelegate`].
pub(crate) struct QuadraNotificationDelegateIvars {
    sink: NotificationEventSink,
}

define_class!(
    /// `UNUserNotificationCenterDelegate` implementation: translates a
    /// user activating a delivered notification (or one of its action
    /// buttons) into [`UiEvent::NotificationActivated`] via the ivars'
    /// [`NotificationEventSink`]. Held alive by [`MacPlatformServices`]
    /// for the life of the process — `UNUserNotificationCenter` holds
    /// only a weak reference to its `delegate`.
    //
    // SAFETY:
    // - No `MainThreadOnly` bound: AppKit is free to invoke delegate
    //   callbacks off the main thread, and `NotificationEventSink` is
    //   `Send + Sync` precisely so that's safe here.
    // - No Drop impl; ivars hold an owned `NotificationEventSink` whose
    //   own fields (`Arc`s) drop cleanly when the Obj-C runtime
    //   finalises the instance.
    #[unsafe(super(NSObject))]
    #[ivars = QuadraNotificationDelegateIvars]
    pub(crate) struct QuadraNotificationDelegate;

    unsafe impl NSObjectProtocol for QuadraNotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for QuadraNotificationDelegate {
        /// Fires when the user activates a delivered notification or one
        /// of its action buttons. Must call `completion_handler` exactly
        /// once, or `UNUserNotificationCenter` considers the response
        /// unhandled.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn handle_notification_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let action_identifier = response.actionIdentifier().to_string();
            let request_id = response.notification().request().identifier().to_string();
            // SAFETY: both are read-only `extern "C"` statics AppKit
            // initialises before any delegate callback can fire.
            let default_id = unsafe { UNNotificationDefaultActionIdentifier.to_string() };
            let dismiss_id = unsafe { UNNotificationDismissActionIdentifier.to_string() };
            let action = activated_action(&action_identifier, &default_id, &dismiss_id);
            let tag = tag_from_request_identifier(&request_id);
            self.ivars()
                .sink
                .deliver(UiEvent::NotificationActivated { tag, action });
            completion_handler.call(());
        }
    }
);

impl QuadraNotificationDelegate {
    fn new(sink: NotificationEventSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(QuadraNotificationDelegateIvars { sink });
        unsafe { msg_send![super(this), init] }
    }
}

/// macOS platform services backed by AppKit + `arboard` + shell-out
/// helpers. Constructed by [`crate::macos::MacBackend::new`] and
/// exposed through [`crate::Backend::services`].
pub struct MacPlatformServices {
    clipboard: MacClipboard,
    /// Set once by `MacBackend::new` via [`Self::set_notification_sink`].
    /// `RefCell`-wrapped rather than a constructor argument since
    /// `MacPlatformServices::new()` has no access to the `MacBackend`
    /// fields the sink needs to be built from.
    notification_sink: RefCell<Option<NotificationEventSink>>,
    /// Categories registered with `UNUserNotificationCenter` so far,
    /// keyed by [`category_identifier`]'s output. Accumulated here
    /// rather than registered fresh per call because
    /// `setNotificationCategories` *replaces* the whole registered set —
    /// re-registering only the current notification's category would
    /// silently un-register every other category already delivered and
    /// still pending a user response.
    notification_categories: RefCell<HashMap<String, Retained<UNNotificationCategory>>>,
    /// The one `UNUserNotificationCenterDelegate` installed so far,
    /// kept alive here since `UNUserNotificationCenter::setDelegate`
    /// only holds a weak reference.
    notification_delegate:
        RefCell<Option<Retained<ProtocolObject<dyn UNUserNotificationCenterDelegate>>>>,
    /// Monotonic counter feeding [`request_identifier`]'s synthetic
    /// identifiers for untagged notifications.
    next_notification_id: Cell<u64>,
    /// Whether `requestAuthorizationWithOptions_completionHandler` has
    /// already been called once — asking again on every
    /// `send_notification` call would be noisy and pointless once the
    /// user has answered.
    notification_authorization_requested: Cell<bool>,
}

impl MacPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: MacClipboard::new(),
            notification_sink: RefCell::new(None),
            notification_categories: RefCell::new(HashMap::new()),
            notification_delegate: RefCell::new(None),
            next_notification_id: Cell::new(0),
            notification_authorization_requested: Cell::new(false),
        }
    }

    /// Install the sink notification-activation events are delivered
    /// through. Called once by [`super::backend::MacBackend::new`];
    /// interior-mutable because `MacBackend::new` only has a `&`
    /// reference to `services` by the time the sink is built (see that
    /// call site's own comment for why).
    pub(crate) fn set_notification_sink(&self, sink: NotificationEventSink) {
        *self.notification_sink.borrow_mut() = Some(sink);
    }

    /// `UNUserNotificationCenter` delivery path for
    /// [`Self::send_notification`] once [`should_use_notification_center`]
    /// confirms the process is a bundled `.app`. One-time setup (requesting
    /// authorization, installing [`QuadraNotificationDelegate`]) happens
    /// lazily on first use, guarded by the corresponding `Cell`/`RefCell`
    /// fields; `notification_categories` then accumulates a category per
    /// distinct action set across every call, since
    /// `setNotificationCategories` replaces rather than merges the
    /// registered set. Removing any notification already delivered under
    /// `n`'s request identifier before adding the fresh one gives tagged
    /// notifications deterministic replace-by-tag semantics.
    fn send_notification_via_notification_center(&self, n: &Notification) {
        let center = UNUserNotificationCenter::currentNotificationCenter();

        if !self.notification_authorization_requested.replace(true) {
            let options = UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge;
            let completion = RcBlock::new(move |_granted: Bool, _error: *mut NSError| {});
            center.requestAuthorizationWithOptions_completionHandler(options, &completion);
        }

        if self.notification_delegate.borrow().is_none() {
            if let Some(sink) = self.notification_sink.borrow().clone() {
                let delegate = QuadraNotificationDelegate::new(sink);
                let delegate: Retained<ProtocolObject<dyn UNUserNotificationCenterDelegate>> =
                    ProtocolObject::from_retained(delegate);
                center.setDelegate(Some(&delegate));
                *self.notification_delegate.borrow_mut() = Some(delegate);
            }
        }

        let action_ids: Vec<&str> = n.actions().iter().map(|(id, _)| id.as_str()).collect();
        let category_id = category_identifier(&action_ids);
        if !self
            .notification_categories
            .borrow()
            .contains_key(&category_id)
        {
            let ns_actions: Vec<Retained<UNNotificationAction>> = n
                .actions()
                .iter()
                .map(|(id, label)| {
                    UNNotificationAction::actionWithIdentifier_title_options(
                        &NSString::from_str(id.as_str()),
                        &NSString::from_str(label),
                        UNNotificationActionOptions::empty(),
                    )
                })
                .collect();
            let actions_array = NSArray::from_retained_slice(&ns_actions);
            let intent_identifiers = NSArray::<NSString>::from_slice(&[]);
            let category =
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(&category_id),
                    &actions_array,
                    &intent_identifiers,
                    UNNotificationCategoryOptions::empty(),
                );
            self.notification_categories
                .borrow_mut()
                .insert(category_id.clone(), category);
            let all_categories: Vec<Retained<UNNotificationCategory>> = self
                .notification_categories
                .borrow()
                .values()
                .cloned()
                .collect();
            center.setNotificationCategories(&NSSet::from_retained_slice(&all_categories));
        }

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&n.title));
        content.setBody(&NSString::from_str(&n.body));
        content.setCategoryIdentifier(&NSString::from_str(&category_id));
        if !n.is_silent() {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }

        let fresh_id = self.next_notification_id.get();
        self.next_notification_id.set(fresh_id + 1);
        let request_id = request_identifier(n.tag(), fresh_id);

        let to_remove = NSArray::from_retained_slice(&[NSString::from_str(&request_id)]);
        center.removeDeliveredNotificationsWithIdentifiers(&to_remove);

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&request_id),
            &content,
            None,
        );
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
}

impl Default for MacPlatformServices {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for MacPlatformServices {
    fn clipboard(&self) -> &dyn Clipboard {
        &self.clipboard
    }

    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_file_open_dialog must be called from the main thread");
        // SAFETY: AppKit panels are constructed and driven exclusively
        // on the main thread; MainThreadMarker enforces that.
        unsafe {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            configure_panel(&panel, &opts);
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_file_save_dialog must be called from the main thread");
        // SAFETY: same as `show_file_open_dialog` above.
        unsafe {
            let panel = NSSavePanel::savePanel(mtm);
            configure_panel(&panel, &opts);
            if let Some(ref name) = opts.initial_filename {
                panel.setNameFieldStringValue(&NSString::from_str(name));
            }
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    /// A two-line variant of `show_file_open_dialog` above —
    /// the same `NSOpenPanel`, with the choose-directories/choose-files
    /// pair flipped.
    fn show_folder_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_folder_open_dialog must be called from the main thread");
        // SAFETY: same as `show_file_open_dialog` above.
        unsafe {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            configure_panel(&panel, &opts);
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    /// `NSAlert` — the macOS native dialog implementation.
    /// `native_button_order` picks which order to add buttons in (macOS
    /// convention: most-prominent / default button added first, since
    /// `NSAlert` lays out the first-added button trailing/rightmost — see
    /// `addButtonWithTitle:`'s docs); `severity_to_alert_style` maps
    /// `DialogSeverity` onto `NSAlertStyle`. Both are free functions below
    /// so the mapping is unit-testable without presenting anything — the
    /// modal `runModal()` call itself still needs a human on a real Mac.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        // Graceful degrade rather than `expect`'s hard panic: returns `None` when
        // called off the main thread instead of aborting the test. Rust's default
        // test harness spawns a fresh OS thread per `#[test]` fn, so a test
        // that reaches this method off the main thread must see `None` — the same
        // "no native alert facility" shape callers already get from
        // `BackendCaps::native_dialogs == false`. `NSAlert`'s methods below
        // (unlike `NSOpenPanel`/`NSSavePanel`'s `URL()` and this module's own
        // `url_to_path`) are all safe AppKit wrappers — `MainThreadMarker` still
        // gates *construction* (`NSAlert::new` takes it by value), which is what
        // actually enforces the main-thread requirement here.
        let mtm = MainThreadMarker::new()?;
        let order = native_button_order(&opts.buttons);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&opts.title));
        alert.setInformativeText(&NSString::from_str(&opts.body));
        alert.setAlertStyle(severity_to_alert_style(opts.severity));
        for &i in &order {
            let button = &opts.buttons[i];
            let ns_button = alert.addButtonWithTitle(&NSString::from_str(&button.label));
            if let Some(key) = key_equivalent_for_button(button) {
                ns_button.setKeyEquivalent(&NSString::from_str(key));
            }
        }
        let response = alert.runModal();
        let idx = usize::try_from(response - NSAlertFirstButtonReturn).ok()?;
        let orig = *order.get(idx)?;
        Some(opts.buttons[orig].id)
    }

    /// Routes through `UNUserNotificationCenter` when the process is a
    /// bundled `.app` ([`should_use_notification_center`]); otherwise
    /// falls back to `osascript -e 'display notification ...'`, where
    /// only `is_silent()` of the `Notification` fields has anywhere to
    /// go (`display notification` has no `icon`/`actions`/`tag`
    /// parameters). [`crate::desktop::display_notification_script`] is
    /// the pure logic half of that fallback so the AppleScript text
    /// itself is unit-testable without a live mac; only the
    /// `Command::spawn` below needs one. Shared with `tui::services` for
    /// its own macOS notification fallback leg.
    fn send_notification(&self, n: Notification) {
        let bundle_identifier = NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|s| s.to_string());
        if should_use_notification_center(bundle_identifier.as_deref()) {
            self.send_notification_via_notification_center(&n);
            return;
        }
        let script = crate::desktop::display_notification_script(&n.title, &n.body, n.is_silent());
        let _ = Command::new("osascript").arg("-e").arg(&script).spawn();
    }

    fn open_url(&self, url: &str) {
        let _ = self.open_url_result(url);
    }

    /// Fallible twin of [`Self::open_url`], via
    /// [`crate::desktop::open_with_default`] — reports whether `open`
    /// genuinely spawned instead of `open_url`'s discarded outcome.
    /// Before this override, `open_url_result` fell to
    /// [`crate::backend::PlatformServices::open_url_result`]'s default
    /// (`open_url` then always `Ok(())`), so a `Command::spawn` failure
    /// (e.g. `open` missing from `$PATH`) was silently reported as
    /// success.
    fn open_url_result(&self, url: &str) -> ServiceResult<()> {
        crate::desktop::open_with_default(std::ffi::OsStr::new(url))
    }

    /// `NSWorkspace::activateFileViewerSelectingURLs` on `path`'s
    /// `file://` URL — "Reveal in Finder." `path.is_dir()`
    /// decides `fileURLWithPath:isDirectory:`'s bool the same way
    /// `configure_panel`'s directory-URL construction would need to, were
    /// it not always `true` there (a directory *chooser*'s initial
    /// folder); here `path` can be either a file or a directory, so this
    /// checks rather than assuming.
    fn reveal_in_file_manager(&self, path: &Path) -> ServiceResult<()> {
        let Some(path_str) = path.to_str() else {
            return Err(BackendError::PlatformFailure {
                context: "reveal_in_file_manager: path is not valid UTF-8".to_string(),
            });
        };
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path_str), path.is_dir());
        let urls = NSArray::from_retained_slice(&[url]);
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
        Ok(())
    }

    /// `open <path>` via [`crate::desktop::open_with_default`] — the same
    /// call [`Self::open_url_result`] makes. macOS's `open(1)` already
    /// accepts a filesystem path exactly as readily as a URL (it's the
    /// command-line front end to `NSWorkspace.open(_:)`), so there is nothing
    /// path-specific to add beyond handing it a path instead of a URL string.
    fn open_path(&self, path: &Path) -> ServiceResult<()> {
        crate::desktop::open_with_default(path.as_os_str())
    }

    /// [`crate::desktop::move_to_trash`] — see that
    /// function's doc for why every backend, macOS included, shares this
    /// one `trash`-crate-backed implementation rather than hand-rolling
    /// `NSFileManager::trashItemAtURL_resultingItemURL_error` here.
    fn move_to_trash(&self, path: &Path) -> ServiceResult<()> {
        crate::desktop::move_to_trash(path)
    }

    /// `NSBeep` — a bare AppKit C function, not a method on
    /// any object, so unlike every other AppKit call in this module it
    /// needs no `MainThreadMarker`/receiver of its own.
    fn beep(&self) -> ServiceResult<()> {
        NSBeep();
        Ok(())
    }

    /// `NSApp.effectiveAppearance` for light/dark,
    /// `NSWorkspace::accessibilityDisplayShouldIncreaseContrast` for high
    /// contrast, `NSColor::controlAccentColor` for the accent colour.
    /// Reads `effectiveAppearance().name()` and checks it for `"Dark"`
    /// rather than building an `NSArray` and calling
    /// `NSAppearance::bestMatchFromAppearancesWithNames` — for a one-shot
    /// query like this, the name itself already carries the answer, and every
    /// system appearance name contains the substring unambiguously.
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
        let appearance_name = NSApplication::sharedApplication(mtm)
            .effectiveAppearance()
            .name()
            .to_string();
        let high_contrast =
            NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
        Ok(system_theme_from_mac_appearance(
            &appearance_name,
            high_contrast,
            mac_accent_color(),
        ))
    }

    /// `NSScreen::screens()` — `frame()`/`visibleFrame()` map
    /// straight to [`Display::bounds`]/[`Display::work_area`], and
    /// `backingScaleFactor()` to [`Display::scale`]. `screens()[0]` is
    /// always the screen containing the menu bar (documented AppKit
    /// behaviour) — used as [`Display::primary`], distinct from
    /// `NSScreen::mainScreen()`, which tracks the *key window*'s screen,
    /// not the platform's primary one.
    ///
    /// `Err(BackendError::Unsupported)` only off the main thread (no
    /// `MainThreadMarker`) — same guard [`Self::system_theme`] uses.
    fn displays(&self) -> ServiceResult<Vec<Display>> {
        let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
        let screens = NSScreen::screens(mtm);
        let count = screens.count();
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let screen = screens.objectAtIndex(i);
            out.push(Display {
                bounds: rect_from_ns_rect(screen.frame()),
                work_area: rect_from_ns_rect(screen.visibleFrame()),
                scale: screen.backingScaleFactor() as f32,
                primary: i == 0,
            });
        }
        Ok(out)
    }

    /// `NSEvent::mouseLocation()` — a bare AppKit class
    /// method, like [`Self::beep`]'s `NSBeep`, needing no
    /// `MainThreadMarker`/receiver of its own. Returns coordinates in the
    /// same bottom-left-origin, y-up screen space [`Self::displays`]'s
    /// `bounds`/`work_area` do — both come from the same `NSScreen`/
    /// `NSEvent` global-coordinate system, so the two stay directly
    /// comparable (testing which [`Display`] contains the cursor is a
    /// plain point-in-rect check, no coordinate flip needed).
    fn cursor_screen_point(&self) -> ServiceResult<Point> {
        let p = NSEvent::mouseLocation();
        Ok(Point::new(p.x as f32, p.y as f32))
    }

    fn platform_name(&self) -> &'static str {
        "macos"
    }
}

/// Convert an AppKit `NSRect` (`CGRect`: `f64` origin + size) to
/// [`Rect`]'s `f32` shape — shared by every [`Display`] field
/// [`MacPlatformServices::displays`] reads off `NSScreen`.
fn rect_from_ns_rect(r: NSRect) -> Rect {
    Rect::new(
        r.origin.x as f32,
        r.origin.y as f32,
        r.size.width as f32,
        r.size.height as f32,
    )
}

/// Pure mapping from an `NSAppearance` name plus the two other
/// already-read signals to [`SystemTheme`] — split out from
/// `system_theme` so the name-matching logic is unit-testable without a
/// live `NSApplication` (mirrors this module's `native_button_order`/
/// `severity_to_alert_style`-style helpers).
fn system_theme_from_mac_appearance(
    appearance_name: &str,
    high_contrast: bool,
    accent: Option<Color>,
) -> SystemTheme {
    SystemTheme {
        dark: appearance_name.contains("Dark"),
        accent,
        high_contrast,
    }
}

/// `NSColor::controlAccentColor`'s RGB components, converted to
/// [`Color`]. `getRed:green:blue:alpha:` raises `NSInvalidArgumentException`
/// if the receiver isn't already in an RGB-compatible colour space, and an
/// ObjC exception unwinding across this Rust/FFI boundary is UB (likely an
/// abort) — Apple's docs don't guarantee `controlAccentColor` (a dynamic,
/// catalog-backed system colour) is pre-resolved to RGB in every runtime
/// context, so this defensively runs it through `colorUsingColorSpace:`
/// first rather than assuming it. `colorUsingColorSpace:` returns `None`
/// only when the conversion itself is impossible (no live colour-management
/// pipeline), which — like the rest of this crate's platform-services
/// layer — is treated as "feature unavailable" rather than an error.
fn mac_accent_color() -> Option<Color> {
    let color = NSColor::controlAccentColor();
    let rgb_color = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let (mut r, mut g, mut b, mut a) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    // SAFETY: four valid, non-null `f64` out-pointers, matching
    // `getRed:green:blue:alpha:`'s documented safety requirement. Calling
    // it on `rgb_color` rather than `color` is what makes this safe from
    // the ObjC-exception hazard described above: `colorUsingColorSpace:`
    // above already guarantees an RGB-compatible receiver.
    unsafe {
        rgb_color.getRed_green_blue_alpha(&mut r, &mut g, &mut b, &mut a);
    }
    Some(Color::rgba(
        unit_to_u8(r),
        unit_to_u8(g),
        unit_to_u8(b),
        unit_to_u8(a),
    ))
}

/// Convert a `0.0..=1.0` colour component to a `0..=255` byte, clamping
/// out-of-range input rather than wrapping/panicking (`as u8` on a
/// negative or `NaN` float is technically defined since Rust 1.45's
/// saturating float casts, but `clamp` here makes the intent explicit
/// rather than relying on that cast behaviour).
fn unit_to_u8(component: f64) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Apply common options (message, initial directory, file-type filters)
/// to an `NSSavePanel` — also covers `NSOpenPanel`, which inherits all
/// of these setters from `NSSavePanel`.
///
/// # Safety
///
/// Must be called from the main thread; `panel` must be a valid panel
/// retrieved on this same thread via `openPanel:` / `savePanel:`.
unsafe fn configure_panel(panel: &NSSavePanel, opts: &FileDialogOptions) {
    if let Some(ref title) = opts.title {
        panel.setMessage(Some(&NSString::from_str(title)));
    }
    if let Some(ref dir) = opts.initial_dir {
        if let Some(dir_str) = dir.to_str() {
            let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(dir_str), true);
            panel.setDirectoryURL(Some(&url));
        }
    }
    let exts: Vec<_> = opts
        .filters
        .iter()
        .flat_map(|(_, e)| e.iter())
        .map(|ext| NSString::from_str(ext))
        .collect();
    if !exts.is_empty() {
        // `NSArray::from_slice` requires `T: IsRetainable`, which NSString doesn't
        // satisfy (it has an `NSMutableString` subclass). `from_retained_slice`
        // takes already-`Retained` handles and sidesteps that bound.
        // `setAllowedFileTypes:` is deprecated in favour of
        // `setAllowedContentTypes:` (UTType), but that requires the
        // UniformTypeIdentifiers framework which objc2 doesn't yet wrap. The
        // legacy API still works on macOS 11–15.
        let arr = NSArray::from_retained_slice(&exts);
        // `setAllowedFileTypes:` is deprecated in favour of
        // `setAllowedContentTypes:` (UTType), but that requires the
        // UniformTypeIdentifiers framework which objc2 doesn't yet
        // wrap. The legacy API still works on macOS 11–15.
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&arr));
    }
}

/// Convert an `NSURL` back to a Rust `PathBuf`. Returns `None` if the
/// URL is missing or non-file-scheme.
///
/// # Safety
///
/// `url` must be a valid `NSURL` retrieved on the main thread.
unsafe fn url_to_path(url: Option<&NSURL>) -> Option<PathBuf> {
    let path = url?.path()?;
    Some(PathBuf::from(path.to_string()))
}

/// Order `buttons` for `NSAlert::addButtonWithTitle:`:
/// `NSAlert` lays out the first-added button trailing/rightmost (its own
/// docs: "arranged from trailing-to-leading edge... Buttons should be
/// added from most-to-least prominent"), so the default/primary button
/// goes first, any cancel button goes second, and everything else keeps
/// its declared relative order after that. Returns indices into
/// `buttons` — `show_message_dialog` maps `runModal`'s chosen index back
/// through this same slice to recover the original
/// [`MessageDialogButton::id`], mirroring
/// `gtk::services::hig_button_order`'s job for the GTK4 `AlertDialog`.
///
/// A button with both `is_default` and `is_cancel` set (a single-button
/// "OK" alert) lands in the default slot here — `show_message_dialog`
/// also binds it to Return, not Escape, when assigning the key
/// equivalent (see that method's doc comment), so which bucket this
/// function sorts it into only affects visual position, not behaviour.
fn native_button_order(buttons: &[MessageDialogButton]) -> Vec<usize> {
    let mut default_idx = None;
    let mut cancel_idx = None;
    let mut rest = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        if b.is_default && default_idx.is_none() {
            default_idx = Some(i);
        } else if b.is_cancel && cancel_idx.is_none() {
            cancel_idx = Some(i);
        } else {
            rest.push(i);
        }
    }
    let mut order = Vec::with_capacity(buttons.len());
    order.extend(default_idx);
    order.extend(cancel_idx);
    order.extend(rest);
    order
}

/// Pick the `NSButton::setKeyEquivalent` string for `button`, or `None`
/// to leave `NSAlert`'s own default untouched.
///
/// `setKeyEquivalent` only ever holds one string, so a button that is
/// both `is_default` and `is_cancel` (a single-button "OK" alert — the
/// most common message-dialog shape in this codebase) can only be bound
/// to one of Return/Escape here. `is_default` wins: `NSAlert`'s own
/// native default (its docs: "By default, the first button has a key
/// equivalent of Return...") already gives such a button Return for
/// free when `setKeyEquivalent` is never called, and both sibling
/// backends bind *both* keys to this button —
/// `gtk::services::hig_button_order`'s doc points `set_cancel_button`
/// *and* `set_default_button` at it, and
/// `win::services::assign_button_ids` gives it `TASKDIALOG_IDCANCEL`
/// *and* looks it up as `pszDefaultButton` — so Return is the one to
/// keep when only one of the two can survive on this platform.
fn key_equivalent_for_button(button: &MessageDialogButton) -> Option<&'static str> {
    if button.is_default {
        Some("\r")
    } else if button.is_cancel {
        Some("\u{1b}")
    } else {
        None
    }
}

/// Map [`DialogSeverity`] onto `NSAlertStyle` — shares
/// `win::services::win_show_message_dialog`'s `icon` match's "`Question`
/// has no dedicated native icon" gap: `NSAlertStyle` (like
/// `TaskDialogIndirect`'s icon set) has no question-mark style, so
/// `Question` degrades to a neutral style, matching [`DialogSeverity`]'s
/// own "`None` = neutral" doc. Unlike the Windows arm, though,
/// `NSAlertStyle` also has no dedicated "no icon" state the way
/// `PCWSTR::null()` gives Windows one, so `Info` is folded into the same
/// `Informational` bucket as `Question`/`None` here rather than getting
/// its own arm the way `TD_INFORMATION_ICON` does on Windows.
fn severity_to_alert_style(severity: Option<DialogSeverity>) -> NSAlertStyle {
    match severity {
        Some(DialogSeverity::Error) => NSAlertStyle::Critical,
        Some(DialogSeverity::Warning) => NSAlertStyle::Warning,
        Some(DialogSeverity::Info) | Some(DialogSeverity::Question) | None => {
            NSAlertStyle::Informational
        }
    }
}

/// System clipboard via `arboard` (NSPasteboard under the hood on
/// macOS). Held for the lifetime of `MacPlatformServices` so the
/// pasteboard handle outlives any cached connection state.
pub struct MacClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl MacClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
        }
    }
}

/// Context string used when there is no live `arboard::Clipboard` handle
/// at all (construction failed — e.g. no `WindowServer` session) rather
/// than a native call on a real handle failing. Shared by
/// [`MacClipboard`]'s `Clipboard` methods below.
const NO_PASTEBOARD_CONTEXT: &str = "arboard::Clipboard::new (no NSPasteboard session)";

/// Map an `arboard::Error` from a named native call into a
/// [`BackendError::PlatformFailure`] — the one
/// implementation shared with `gtk::services`; see
/// [`crate::desktop::map_arboard_error`]'s own doc for the full
/// rationale.
use crate::desktop::map_arboard_error;

impl Clipboard for MacClipboard {
    fn read_text(&self) -> Option<String> {
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }

    /// Decoded RGBA pixels via `arboard::Clipboard::get_image` — the
    /// `image-data`-featured leg of the same `arboard` handle
    /// `read_text`/`write_text` already share.
    fn read_image(&self) -> ServiceResult<RgbaImage> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_PASTEBOARD_CONTEXT.to_string(),
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
            context: NO_PASTEBOARD_CONTEXT.to_string(),
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
            context: NO_PASTEBOARD_CONTEXT.to_string(),
        })?;
        cb.set_html(html, Some(alt_text))
            .map_err(|e| map_arboard_error("arboard::set_html", e))
    }

    fn read_file_list(&self) -> ServiceResult<Vec<PathBuf>> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_PASTEBOARD_CONTEXT.to_string(),
        })?;
        cb.get()
            .file_list()
            .map_err(|e| map_arboard_error("arboard::get().file_list()", e))
    }

    fn clear(&self) -> ServiceResult<()> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: NO_PASTEBOARD_CONTEXT.to_string(),
        })?;
        cb.clear()
            .map_err(|e| map_arboard_error("arboard::clear", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_macos() {
        let svc = MacPlatformServices::new();
        assert_eq!(svc.platform_name(), "macos");
    }

    /// `NSBeep` — a bare AppKit C function with no receiver
    /// and no window/UI object graph to touch, so unlike
    /// `show_message_dialog`'s `NSAlert::runModal` above this is safe to
    /// call from a spawned test thread without a `MainThreadMarker`.
    /// `reveal_in_file_manager`/`open_path` aren't covered by an
    /// automated test here for the same reason `open_url` never has been
    /// in this module: both have a real, visible side effect (a Finder
    /// window opens; the default app for a file launches) that a test
    /// suite shouldn't trigger — manual smoke-testing only, same posture
    /// as `open_url`. `move_to_trash` is covered once, backend-neutrally,
    /// by `crate::desktop`'s own real round-trip test.
    #[test]
    fn beep_reports_success() {
        let svc = MacPlatformServices::new();
        assert_eq!(svc.beep(), Ok(()));
    }

    /// `displays` needs a `MainThreadMarker` the same way
    /// `system_theme`/`show_message_dialog` above do, so off the spawned
    /// thread `cargo test` runs every `#[test]` fn on, it reports
    /// `Unsupported` rather than panicking — same shape
    /// `show_message_dialog_off_main_thread_returns_none_not_panic`
    /// pins for that method.
    #[test]
    fn displays_off_main_thread_returns_unsupported_not_panic() {
        let svc = MacPlatformServices::new();
        assert_eq!(svc.displays(), Err(BackendError::Unsupported));
    }

    /// `NSEvent::mouseLocation()` — a bare AppKit class
    /// method with no receiver, like `NSBeep` above, so this is safe to
    /// call from a spawned test thread without a `MainThreadMarker` and
    /// always reports a real point.
    #[test]
    fn cursor_screen_point_reports_a_point() {
        let svc = MacPlatformServices::new();
        assert!(svc.cursor_screen_point().is_ok());
    }

    /// `open_url_result` reports a real failure when the platform opener
    /// can't even launch, instead of silently discarding the outcome.
    /// `$PATH` is temporarily redirected (guarded by
    /// [`crate::desktop::PATH_OVERRIDE_TEST_LOCK`], and always restored
    /// via a drop guard even on panic) to a directory containing no `open`
    /// binary at all, so the real `Command::new("open").spawn()` genuinely
    /// fails (`ENOENT`) rather than launching anything.
    #[test]
    fn open_url_result_reports_failure_when_the_opener_is_missing() {
        let _guard = crate::desktop::PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let original_path = std::env::var_os("PATH");
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
        let _restore = RestorePath(original_path);
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        let svc = MacPlatformServices::new();
        assert!(svc.open_url_result("https://example.com/1087").is_err());
    }

    // ── Clipboard image/html/file-list/clear ───────────────────────────

    /// Real `NSPasteboard` round trip through every method added in one
    /// `#[test]` fn rather than several: `cargo test` runs `#[test]` fns in
    /// parallel by default, and separate fns would fight over the one real
    /// systemwide pasteboard. Skips gracefully on a runner with no live
    /// `NSPasteboard` session at all, rather than failing the whole crate's
    /// test run over an environment gap this test isn't trying to cover.
    ///
    /// This fn takes the same lock as [`crate::macos::lock_real_pasteboard`]
    /// to avoid races with other real-pasteboard tests.
    #[allow(clippy::print_stderr)]
    #[test]
    fn clipboard_image_html_file_list_and_clear_round_trip() {
        let _pasteboard = crate::macos::lock_real_pasteboard();
        let svc = MacPlatformServices::new();
        let cb = svc.clipboard();

        // write_image → read_image: dimensions and pixel-buffer length
        // round-trip even though NSPasteboard may re-encode the bytes
        // internally (TIFF) — this doesn't assert byte-for-byte pixel
        // equality, only shape.
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
            eprintln!("skipping: no live NSPasteboard session in this environment ({e:?})");
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

        // write_html: no read-back method exists (see `ClipboardFormat`'s
        // doc for why) — just assert it succeeds.
        cb.write_html("<b>hi</b>", "hi").expect(
            "write_html should succeed once write_image already proved the pasteboard is live",
        );

        // read_file_list: `Clipboard` has no `write_file_list` (only
        // `read_file_list` — again see `ClipboardFormat`'s doc), so seed
        // the pasteboard directly via a second `arboard::Clipboard`
        // handle, then confirm `MacClipboard::read_file_list` reads back
        // what was set.
        let tmp = std::env::temp_dir().join("quadraui-954-clipboard-test-file.txt");
        std::fs::write(&tmp, b"quadraui#954").expect("write temp file");
        arboard::Clipboard::new()
            .expect("a second arboard handle should also see the live pasteboard")
            .set()
            .file_list(&[&tmp])
            .expect("seeding the pasteboard with a file list should succeed");
        let files = cb
            .read_file_list()
            .expect("read_file_list should see the seeded file");
        // Canonicalize before comparing: NSPasteboard round-trips the
        // path through `/private/var/...` even though `std::env::temp_dir()`
        // returns the `/var/...` symlink on macOS — a path-spelling
        // artifact of the OS pasteboard, not something `read_file_list`
        // got wrong.
        assert_eq!(
            files
                .iter()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
                .collect::<Vec<_>>(),
            vec![tmp.canonicalize().expect("temp file should exist")]
        );
        let _ = std::fs::remove_file(&tmp);

        // clear: removes everything, regardless of format.
        cb.write_text("some text #954");
        assert_eq!(cb.read_text(), Some("some text #954".to_string()));
        cb.clear()
            .expect("clear should succeed once the pasteboard has proven live");
        assert_eq!(cb.read_text(), None);
    }

    // `applescript_escape`/`display_notification_script` tests moved to
    // `crate::desktop::display_notification_script_tests` alongside the
    // functions themselves.

    // ── system_theme_from_mac_appearance / unit_to_u8 ───────────────────

    #[test]
    fn dark_aqua_appearance_reports_dark() {
        let theme = system_theme_from_mac_appearance("NSAppearanceNameDarkAqua", false, None);
        assert!(theme.dark);
        assert!(!theme.high_contrast);
        assert_eq!(theme.accent, None);
    }

    #[test]
    fn aqua_appearance_reports_light() {
        let theme = system_theme_from_mac_appearance("NSAppearanceNameAqua", false, None);
        assert!(!theme.dark);
    }

    #[test]
    fn high_contrast_flag_is_passed_through_not_rederived_from_name() {
        // The appearance name alone already implies high contrast, but
        // `high_contrast` is a caller-supplied signal
        // (`NSWorkspace::accessibilityDisplayShouldIncreaseContrast`),
        // not derived from the name — this pins that the function trusts
        // its caller's flag rather than re-deriving it from the name
        // string.
        let theme = system_theme_from_mac_appearance(
            "NSAppearanceNameAccessibilityHighContrastDarkAqua",
            true,
            None,
        );
        assert!(theme.dark);
        assert!(theme.high_contrast);
    }

    #[test]
    fn accent_color_passes_through_unchanged() {
        let accent = Some(Color::rgb(0, 122, 255));
        let theme = system_theme_from_mac_appearance("NSAppearanceNameAqua", false, accent);
        assert_eq!(theme.accent, accent);
    }

    #[test]
    fn unit_to_u8_maps_the_full_range() {
        assert_eq!(unit_to_u8(0.0), 0);
        assert_eq!(unit_to_u8(1.0), 255);
        assert_eq!(unit_to_u8(0.5), 128);
    }

    #[test]
    fn unit_to_u8_clamps_out_of_range_input() {
        assert_eq!(unit_to_u8(-1.0), 0);
        assert_eq!(unit_to_u8(2.0), 255);
    }

    fn msg_btn(id: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
        MessageDialogButton {
            id: crate::types::WidgetId::new(id),
            label: id.to_string(),
            is_default,
            is_cancel,
        }
    }

    #[test]
    fn native_button_order_puts_default_first_and_cancel_second() {
        let buttons = [
            msg_btn("middle", false, false),
            msg_btn("cancel", false, true),
            msg_btn("default", true, false),
        ];
        assert_eq!(native_button_order(&buttons), vec![2, 1, 0]);
    }

    #[test]
    fn native_button_order_preserves_relative_order_of_the_rest() {
        let buttons = [
            msg_btn("a", false, false),
            msg_btn("b", false, false),
            msg_btn("c", false, false),
        ];
        assert_eq!(native_button_order(&buttons), vec![0, 1, 2]);
    }

    #[test]
    fn native_button_order_single_button_both_default_and_cancel() {
        // A single "OK" button often carries both flags. It should still
        // appear exactly once (in the default slot), not twice.
        let buttons = [msg_btn("ok", true, true)];
        assert_eq!(native_button_order(&buttons), vec![0]);
    }

    #[test]
    fn native_button_order_no_default_or_cancel_keeps_declared_order() {
        let buttons = [msg_btn("a", false, false), msg_btn("b", false, false)];
        assert_eq!(native_button_order(&buttons), vec![0, 1]);
    }

    #[test]
    fn key_equivalent_for_button_default_only_binds_return() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("ok", true, false)),
            Some("\r")
        );
    }

    #[test]
    fn key_equivalent_for_button_cancel_only_binds_escape() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("cancel", false, true)),
            Some("\u{1b}")
        );
    }

    #[test]
    fn key_equivalent_for_button_neither_binds_nothing() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("middle", false, false)),
            None
        );
    }

    #[test]
    fn key_equivalent_for_button_both_default_and_cancel_binds_return_not_escape() {
        // Regression test: a single "OK" button
        // that carries both flags — the most common message-dialog
        // shape — MUST get Return, not just Escape, or pressing Enter
        // does nothing and the dialog is keyboard-unresponsive on its
        // most common shape.
        assert_eq!(
            key_equivalent_for_button(&msg_btn("ok", true, true)),
            Some("\r")
        );
    }

    #[test]
    fn severity_to_alert_style_maps_error_and_warning_distinctly() {
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Error)),
            NSAlertStyle::Critical
        );
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Warning)),
            NSAlertStyle::Warning
        );
    }

    #[test]
    fn severity_to_alert_style_defaults_question_and_none_to_informational() {
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Info)),
            NSAlertStyle::Informational
        );
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Question)),
            NSAlertStyle::Informational
        );
        assert_eq!(severity_to_alert_style(None), NSAlertStyle::Informational);
    }

    /// Off the main thread — which is exactly where Rust's default per-test-fn
    /// thread puts this test — `show_message_dialog` must degrade to `None`
    /// rather than panic, the same contract the doc comment on
    /// `show_message_dialog` describes. No `MainThreadMarker` is available
    /// off the main thread, so the method returns `None`.
    #[test]
    fn show_message_dialog_off_main_thread_returns_none_not_panic() {
        let svc = MacPlatformServices::new();
        let opts = MessageDialogOptions {
            title: "Title".to_string(),
            body: "Body".to_string(),
            buttons: vec![msg_btn("ok", true, true)],
            severity: None,
        };
        assert!(svc.show_message_dialog(opts).is_none());
    }

    // ── Notification-Center pure helpers ────────────────────────────────

    #[test]
    fn request_identifier_uses_tag_verbatim_when_present() {
        assert_eq!(
            request_identifier(Some("download-complete"), 7),
            "download-complete"
        );
    }

    #[test]
    fn request_identifier_falls_back_to_synthetic_id_without_a_tag() {
        let id = request_identifier(None, 7);
        assert_eq!(id, "quadraui-notification-7");
        assert!(id.starts_with(SYNTHETIC_TAG_PREFIX));
    }

    #[test]
    fn request_identifier_round_trips_through_tag_from_request_identifier() {
        let tagged = request_identifier(Some("download-complete"), 0);
        assert_eq!(
            tag_from_request_identifier(&tagged),
            Some("download-complete".to_string())
        );

        let synthetic = request_identifier(None, 42);
        assert_eq!(tag_from_request_identifier(&synthetic), None);
    }

    #[test]
    fn activated_action_reports_none_for_default_and_dismiss_identifiers() {
        assert_eq!(activated_action("default", "default", "dismiss"), None);
        assert_eq!(activated_action("dismiss", "default", "dismiss"), None);
    }

    #[test]
    fn activated_action_reports_the_widget_id_for_a_real_action() {
        assert_eq!(
            activated_action("snooze", "default", "dismiss"),
            Some(WidgetId::new("snooze"))
        );
    }

    #[test]
    fn should_use_notification_center_requires_a_bundle_identifier() {
        assert!(should_use_notification_center(Some("com.example.app")));
        assert!(!should_use_notification_center(None));
    }

    #[test]
    fn category_identifier_is_stable_for_the_same_action_set() {
        assert_eq!(
            category_identifier(&["snooze", "dismiss"]),
            category_identifier(&["snooze", "dismiss"])
        );
    }

    #[test]
    fn category_identifier_distinguishes_action_sets_and_order() {
        let none = category_identifier(&[]);
        let one = category_identifier(&["snooze"]);
        let two = category_identifier(&["snooze", "dismiss"]);
        let reordered = category_identifier(&["dismiss", "snooze"]);

        assert_ne!(none, one);
        assert_ne!(one, two);
        assert_ne!(two, reordered);
    }
}
