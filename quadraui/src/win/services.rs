//! Win-GUI implementation of [`quadraui::PlatformServices`].
//!
//! - **Clipboard** — Win32 `OpenClipboard`/`GetClipboardData`/
//!   `SetClipboardData`, with every payload backed by a
//!   `GlobalAlloc(GMEM_MOVEABLE)` block per the classic Win32 clipboard
//!   contract (the system takes ownership of the block once
//!   `SetClipboardData` succeeds — it must never be `GlobalFree`d by the
//!   caller after that point). `read_text`/`write_text` use
//!   `CF_UNICODETEXT`; `read_image` decodes `CF_DIBV5` (preferred,
//!   alpha-capable) or classic `CF_DIB` (24bpp, always opaque) and
//!   `write_image` always encodes `CF_DIBV5` — see `decode_dib`/
//!   `encode_dibv5`'s own docs for the exact pixel-format subset;
//!   `write_html` registers and writes `CF_HTML` ("HTML Format")'s
//!   byte-offset text header (see `build_cf_html`'s own doc) alongside a
//!   `CF_UNICODETEXT` plain-text fallback in the same open/close session.
//! - **File dialogs** — the Vista+ common file dialog: `IFileOpenDialog`
//!   / `IFileSaveDialog` via COM (`CoCreateInstance`), parented to the
//!   live `HWND` [`WinBackend::attach_surface`][super::backend::WinBackend::attach_surface]
//!   hands to [`WinPlatformServices::set_window`] — same role as
//!   `GtkPlatformServices::window`.
//! - **Folder dialog** — the same `IFileOpenDialog`, with
//!   `FOS_PICKFOLDERS` set on its options: Windows has no separate
//!   directory-picker COM object, so this is the documented way to turn
//!   the file-open dialog into one.
//! - **Notifications** — WinRT `ToastNotificationManager`/
//!   `ToastNotification`: `send_notification` builds a `ToastGeneric` XML
//!   document from the `Notification`'s title/body/action buttons (see
//!   [`build_toast_xml`]), registers it under this process's
//!   AppUserModelID (set once in `win::run` at window creation —
//!   `ToastNotificationManager::CreateToastNotifierWithId` fails without
//!   one), and shows it through a `ToastNotifier`. `tag()` maps onto the
//!   toast's own `Tag` property, so Windows itself replaces a prior toast
//!   that shares a tag rather than stacking a new one in Action Center.
//!   `silent` maps onto `ToastGeneric`'s own `<audio silent="true"/>`
//!   element (see [`build_toast_xml`]). `icon` has no straightforward
//!   WinRT toast equivalent on this API surface (an icon needs a file URI
//!   the toast renderer can read back out-of-process, which
//!   `ImageSource::Bytes` doesn't carry) and is dropped, same as
//!   `macos::services`'s `osascript` path. Clicking the toast or one of
//!   its action buttons fires the WinRT `Activated` event on an arbitrary
//!   COM thread; the handler decodes the clicked action from the toast
//!   XML's per-button `arguments` and pushes a
//!   [`crate::event::UiEvent::NotificationActivated`] onto
//!   [`WinPlatformServices`]'s own notification queue, then wakes the UI
//!   thread with the same `WM_QUADRAUI_USER_EVENT` message
//!   [`crate::backend::Backend::waker`] uses — `win::run`'s `wndproc`
//!   drains and dispatches it from there (see
//!   [`WinPlatformServices::drain_notification_events`]). If
//!   `CreateToastNotifierWithId`/`Show` fails for any reason (no AUMID
//!   registered, WinRT unavailable), `send_notification` falls back to
//!   the legacy transient `Shell_NotifyIconW` balloon tip — add a tray
//!   icon with `NIF_INFO` set, then remove it a few seconds later from a
//!   spawned thread (a balloon has no lifetime of its own independent of
//!   the icon it's attached to, and this backend has no persistent tray
//!   icon to hang it off). The balloon's `silent` maps onto
//!   `NIIF_NOSOUND`; its `icon`/`actions`/`tag` don't fit a balloon's
//!   shape and are dropped — see `win_send_notification_balloon`'s own
//!   doc.
//! - **Message dialogs** — `TaskDialogIndirect`, the modern
//!   common-controls v6 alert (preferred over the legacy `MessageBoxW`
//!   for its richer, arbitrarily-labelled button row — `MessageDialogOptions`
//!   carries caller-declared buttons, not a fixed OK/Cancel/Yes/No set).
//!   Blocking and synchronous, same as the file dialogs above, so it
//!   needs no [`crate::desktop::ModalPumpGuard`] of its own either — see
//!   this module's `#702` audit note below, which applies identically
//!   here.
//! - **`open_url`/`open_url_result`** — `ShellExecuteW(NULL, "open", url,
//!   ...)` via [`crate::desktop::open_with_default`] — the
//!   crate-wide opener shared with `tui::services`/`macos::services`.
//!   `open_url_result` now overrides the trait's `Ok(())` default and
//!   reports the call's real outcome; `open_url` itself stays the
//!   infallible fire-and-forget wrapper.
//! - **`shell.*` parity** — `reveal_in_file_manager`
//!   (`SHOpenFolderAndSelectItems`, see `win_reveal_in_file_manager`'s own
//!   doc for the PIDL dance it takes), `open_path` (the same
//!   `ShellExecuteW` call `open_url_result` makes, via
//!   `crate::desktop::open_with_default` and shared by both),
//!   `move_to_trash` (delegates to [`crate::desktop::move_to_trash`] — the
//!   cross-platform `trash` crate, not a hand-rolled `SHFileOperationW`, see that
//!   function's doc for why), and `beep` (`MessageBeep(MB_OK)`).
//! - **Displays** — `displays` uses `EnumDisplayMonitors` +
//!   `GetMonitorInfoW` (`rcMonitor`/`rcWork`/`MONITORINFOF_PRIMARY`) and
//!   `GetDpiForMonitor`; `cursor_screen_point` uses `GetCursorPos`. See
//!   `win_displays`/`win_cursor_screen_point`'s own docs.
//!
//! Real WinAPI/COM calls are gated on `cfg(target_os = "windows")` —
//! see `super`'s module docs and `Cargo.toml`'s `win` feature comment for
//! why that keeps `cargo check --features win` meaningful on Linux.
//! Everywhere else every method keeps the original graceful-no-op stub
//! body it shipped with before this issue.
//!
//! ## #702 audit note: `IFileOpenDialog`/`IFileSaveDialog::Show` needs no
//! `ModalPumpGuard` of its own here
//!
//! quadraui#702's issue text names `IFileOpenDialog::Show` as this
//! backend's own instance of the nested-native-modal-loop hazard
//! [`crate::desktop::ModalPumpDepth`]/[`crate::desktop::ModalPumpGuard`]
//! exist to guard (mirroring `GtkPlatformServices`'s async-`FileDialog`
//! pump, #427) — so it's worth being explicit about why
//! `show_file_open_dialog`/`show_file_save_dialog` (and, since #744,
//! `show_message_dialog`'s `TaskDialogIndirect` call) below take no new
//! guard of their own, rather than the omission looking like an
//! oversight. All three are only ever called from inside `app.handle`
//! (`Backend::services()` is the only way to reach them), and `win::run`'s
//! `dispatch` (`src/win/run.rs`) already wraps *all* of `app.handle` in a
//! `super::guarded_call(&ws.state, &ws.pump_depth, …)` — so
//! `ws.pump_depth` is already incremented for the *entire* duration of
//! whatever `app.handle` does, including a synchronous, blocking
//! `IFileOpenDialog::Show`/`TaskDialogIndirect` call made from inside it.
//! Any `wndproc` message that re-enters during that blocking call (a
//! `WM_PAINT` for an exposed region, say — Win32 still paints disabled
//! owner windows) already sees `ws.pump_depth.is_pumping()` and cedes to
//! `DefWindowProcW`, purely as a side effect of `dispatch`'s existing
//! guard scope. A second, independent guard planted here would be
//! redundant, not additive — there is only one `WindowState::pump_depth`
//! counter, and it's already live for this entire call stack.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use crate::backend::MessageDialogButton;
use crate::backend::{
    BackendError, Clipboard, Display, FileDialogOptions, MessageDialogChoice, MessageDialogOptions,
    Notification, PlatformServices, RgbaImage, ServiceResult, SystemTheme,
};
use crate::event::{Point, Rect, UiEvent};
#[cfg(target_os = "windows")]
use crate::primitives::dialog::DialogSeverity;
use crate::types::Color;
#[cfg(target_os = "windows")]
use crate::types::WidgetId;

#[cfg(target_os = "windows")]
use std::cell::{Cell, RefCell};
#[cfg(target_os = "windows")]
use std::collections::{HashMap, VecDeque};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use windows::core::{IUnknown, Interface, BOOL, HSTRING, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Data::Xml::Dom::XmlDocument;
#[cfg(target_os = "windows")]
use windows::Foundation::TypedEventHandler;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IBindCtx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
#[cfg(target_os = "windows")]
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON,
    TDF_ALLOW_DIALOG_CANCELLATION, TD_ERROR_ICON, TD_INFORMATION_ICON, TD_WARNING_ICON,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::{
    DragQueryFileW, FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, IShellItem,
    SHCreateItemFromParsingName, SHOpenFolderAndSelectItems, Shell_NotifyIconW, FOS_PICKFOLDERS,
    HDROP, NIF_ICON, NIF_INFO, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW, SIGDN_FILESYSPATH,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, LoadIconW, PostMessageW, SystemParametersInfoW, HICON, IDI_ERROR,
    IDI_INFORMATION, MB_OK, MONITORINFOF_PRIMARY, SPI_GETCLIENTAREAANIMATION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, USER_DEFAULT_SCREEN_DPI,
};
// WinRT (not Win32) — `system_theme`. `UISettings` is the
// same class the issue names (`UISettings::GetColorValue`); `AccessibilitySettings`
// is its sibling in the same `Windows.UI.ViewManagement` namespace and the
// natural WinRT source for "is high contrast active" — no separate Win32
// `SystemParametersInfoW(SPI_GETHIGHCONTRAST)` call (and no extra
// `Win32_UI_Accessibility` Cargo feature) needed.
#[cfg(target_os = "windows")]
use windows::UI::ViewManagement::{AccessibilitySettings, UIColorType, UISettings};
// WinRT toast notifications — `ToastNotification`/`ToastNotificationManager`
// build and show the `ToastGeneric` XML document `build_toast_xml` produces;
// `ToastActivatedEventArgs` is what `Activated`'s generic `IInspectable`
// argument casts to for the clicked action's `arguments` string.
#[cfg(target_os = "windows")]
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs, ToastFailedEventArgs,
    ToastNotification, ToastNotificationManager, ToastNotifier,
};

/// Live toasts keyed by notification id: the toast's `tag` plus the
/// `ToastNotifier`/`ToastNotification` COM pair kept alive until the toast
/// is activated, dismissed or fails.
#[cfg(target_os = "windows")]
type ActiveToasts = Arc<Mutex<HashMap<u64, (Option<String>, ToastNotifier, ToastNotification)>>>;

/// System clipboard via raw Win32 calls. Stateless — every
/// call opens, does one thing, and closes the clipboard, matching the
/// classic Win32 clipboard's own "hold it as briefly as possible"
/// contract (it's a single systemwide resource; holding it open blocks
/// every other app's clipboard access).
pub struct WinClipboard;

impl Clipboard for WinClipboard {
    fn read_text(&self) -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_read()
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    fn write_text(&self, text: &str) {
        #[cfg(target_os = "windows")]
        {
            let _ = win_clipboard_write(text);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
        }
    }

    /// Real `OpenClipboard`/`GlobalAlloc`/`SetClipboardData` failure,
    /// surfaced instead of silently discarded — the one
    /// in-tree override of [`Clipboard::write_text_result`]'s default.
    fn write_text_result(&self, text: &str) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_write(text)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
            Ok(())
        }
    }

    /// `CF_HDROP` file-path list — a Finder/Explorer copy. One
    /// well-known fixed-layout struct (`DROPFILES` + a `\0`-separated,
    /// `\0\0`-terminated path list), decoded below with no format
    /// ambiguity.
    fn read_file_list(&self) -> ServiceResult<Vec<PathBuf>> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_read_file_list()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `EmptyClipboard` — clears every format, not just text.
    fn clear(&self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_clear()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(())
        }
    }

    /// Decoded RGBA pixels from `CF_DIBV5` (preferred, alpha-capable) or
    /// classic `CF_DIB` (24bpp, always opaque) — see [`decode_dib`]'s own
    /// doc for the supported pixel-format subset.
    fn read_image(&self) -> ServiceResult<RgbaImage> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_read_image()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// Always written as `CF_DIBV5` (32bpp `BI_BITFIELDS`, alpha-capable)
    /// — see [`encode_dibv5`]'s own doc.
    fn write_image(&self, image: &RgbaImage) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_write_image(image)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = image;
            Err(BackendError::Unsupported)
        }
    }

    /// `RegisterClipboardFormatW(L"HTML Format")` plus the `CF_HTML`
    /// byte-offset text header — see [`build_cf_html`]'s own doc. Also
    /// writes `alt_text` as `CF_UNICODETEXT` in the same open/close
    /// session, the plain-text degrade this trait method's own doc
    /// requires.
    fn write_html(&self, html: &str, alt_text: &str) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_clipboard_write_html(html, alt_text)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (html, alt_text);
            Err(BackendError::Unsupported)
        }
    }
}

pub struct WinPlatformServices {
    clipboard: WinClipboard,
    /// Top-level window used to parent file dialogs (`IModalWindow::Show`)
    /// and host the notification tray icon's owning `HWND`. `None` until
    /// [`WinBackend::attach_surface`][super::backend::WinBackend::attach_surface]
    /// calls [`Self::set_window`] with a live window. `Cell`, not
    /// `RefCell<Option<_>>`, since `HWND` is `Copy`.
    #[cfg(target_os = "windows")]
    window: Cell<Option<HWND>>,
    /// This process's AppUserModelID, set once by `win::run` at window
    /// creation via [`Self::set_aumid`]. `ToastNotificationManager::
    /// CreateToastNotifierWithId` needs one to attribute a toast to this
    /// app; `None` means `send_notification` has no choice but the
    /// `Shell_NotifyIconW` balloon fallback.
    #[cfg(target_os = "windows")]
    aumid: RefCell<Option<String>>,
    /// `UiEvent::NotificationActivated` events queued by a toast's
    /// `Activated` handler (fired on an arbitrary COM thread) for
    /// `win::run`'s `wndproc` to drain and dispatch back on the UI thread —
    /// see [`Self::drain_notification_events`]. `Arc`-wrapped so the
    /// `Send + 'static` `Activated` closure can hold its own clone without
    /// borrowing `self`.
    #[cfg(target_os = "windows")]
    notification_events: Arc<Mutex<VecDeque<UiEvent>>>,
    /// Live toasts shown by [`win_send_notification_toast`], keyed by a
    /// monotonic id private to this map (see [`NEXT_TOAST_ID`]). A toast's
    /// `ToastNotifier`/`ToastNotification` are ordinary refcounted COM
    /// wrappers: dropping them unregisters the `Activated`/`Dismissed`/
    /// `Failed` handlers that were just wired up, which would silently
    /// break click-through the instant `send_notification` returns. An
    /// entry is removed once the toast can no longer be activated: by the
    /// `Activated` handler itself once the user has acted on it, by the
    /// `Dismissed` handler when the dismissal reason is `UserCanceled` or
    /// `ApplicationHidden` (but *not* `TimedOut` — that just means the
    /// toast moved from the popup banner into Action Center, where it is
    /// still live and awaiting activation), by the `Failed` handler, or
    /// immediately if `Show` itself errors synchronously. Any earlier
    /// entry sharing the new toast's `tag` is pruned before the new one is
    /// inserted — same "replace, don't stack" rule `Tag` itself gives
    /// Action Center.
    #[cfg(target_os = "windows")]
    active_toasts: ActiveToasts,
}

impl WinPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: WinClipboard,
            #[cfg(target_os = "windows")]
            window: Cell::new(None),
            #[cfg(target_os = "windows")]
            aumid: RefCell::new(None),
            #[cfg(target_os = "windows")]
            notification_events: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(target_os = "windows")]
            active_toasts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Store the live top-level window handle so file dialogs open
    /// parented to it and notifications have an owning `HWND` to attach
    /// their tray icon to. Called once by `WinBackend::attach_surface`
    /// right after `CreateWindowExW` returns a live `HWND` — same timing
    /// as `GtkPlatformServices::set_window`.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_window(&self, hwnd: HWND) {
        self.window.set(Some(hwnd));
    }

    /// Store this process's AppUserModelID, computed by `win::run` at
    /// window creation from `RunConfig::title`, so `send_notification` can
    /// pass it to `ToastNotificationManager::CreateToastNotifierWithId`.
    /// Ungated signature: `WinBackend::set_aumid` calls this unconditionally
    /// from `wndproc`'s `#[cfg(target_os = "windows")]` block, but the
    /// delegate itself still needs to type-check on every platform.
    pub(crate) fn set_aumid(&self, aumid: String) {
        #[cfg(target_os = "windows")]
        {
            *self.aumid.borrow_mut() = Some(aumid);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = aumid;
        }
    }

    /// Drain every `UiEvent::NotificationActivated` a toast's `Activated`
    /// handler has queued since the last drain. Called from `win::run`'s
    /// `wndproc` on `WM_QUADRAUI_USER_EVENT`, alongside
    /// `WinBackend::drain_user_events`.
    pub(crate) fn drain_notification_events(&self) -> Vec<UiEvent> {
        #[cfg(target_os = "windows")]
        {
            let mut queue = self
                .notification_events
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            queue.drain(..).collect()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Vec::new()
        }
    }
}

impl Default for WinPlatformServices {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for WinPlatformServices {
    fn platform_name(&self) -> &'static str {
        "win-gui"
    }

    fn clipboard(&self) -> &dyn Clipboard {
        &self.clipboard
    }

    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            win_show_open_dialog(self.window.get(), &opts)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opts;
            None
        }
    }

    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            win_show_save_dialog(self.window.get(), &opts)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opts;
            None
        }
    }

    /// The shell folder-picker (`IFileOpenDialog` + `FOS_PICKFOLDERS`).
    fn show_folder_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            win_show_folder_open_dialog(self.window.get(), &opts)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opts;
            None
        }
    }

    /// `TaskDialogIndirect` — see this module's doc comment for
    /// why `TaskDialog` over the legacy `MessageBoxW`.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        #[cfg(target_os = "windows")]
        {
            win_show_message_dialog(self.window.get(), &opts)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opts;
            None
        }
    }

    /// Try the WinRT toast path first (requires an AUMID, set via
    /// [`Self::set_aumid`]); fall back to the legacy
    /// [`win_send_notification_balloon`] on any failure — no AUMID
    /// registered, or `CreateToastNotifierWithId`/`Show` erroring out.
    fn send_notification(&self, n: Notification) {
        #[cfg(target_os = "windows")]
        {
            let aumid = self.aumid.borrow().clone();
            let shown = match aumid {
                Some(aumid) => win_send_notification_toast(
                    self.window.get(),
                    &aumid,
                    Arc::clone(&self.notification_events),
                    Arc::clone(&self.active_toasts),
                    &n,
                )
                .is_ok(),
                None => false,
            };
            if !shown {
                win_send_notification_balloon(self.window.get(), &n);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = n;
        }
    }

    fn open_url(&self, url: &str) {
        let _ = self.open_url_result(url);
    }

    /// Fallible twin of [`Self::open_url`]: reports
    /// `ShellExecuteW`'s real outcome via
    /// [`crate::desktop::open_with_default`] instead of `open_url`'s
    /// discarded one — before this override, `open_url_result` fell to
    /// [`crate::backend::PlatformServices::open_url_result`]'s default
    /// (`open_url` then always `Ok(())`), so a failed `ShellExecuteW` call
    /// (e.g. `SE_ERR_FNF` — the target does not exist) was silently
    /// reported as success.
    ///
    /// Note what this still cannot promise: `ShellExecuteW` reports
    /// whether it *handed the target off*, not whether anything
    /// subsequently opened. An unregistered URI scheme, for instance,
    /// returns success (42) because the shell accepted it and will sort
    /// out the missing handler asynchronously — see this module's
    /// `open_url_result_and_open_path_report_a_real_shell_execute_failure`
    /// test for the failure this *can* honestly detect.
    fn open_url_result(&self, url: &str) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            crate::desktop::open_with_default(std::ffi::OsStr::new(url))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = url;
            Err(BackendError::Unsupported)
        }
    }

    /// `SHOpenFolderAndSelectItems` — see
    /// `win_reveal_in_file_manager`'s doc for the `ILCreateFromPathW`/
    /// `ILClone`/`ILRemoveLastID` PIDL dance it takes.
    fn reveal_in_file_manager(&self, path: &Path) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_reveal_in_file_manager(path)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = path;
            Err(BackendError::Unsupported)
        }
    }

    /// `ShellExecuteW(NULL, "open", path, ...)` via
    /// [`crate::desktop::open_with_default`] — the exact
    /// same call [`Self::open_url_result`] makes above, just fed a
    /// filesystem path instead of a URL string; `ShellExecuteW`'s
    /// `"open"` verb already accepts either.
    fn open_path(&self, path: &Path) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            crate::desktop::open_with_default(path.as_os_str())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = path;
            Err(BackendError::Unsupported)
        }
    }

    /// [`crate::desktop::move_to_trash`] — see that
    /// function's doc for why every backend, Win-GUI included, shares
    /// this one `trash`-crate-backed implementation rather than
    /// hand-rolling `SHFileOperationW(FOF_ALLOWUNDO)` here.
    fn move_to_trash(&self, path: &Path) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            crate::desktop::move_to_trash(path)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = path;
            Err(BackendError::Unsupported)
        }
    }

    /// `MessageBeep(MB_OK)` — the default system
    /// notification sound (Windows' Settings maps `MB_OK`'s alias, "Asterisk"/
    /// "Default Beep" depending on the Windows version, to whichever sound
    /// scheme the user picked; there is no dedicated "just beep" API
    /// distinct from this legacy `MessageBoxW`-family sound-alias
    /// mechanism).
    fn beep(&self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_beep()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `UISettings::GetColorValue` (`Background`/`Accent`) +
    /// `AccessibilitySettings::HighContrast` — see this
    /// module's WinRT import comment for why the latter comes from the
    /// same namespace rather than a separate Win32 SPI call.
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        #[cfg(target_os = "windows")]
        {
            win_system_theme()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `EnumDisplayMonitors` + `GetMonitorInfoW` for `bounds`/`work_area`/
    /// `primary` (`rcMonitor`/`rcWork`/`MONITORINFOF_PRIMARY`),
    /// `GetDpiForMonitor` for `scale`.
    fn displays(&self) -> ServiceResult<Vec<Display>> {
        #[cfg(target_os = "windows")]
        {
            win_displays()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `GetCursorPos` — the global cursor position in
    /// virtual-screen coordinates, the same coordinate space
    /// [`Self::displays`]'s `bounds`/`work_area` use.
    fn cursor_screen_point(&self) -> ServiceResult<Point> {
        #[cfg(target_os = "windows")]
        {
            win_cursor_screen_point()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }
}

// ─── Wide-string framing (host-independent — unit-tested off Windows,
// same posture as `win::msg`'s pure arithmetic) ─────────────────────────

/// Encode `text` as a NUL-terminated UTF-16 buffer — the shape every
/// wide-string Win32 API below expects (`CF_UNICODETEXT`'s clipboard
/// payload, `NOTIFYICONDATAW`'s fixed-size fields). The one
/// implementation shared crate-wide — see
/// [`crate::desktop::wide_nul_terminated`]'s own doc for why it's
/// `allow`-gated rather than `cfg`-gated.
use crate::desktop::wide_nul_terminated;

/// Decode a UTF-16 slice back to a `String`, stopping at the first NUL
/// (or the slice's end, whichever comes first) — the inverse framing of
/// [`wide_nul_terminated`], used to read a `CF_UNICODETEXT` clipboard
/// payload back out. See [`wide_nul_terminated`]'s doc comment for why
/// this is `allow`-gated rather than `cfg`-gated.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_wide_nul_terminated(slice: &[u16]) -> String {
    let len = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..len])
}

/// Copy `text`'s UTF-16 encoding into `dst`, NUL-terminated, truncating
/// if it doesn't fit. Used for `NOTIFYICONDATAW`'s fixed-size
/// `szInfo`/`szInfoTitle` arrays, which can't grow to fit an arbitrarily
/// long notification title/body. See [`wide_nul_terminated`]'s doc
/// comment for why this is `allow`-gated rather than `cfg`-gated.
///
/// `pub(crate)`, not private: issue #953's `super::tray` reuses this
/// for `NOTIFYICONDATAW::szTip`, the persistent tray icon's fixed-size
/// tooltip buffer — the exact same truncate-and-NUL-terminate contract
/// this module's own `szInfo`/`szInfoTitle` calls already rely on.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn copy_wide_truncated(dst: &mut [u16], text: &str) {
    if dst.is_empty() {
        return;
    }
    let wide = wide_nul_terminated(text);
    let take = wide.len().min(dst.len() - 1);
    dst[..take].copy_from_slice(&wide[..take]);
    dst[take] = 0;
}

// ─── Clipboard ──────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_clipboard_read() -> Option<String> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every exit
    // path (the inner closure's early returns only skip the *decode*, the
    // `CloseClipboard()` after it always runs). `GlobalLock` is checked
    // for null before the resulting pointer is read, and the slice built
    // from it is bounded by `GlobalSize` — a live lock on a handle this
    // process just got from `GetClipboardData`, so both the pointer and
    // the length it's paired with come from the same OS call.
    unsafe {
        // Checked before `OpenClipboard` — an empty/non-text clipboard is
        // the normal "nothing to paste" case, not an error worth holding
        // the clipboard open to discover.
        IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
        OpenClipboard(None).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Bound the read by the block's real size rather than
            // trusting an unbounded NUL scan — `CF_UNICODETEXT` is
            // documented NUL-terminated, but bounding first is cheap
            // insurance against a misbehaving clipboard owner.
            let word_len = GlobalSize(hglobal) / std::mem::size_of::<u16>();
            let slice = std::slice::from_raw_parts(ptr, word_len);
            let text = decode_wide_nul_terminated(slice);
            let _ = GlobalUnlock(hglobal);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Write `text` to the system clipboard as `CF_UNICODETEXT`, returning
/// the real failure instead of swallowing it. Every early
/// return below leaves the clipboard exactly as `write_text`/
/// `write_text_result`'s callers have always observed it — this only
/// adds the `Err` a caller can now ask for via `write_text_result`;
/// `write_text` itself still discards it.
#[cfg(target_os = "windows")]
fn win_clipboard_write(text: &str) -> ServiceResult<()> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path
    // through this function (the early `return` on `OpenClipboard`
    // failure is before the clipboard is ever opened; every other exit
    // falls through to the trailing `CloseClipboard()`). `GlobalLock`'s
    // pointer is checked for null before the `copy_nonoverlapping` write
    // through it, and the write is bounded by `bytes` — the exact size
    // just passed to the paired `GlobalAlloc`. `hglobal` ownership after
    // `SetClipboardData` follows the comment at that call site below.
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let _ = EmptyClipboard();
        let wide = wide_nul_terminated(text);
        let bytes = wide.len() * std::mem::size_of::<u16>();
        let result = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
            Ok(hglobal) => {
                let ptr = GlobalLock(hglobal) as *mut u16;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(hglobal));
                    Err(BackendError::PlatformFailure {
                        context: "GlobalLock".to_string(),
                    })
                } else {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    let _ = GlobalUnlock(hglobal);
                    // On success the system now owns `hglobal` — it must NOT
                    // be `GlobalFree`d here. On failure nothing took
                    // ownership, so this is the only chance to release it.
                    if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hglobal.0))).is_err() {
                        let _ = GlobalFree(Some(hglobal));
                        Err(BackendError::PlatformFailure {
                            context: "SetClipboardData".to_string(),
                        })
                    } else {
                        Ok(())
                    }
                }
            }
            Err(_) => Err(BackendError::PlatformFailure {
                context: "GlobalAlloc".to_string(),
            }),
        };
        let _ = CloseClipboard();
        result
    }
}

/// Read the `CF_HDROP` file-path list off the clipboard — a
/// Finder/Explorer copy, on Windows a `DROPFILES` struct whose payload is
/// enumerated with `DragQueryFileW` rather than parsed by hand (the same
/// API a drag-and-drop `WM_DROPFILES` handler uses for the live-drag
/// case).
#[cfg(target_os = "windows")]
fn win_clipboard_read_file_list() -> ServiceResult<Vec<PathBuf>> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path
    // that opens the clipboard (both early `Err` returns above are
    // before `OpenClipboard`; the inner closure's own early returns only
    // skip building `files`, and the trailing `CloseClipboard()` always
    // runs after it). `DragQueryFileW`'s two-call idiom (no buffer to
    // learn the length, then a buffer sized for it) is the documented
    // Win32 pattern this mirrors `GetWindowTextW`-style elsewhere in this
    // module.
    unsafe {
        // Checked before `OpenClipboard`, same "don't hold the clipboard
        // open just to discover it's the wrong format" posture as
        // `win_clipboard_read`'s `CF_UNICODETEXT` check.
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "IsClipboardFormatAvailable(CF_HDROP)".to_string(),
            });
        }
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let result = (|| {
            let handle =
                GetClipboardData(CF_HDROP.0 as u32).map_err(|_| BackendError::PlatformFailure {
                    context: "GetClipboardData(CF_HDROP)".to_string(),
                })?;
            let hdrop = HDROP(handle.0);
            // `0xFFFFFFFF` (no buffer) asks `DragQueryFileW` for the file
            // count instead of a filename — the documented Win32 idiom.
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            let mut files = Vec::with_capacity(count as usize);
            for i in 0..count {
                // First call with no buffer to learn the required length
                // (excluding the NUL terminator); second call fills a
                // buffer sized for it. Two calls, same idiom as
                // `GetWindowTextW`/friends.
                let needed = DragQueryFileW(hdrop, i, None);
                if needed == 0 {
                    continue;
                }
                let mut buf = vec![0u16; needed as usize + 1];
                let copied = DragQueryFileW(hdrop, i, Some(&mut buf));
                buf.truncate(copied as usize);
                files.push(PathBuf::from(String::from_utf16_lossy(&buf)));
            }
            Ok(files)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// `EmptyClipboard` — clears every clipboard format at once, regardless
/// of what's on it. Unlike `win_clipboard_write`, there is
/// no payload to allocate/own, so this is the shortest possible
/// open/mutate/close cycle.
#[cfg(target_os = "windows")]
fn win_clipboard_clear() -> ServiceResult<()> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path —
    // the early `Err` return is before `OpenClipboard`, and the only
    // other exit falls through to the trailing `CloseClipboard()`. No
    // pointers or buffers cross the FFI boundary here.
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let result = EmptyClipboard().map_err(|_| BackendError::PlatformFailure {
            context: "EmptyClipboard".to_string(),
        });
        let _ = CloseClipboard();
        result
    }
}

// `BI_RGB`/`BI_BITFIELDS` as plain `u32`s rather than the `windows` crate's
// `BI_COMPRESSION` newtype — the DIB codec below works entirely on raw
// `&[u8]` byte buffers (see its module doc) and never touches a
// `windows`-crate struct, so pulling in the typed constant would be the
// only reason to depend on that module at all.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const BI_RGB_VALUE: u32 = 0;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const BI_BITFIELDS_VALUE: u32 = 3;
// `LCS_sRGB` FourCC and `LCS_GM_IMAGES` — `BITMAPV5HEADER`'s
// `bV5CSType`/`bV5Intent` fields. No named constant for either exists in
// the `windows` crate's `Gdi` module; these are the raw values from the
// Win32 SDK headers.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const LCS_SRGB: u32 = 0x7352_4742;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const LCS_GM_IMAGES: u32 = 4;

/// Byte length of one scanline of a `width`-pixel-wide, `bits_per_pixel`
/// DIB row, padded up to the next 4-byte boundary — every DIB row,
/// regardless of bit depth, is stored at this stride.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn dib_stride_bytes(width: u32, bits_per_pixel: u32) -> usize {
    let bits_per_row = width as usize * bits_per_pixel as usize;
    bits_per_row.div_ceil(32) * 4
}

/// Scale a `mask`-selected bitfield out of `pixel` to a full 0..=255
/// channel value. Widens/narrows proportionally
/// (`raw * 255 / max_value`, rounded to nearest) rather than assuming an
/// 8-bit-wide mask, but an 8-bit-wide mask (this module's own
/// [`encode_dibv5`] output) round-trips through this exactly: `raw` and
/// the result are numerically identical for every `raw` in `0..=255`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extract_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width_bits = mask.count_ones();
    let max_value = (1u64 << width_bits) - 1;
    let raw = ((pixel & mask) >> shift) as u64;
    ((raw * 255 + max_value / 2) / max_value) as u8
}

/// Locate a `BI_BITFIELDS` DIB's channel masks and the byte offset its
/// pixel data starts at. The DIB contract puts them in one of two places
/// depending on the header version, and the two disagree about both the
/// mask count and where the pixels begin:
/// - A `BITMAPV4HEADER` (108 bytes) or `BITMAPV5HEADER` (124 bytes)
///   declares all four R/G/B/A masks as header fields at offsets
///   40..56, so the pixels start at `header_size`.
/// - A plain 40-byte `BITMAPINFOHEADER` has no mask fields; it stores
///   three R/G/B mask `DWORD`s at offsets 40..52, in the space a
///   paletted DIB would use for its color table, which pushes the pixels
///   out to offset 52. There is no fourth `DWORD`, so the alpha mask is
///   reported as `0` — [`decode_32bpp`] reads that as "no alpha channel"
///   and forces those pixels opaque.
///
/// Returns `None` for a buffer too short to hold the masks it claims, or
/// a header size matching neither shape.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn dib_bitfield_masks(data: &[u8], header_size: u32) -> Option<((u32, u32, u32, u32), usize)> {
    let mask = |offset: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            data.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    if header_size >= 56 {
        let masks = (mask(40)?, mask(44)?, mask(48)?, mask(52)?);
        Some((masks, header_size as usize))
    } else if header_size == 40 {
        let masks = (mask(40)?, mask(44)?, mask(48)?, 0);
        Some((masks, 52))
    } else {
        None
    }
}

/// Decode a `CF_DIB`/`CF_DIBV5` byte buffer (the shared 40-byte
/// `BITMAPINFOHEADER`-shaped prefix, optionally followed by the
/// `BITMAPV4HEADER`/`BITMAPV5HEADER` extension, then the pixel data) into
/// an [`RgbaImage`]. Supports exactly three shapes, each decoded with no
/// format-detection ambiguity, and returns `None` for anything else
/// (paletted, RLE-compressed, 16bpp, …) rather than guess at a byte
/// layout this module can't verify:
/// - 32bpp `BI_BITFIELDS` with arbitrary channel masks (this module's own
///   [`encode_dibv5`] output, and any well-formed external producer).
///   Both mask placements the DIB contract allows are read: the four
///   in-header masks of a `BITMAPV4HEADER`/`BITMAPV5HEADER`, and the
///   three R/G/B mask `DWORD`s a 40-byte `BITMAPINFOHEADER` stores in
///   place of its color table (alpha then has no mask at all, so those
///   pixels decode fully opaque) — see [`dib_bitfield_masks`].
/// - 32bpp `BI_RGB` — the 4th byte is producer-defined, not real alpha,
///   so every pixel decodes fully opaque.
/// - 24bpp `BI_RGB` — BGR, no alpha, rows padded to a 4-byte stride.
///
/// All three are bottom-up (`height > 0`) or top-down (`height < 0`) per
/// the standard DIB convention; row order is normalized away here so
/// every [`RgbaImage`] this function returns is top-to-bottom regardless
/// of which the source used.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_dib(data: &[u8]) -> Option<RgbaImage> {
    if data.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let width = i32::from_le_bytes(data[4..8].try_into().ok()?);
    let height = i32::from_le_bytes(data[8..12].try_into().ok()?);
    let bit_count = u16::from_le_bytes(data[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(data[16..20].try_into().ok()?);
    if width <= 0 || height == 0 || header_size as usize > data.len() {
        return None;
    }
    let width = width as u32;
    let top_down = height < 0;
    let abs_height = height.unsigned_abs();

    let (masks, pixel_offset) = if compression == BI_BITFIELDS_VALUE {
        let (masks, offset) = dib_bitfield_masks(data, header_size)?;
        (Some(masks), offset)
    } else {
        (None, header_size as usize)
    };
    if pixel_offset > data.len() {
        return None;
    }

    let pixel_data = &data[pixel_offset..];
    let image = match (bit_count, compression, masks) {
        (32, c, Some((r, g, b, a))) if c == BI_BITFIELDS_VALUE => {
            decode_32bpp(pixel_data, width, abs_height, Some((r, g, b, a)))?
        }
        (32, c, None) if c == BI_RGB_VALUE => decode_32bpp(pixel_data, width, abs_height, None)?,
        (24, c, _) if c == BI_RGB_VALUE => decode_24bpp(pixel_data, width, abs_height)?,
        _ => return None,
    };

    Some(if top_down {
        image
    } else {
        flip_vertical(image)
    })
}

/// Decode a bottom-up-stored 32bpp pixel region. `masks` of `Some`
/// selects `BI_BITFIELDS` channel extraction via [`extract_channel`];
/// `None` means `BI_RGB` — conventional `BGRX` byte order, alpha forced
/// fully opaque since the 4th byte carries no defined meaning.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_32bpp(
    pixel_data: &[u8],
    width: u32,
    height: u32,
    masks: Option<(u32, u32, u32, u32)>,
) -> Option<RgbaImage> {
    let stride = dib_stride_bytes(width, 32);
    if pixel_data.len() < stride * height as usize {
        return None;
    }
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height {
        let src_row = &pixel_data[row as usize * stride..][..width as usize * 4];
        for col in 0..width as usize {
            let px = u32::from_le_bytes(src_row[col * 4..col * 4 + 4].try_into().ok()?);
            let (r, g, b, a) = match masks {
                Some((rm, gm, bm, am)) => (
                    extract_channel(px, rm),
                    extract_channel(px, gm),
                    extract_channel(px, bm),
                    if am == 0 {
                        255
                    } else {
                        extract_channel(px, am)
                    },
                ),
                None => {
                    let bytes = px.to_le_bytes();
                    (bytes[2], bytes[1], bytes[0], 255)
                }
            };
            let out = (row as usize * width as usize + col) * 4;
            pixels[out] = r;
            pixels[out + 1] = g;
            pixels[out + 2] = b;
            pixels[out + 3] = a;
        }
    }
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Decode a bottom-up-stored 24bpp `BI_RGB` pixel region — BGR triples,
/// rows padded to a 4-byte stride, always fully opaque.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_24bpp(pixel_data: &[u8], width: u32, height: u32) -> Option<RgbaImage> {
    let stride = dib_stride_bytes(width, 24);
    if pixel_data.len() < stride * height as usize {
        return None;
    }
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height {
        let src_row = &pixel_data[row as usize * stride..][..width as usize * 3];
        for col in 0..width as usize {
            let b = src_row[col * 3];
            let g = src_row[col * 3 + 1];
            let r = src_row[col * 3 + 2];
            let out = (row as usize * width as usize + col) * 4;
            pixels[out] = r;
            pixels[out + 1] = g;
            pixels[out + 2] = b;
            pixels[out + 3] = 255;
        }
    }
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Flip an [`RgbaImage`]'s rows top-to-bottom — [`decode_dib`]'s one
/// normalization step for the bottom-up (positive-height) DIB case.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn flip_vertical(image: RgbaImage) -> RgbaImage {
    let row_bytes = image.width as usize * 4;
    let mut pixels = vec![0u8; image.pixels.len()];
    for row in 0..image.height as usize {
        let src = &image.pixels[row * row_bytes..][..row_bytes];
        let dst_row = image.height as usize - 1 - row;
        pixels[dst_row * row_bytes..][..row_bytes].copy_from_slice(src);
    }
    RgbaImage {
        width: image.width,
        height: image.height,
        pixels,
    }
}

/// Encode an [`RgbaImage`] as a top-down `CF_DIBV5` byte buffer: a
/// 124-byte `BITMAPV5HEADER` (`BI_BITFIELDS`, standard 8-bit-wide
/// `0x00FF0000`/`0x0000FF00`/`0x000000FF`/`0xFF000000` R/G/B/A masks,
/// `LCS_sRGB` color space) followed by 32bpp `BGRA` pixel data, no row
/// padding needed since 32bpp rows are always a multiple of 4 bytes.
/// Negative height marks the buffer top-down, matching
/// [`encode_32bpp_bgra`]'s row order and letting [`decode_dib`] skip the
/// bottom-up flip on this module's own output.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn encode_dibv5(image: &RgbaImage) -> Vec<u8> {
    let mut header = vec![0u8; 124];
    header[0..4].copy_from_slice(&124u32.to_le_bytes());
    header[4..8].copy_from_slice(&(image.width as i32).to_le_bytes());
    header[8..12].copy_from_slice(&(-(image.height as i32)).to_le_bytes());
    header[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
    header[14..16].copy_from_slice(&32u16.to_le_bytes()); // bit_count
    header[16..20].copy_from_slice(&BI_BITFIELDS_VALUE.to_le_bytes());
    let pixel_bytes = image.width as usize * image.height as usize * 4;
    header[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes()); // size_image
    header[40..44].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // red mask
    header[44..48].copy_from_slice(&0x0000_FF00u32.to_le_bytes()); // green mask
    header[48..52].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // blue mask
    header[52..56].copy_from_slice(&0xFF00_0000u32.to_le_bytes()); // alpha mask
    header[56..60].copy_from_slice(&LCS_SRGB.to_le_bytes());
    header[108..112].copy_from_slice(&LCS_GM_IMAGES.to_le_bytes());
    let mut out = header;
    out.extend_from_slice(&encode_32bpp_bgra(image));
    out
}

/// Row-major, top-to-bottom 32bpp `BGRA` pixel bytes for `image` — the
/// pixel-data half of [`encode_dibv5`]'s output, matching the R/G/B/A
/// masks it writes into the header.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn encode_32bpp_bgra(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.pixels.len());
    for px in image.pixels.chunks_exact(4) {
        out.push(px[2]); // B
        out.push(px[1]); // G
        out.push(px[0]); // R
        out.push(px[3]); // A
    }
    out
}

/// Build the `CF_HTML` ("HTML Format") payload for `html`: a text header
/// (`Version:0.9`, then `StartHTML`/`EndHTML`/`StartFragment`/
/// `EndFragment`, each a `{:010}`-padded decimal byte offset into the
/// whole buffer, `\r\n`-terminated) followed by
/// `<html><body><!--StartFragment-->html<!--EndFragment--></body></html>`.
/// Every offset field is the same fixed width regardless of its value, so
/// the header's total length never depends on what the real offsets turn
/// out to be — computed once from a placeholder of that same width, then
/// reused for the real offsets. (Realistic buffer sizes never reach the
/// 10-digit field's ~10GB ceiling, so this holds unconditionally here.)
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_cf_html(html: &str) -> String {
    fn header(
        start_html: usize,
        end_html: usize,
        start_fragment: usize,
        end_fragment: usize,
    ) -> String {
        format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n",
            start_html, end_html, start_fragment, end_fragment
        )
    }
    let placeholder = header(0, 0, 0, 0);
    let header_len = placeholder.len();
    // `{:010}` only pads up to 10 digits — it doesn't truncate wider
    // values — so the "fixed header length" assumption this function
    // relies on only holds for offsets that actually fit in 10 digits
    // (anything under 10 billion, far past any realistic clipboard
    // payload). `9_999_999_999` is the largest value that still fits.
    debug_assert_eq!(
        header_len,
        header(9_999_999_999, 9_999_999_999, 9_999_999_999, 9_999_999_999).len()
    );

    let prefix = "<html>\r\n<body>\r\n<!--StartFragment-->";
    let suffix = "<!--EndFragment-->\r\n</body>\r\n</html>";

    let start_fragment = header_len + prefix.len();
    let end_fragment = start_fragment + html.len();
    let end_html = end_fragment + suffix.len();

    let mut out = header(header_len, end_html, start_fragment, end_fragment);
    out.push_str(prefix);
    out.push_str(html);
    out.push_str(suffix);
    out
}

/// The exact `CF_HTML` clipboard block for `html`: [`build_cf_html`]'s
/// UTF-8 payload plus the trailing NUL every registered text format's
/// block carries. The header offsets [`build_cf_html`] computes
/// deliberately exclude that terminator — `EndHTML` is the length of the
/// payload proper, so a consumer slicing `StartHTML..EndHTML` never sees
/// it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn cf_html_block(html: &str) -> Vec<u8> {
    let payload = build_cf_html(html);
    let mut block = Vec::with_capacity(payload.len() + 1);
    block.extend_from_slice(payload.as_bytes());
    block.push(0);
    block
}

/// Read `CF_DIBV5` (preferred) or classic `CF_DIB` off the clipboard and
/// decode it via [`decode_dib`].
#[cfg(target_os = "windows")]
fn win_clipboard_read_image() -> ServiceResult<RgbaImage> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path —
    // both early `Err` returns are before `OpenClipboard`; every other
    // exit falls through the trailing `CloseClipboard()`. `GlobalLock`'s
    // pointer is checked for null before the slice built through it, and
    // the slice is bounded by `GlobalSize`.
    unsafe {
        let format = if IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok() {
            CF_DIBV5.0 as u32
        } else if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() {
            CF_DIB.0 as u32
        } else {
            return Err(BackendError::PlatformFailure {
                context: "IsClipboardFormatAvailable(CF_DIBV5/CF_DIB)".to_string(),
            });
        };
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let result = (|| {
            let handle = GetClipboardData(format).map_err(|_| BackendError::PlatformFailure {
                context: "GetClipboardData(CF_DIB)".to_string(),
            })?;
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u8;
            if ptr.is_null() {
                return Err(BackendError::PlatformFailure {
                    context: "GlobalLock".to_string(),
                });
            }
            let len = GlobalSize(hglobal);
            let slice = std::slice::from_raw_parts(ptr, len);
            let image = decode_dib(slice);
            let _ = GlobalUnlock(hglobal);
            image.ok_or_else(|| BackendError::PlatformFailure {
                context: "decode_dib: unsupported DIB pixel format".to_string(),
            })
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Encode `image` via [`encode_dibv5`] and write it to the clipboard as
/// `CF_DIBV5`.
#[cfg(target_os = "windows")]
fn win_clipboard_write_image(image: &RgbaImage) -> ServiceResult<()> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path
    // (the early `Err` return is before `OpenClipboard`); `GlobalLock`'s
    // pointer is checked for null before the `copy_nonoverlapping` write,
    // bounded by `bytes.len()` — the exact size just passed to the paired
    // `GlobalAlloc`. `hglobal` ownership after `SetClipboardData` follows
    // the same rule as `win_clipboard_write`.
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let _ = EmptyClipboard();
        let bytes = encode_dibv5(image);
        let result = match GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
            Ok(hglobal) => {
                let ptr = GlobalLock(hglobal) as *mut u8;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(hglobal));
                    Err(BackendError::PlatformFailure {
                        context: "GlobalLock".to_string(),
                    })
                } else {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                    let _ = GlobalUnlock(hglobal);
                    // On success the system now owns `hglobal` — it must NOT
                    // be `GlobalFree`d here. On failure nothing took
                    // ownership, so this is the only chance to release it.
                    if SetClipboardData(CF_DIBV5.0 as u32, Some(HANDLE(hglobal.0))).is_err() {
                        let _ = GlobalFree(Some(hglobal));
                        Err(BackendError::PlatformFailure {
                            context: "SetClipboardData".to_string(),
                        })
                    } else {
                        Ok(())
                    }
                }
            }
            Err(_) => Err(BackendError::PlatformFailure {
                context: "GlobalAlloc".to_string(),
            }),
        };
        let _ = CloseClipboard();
        result
    }
}

/// Register "HTML Format" and write [`build_cf_html`]'s payload to the
/// clipboard, plus `alt_text` as a `CF_UNICODETEXT` fallback in the same
/// open/close session — the plain-text degrade
/// [`Clipboard::write_html`](crate::backend::Clipboard::write_html)'s own
/// doc requires for a target with no HTML-aware paste path.
#[cfg(target_os = "windows")]
fn win_clipboard_write_html(html: &str, alt_text: &str) -> ServiceResult<()> {
    // SAFETY: `OpenClipboard`/`CloseClipboard` are paired on every path
    // (the early `Err` returns are before `OpenClipboard`). Both
    // `GlobalAlloc`/`GlobalLock`/`SetClipboardData` sequences below follow
    // the identical null-check/bounded-copy/ownership-transfer pattern as
    // `win_clipboard_write` and `win_clipboard_write_image`.
    unsafe {
        // `RegisterClipboardFormatW` returns `0` on failure rather than a
        // `Result` — checked explicitly since it isn't a `windows`-crate
        // typed API.
        let html_format = RegisterClipboardFormatW(windows::core::w!("HTML Format"));
        if html_format == 0 {
            return Err(BackendError::PlatformFailure {
                context: "RegisterClipboardFormatW(HTML Format)".to_string(),
            });
        }
        if OpenClipboard(None).is_err() {
            return Err(BackendError::PlatformFailure {
                context: "OpenClipboard".to_string(),
            });
        }
        let _ = EmptyClipboard();
        let result = (|| {
            // `CF_HTML` is a registered *text* format: its block carries
            // a trailing NUL, and the header's byte offsets describe the
            // payload without it. `GlobalAlloc(GMEM_MOVEABLE)` hands back
            // uninitialized bytes, so the terminator has to be written
            // explicitly rather than assumed.
            let html_payload = cf_html_block(html);
            set_clipboard_bytes(html_format, &html_payload)?;
            let wide = wide_nul_terminated(alt_text);
            let alt_bytes = wide.len() * std::mem::size_of::<u16>();
            set_clipboard_bytes(
                CF_UNICODETEXT.0 as u32,
                std::slice::from_raw_parts(wide.as_ptr() as *const u8, alt_bytes),
            )
        })();
        let _ = CloseClipboard();
        result
    }
}

/// `GlobalAlloc`/`GlobalLock`/copy/`SetClipboardData` for a raw byte
/// payload — the shared tail of [`win_clipboard_write_html`]'s two
/// `SetClipboardData` calls (`CF_HTML`-registered format, then the
/// `CF_UNICODETEXT` fallback), each needing its own allocation since
/// `SetClipboardData` takes ownership of exactly one block per call.
/// Caller must already hold the clipboard open.
#[cfg(target_os = "windows")]
unsafe fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> ServiceResult<()> {
    match GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
        Ok(hglobal) => {
            let ptr = GlobalLock(hglobal) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(Some(hglobal));
                return Err(BackendError::PlatformFailure {
                    context: "GlobalLock".to_string(),
                });
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(hglobal);
            // On success the system now owns `hglobal` — it must NOT be
            // `GlobalFree`d here. On failure nothing took ownership, so
            // this is the only chance to release it.
            if SetClipboardData(format, Some(HANDLE(hglobal.0))).is_err() {
                let _ = GlobalFree(Some(hglobal));
                return Err(BackendError::PlatformFailure {
                    context: "SetClipboardData".to_string(),
                });
            }
            Ok(())
        }
        Err(_) => Err(BackendError::PlatformFailure {
            context: "GlobalAlloc".to_string(),
        }),
    }
}

// ─── File dialogs ───────────────────────────────────────────────────────

/// `CoInitializeEx(COINIT_APARTMENTTHREADED)`, once per thread. File
/// dialogs are always shown synchronously from the Win32 message-loop
/// thread. Both `S_OK` (first call on this thread) and `S_FALSE` (already
/// initialized) are "COM is ready"; a genuine failure has no good
/// recovery here. Never paired with `CoUninitialize` — COM stays
/// initialized for the rest of the thread's life.
///
/// `pub(crate)` (not private) since #739: `super::image`'s WIC decode
/// path also needs COM ready before building a factory.
#[cfg(target_os = "windows")]
pub(crate) fn ensure_com_initialized() {
    thread_local! {
        static COM_INITIALIZED: Cell<bool> = const { Cell::new(false) };
    }
    COM_INITIALIZED.with(|done| {
        if !done.get() {
            // SAFETY: `CoInitializeEx` has no preconditions beyond being
            // called on the thread it initializes (it is); the
            // `thread_local` guard above ensures this runs at most once
            // per thread, matching the "never call `CoUninitialize`"
            // posture this function's own doc comment describes.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            done.set(true);
        }
    });
}

/// Apply the common [`FileDialogOptions`] fields to a live
/// `IFileOpenDialog`/`IFileSaveDialog` — both deref to `IFileDialog`, so
/// this takes the shared base type, mirroring
/// `gtk::services::build_file_dialog`'s single builder for both dialog
/// kinds. `initial_name` is passed separately rather than read off
/// `opts.initial_filename` because it's save-dialog-only — the open-dialog
/// caller always passes `None`.
#[cfg(target_os = "windows")]
fn configure_file_dialog(
    dialog: &IFileDialog,
    opts: &FileDialogOptions,
    initial_name: Option<&str>,
) -> windows::core::Result<()> {
    // SAFETY: `dialog` is a live COM interface pointer the caller owns
    // for the duration of this call. Every `PCWSTR::from_raw` below
    // points at a `wide_nul_terminated` buffer that outlives its use —
    // `title`/`dir`/`name` are locals borrowed for the single `Set*`
    // call that consumes them synchronously, and `buffers` (backing
    // `specs`) is kept alive through the `SetFileTypes` call that reads
    // it, per the comment at that call site.
    unsafe {
        if let Some(ref title) = opts.title {
            dialog.SetTitle(PCWSTR::from_raw(wide_nul_terminated(title).as_ptr()))?;
        }
        if let Some(ref dir) = opts.initial_dir {
            if let Some(dir_str) = dir.to_str() {
                let path = wide_nul_terminated(dir_str);
                if let Ok(item) = SHCreateItemFromParsingName::<_, Option<&IBindCtx>, IShellItem>(
                    PCWSTR::from_raw(path.as_ptr()),
                    None,
                ) {
                    // `SetFolder` failing (e.g. the directory no longer
                    let _ = dialog.SetFolder(&item);
                }
            }
        }
        if let Some(name) = initial_name {
            dialog.SetFileName(PCWSTR::from_raw(wide_nul_terminated(name).as_ptr()))?;
        }
        if !opts.filters.is_empty() {
            // `COMDLG_FILTERSPEC` only stores pointers, so the backing
            // `Vec<u16>` buffers must outlive the `SetFileTypes` call —
            // kept alive in `buffers` alongside the specs that borrow it.
            let buffers: Vec<(Vec<u16>, Vec<u16>)> = opts
                .filters
                .iter()
                .map(|(name, exts)| {
                    let spec = exts
                        .iter()
                        .map(|ext| format!("*.{ext}"))
                        .collect::<Vec<_>>()
                        .join(";");
                    (wide_nul_terminated(name), wide_nul_terminated(&spec))
                })
                .collect();
            let specs: Vec<COMDLG_FILTERSPEC> = buffers
                .iter()
                .map(|(name, spec)| COMDLG_FILTERSPEC {
                    pszName: PCWSTR::from_raw(name.as_ptr()),
                    pszSpec: PCWSTR::from_raw(spec.as_ptr()),
                })
                .collect();
            dialog.SetFileTypes(&specs)?;
        }
    }
    Ok(())
}

/// Resolve a chosen `IShellItem` back to a filesystem `PathBuf` via
/// `SIGDN_FILESYSPATH`, freeing the COM-allocated string afterward.
#[cfg(target_os = "windows")]
fn shell_item_path(item: &IShellItem) -> Option<PathBuf> {
    // SAFETY: `item` is a live COM interface pointer the caller owns.
    // `GetDisplayName` returns a `PWSTR` this process now owns and must
    // free — `pwstr.to_string()` reads it before that ownership is given
    // up, and `CoTaskMemFree` is called exactly once immediately after,
    // matching `GetDisplayName`'s documented "caller frees with
    // `CoTaskMemFree`" contract.
    unsafe {
        let pwstr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = pwstr.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));
        text.map(PathBuf::from)
    }
}

#[cfg(target_os = "windows")]
fn win_show_open_dialog(owner: Option<HWND>, opts: &FileDialogOptions) -> Option<PathBuf> {
    ensure_com_initialized();
    // SAFETY: COM is initialized on this thread by the call above.
    // `CoCreateInstance` returns a COM interface this scope owns and
    // releases when `dialog` drops; `Show`/`GetResult` are ordinary COM
    // calls through that live pointer.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        configure_file_dialog(&dialog, opts, None).ok()?;
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}

/// The shell folder-picker: the same `IFileOpenDialog` as
/// [`win_show_open_dialog`], with `FOS_PICKFOLDERS` set on its options —
/// the documented way to turn the common file-open dialog into a
/// directory chooser (Microsoft's own `IFileDialog::SetOptions` docs name
/// this exact flag for exactly this purpose; there is no separate
/// "folder dialog" COM object). `configure_file_dialog`'s
/// `initial_name`/filters parameters aren't relevant to a directory
/// chooser, so this passes `None` for `initial_name` and lets
/// `configure_file_dialog` skip `SetFileTypes` when `opts.filters` is
/// empty, same as `win_show_open_dialog` does for `opts.initial_filename`.
#[cfg(target_os = "windows")]
fn win_show_folder_open_dialog(owner: Option<HWND>, opts: &FileDialogOptions) -> Option<PathBuf> {
    ensure_com_initialized();
    // SAFETY: same as `win_show_open_dialog`'s borrow above — COM is
    // initialized on this thread first, and `dialog` is a live,
    // scope-owned COM interface for every call below.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        let current_options = dialog.GetOptions().ok()?;
        dialog.SetOptions(current_options | FOS_PICKFOLDERS).ok()?;
        configure_file_dialog(&dialog, opts, None).ok()?;
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}

#[cfg(target_os = "windows")]
fn win_show_save_dialog(owner: Option<HWND>, opts: &FileDialogOptions) -> Option<PathBuf> {
    ensure_com_initialized();
    // SAFETY: same as `win_show_open_dialog`'s borrow above — COM is
    // initialized on this thread first, and `dialog` is a live,
    // scope-owned COM interface for every call below.
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        configure_file_dialog(&dialog, opts, opts.initial_filename.as_deref()).ok()?;
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}

// ─── Message dialog ─────────────────────────────────────────────────────

/// Win32 reserves ids 1–11 for its own common-button set (`IDOK` = 1,
/// `IDCANCEL` = 2, `IDABORT` = 3, …, `IDCONTINUE` = 11 — see
/// `Win32_UI_WindowsAndMessaging`'s `MESSAGEBOX_RESULT` constants).
/// `MessageDialogOptions::buttons` is an arbitrary caller-declared set
/// with no relation to that table, so every custom
/// [`TASKDIALOG_BUTTON::nButtonID`] below starts past it — the range
/// Microsoft's own `TaskDialogIndirect` docs recommend for
/// application-defined buttons.
#[cfg(target_os = "windows")]
const FIRST_CUSTOM_BUTTON_ID: i32 = 100;

/// The reserved id `TaskDialogIndirect` resolves Escape / Alt-F4 / the
/// window's close box to, when [`TDF_ALLOW_DIALOG_CANCELLATION`] is set
/// and no button already owns it (`TaskDialogIndirect`'s own docs on
/// `IDCANCEL`). [`win_show_message_dialog`] assigns this id to the first
/// [`MessageDialogButton::is_cancel`] button (if any) so that dismissal
/// gesture round-trips to the caller's own cancel button instead of a
/// raw id `opts.buttons` has no entry for.
#[cfg(target_os = "windows")]
const TASKDIALOG_IDCANCEL: i32 = 2;

/// Assign each of `opts.buttons` a `TASKDIALOG_BUTTON` id: the first
/// `is_cancel` button (if any) gets [`TASKDIALOG_IDCANCEL`] so Escape/the
/// close box maps back to it; every other button gets a sequential id
/// from [`FIRST_CUSTOM_BUTTON_ID`]. Mirrors
/// `gtk::services::hig_button_order`'s job of reconciling
/// `MessageDialogOptions`' caller-declared button list with a native
/// widget's own id/ordering scheme, minus the reordering GNOME HIG
/// wants and Win32 doesn't.
#[cfg(target_os = "windows")]
fn assign_button_ids(buttons: &[MessageDialogButton]) -> Vec<i32> {
    let mut cancel_assigned = false;
    buttons
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if b.is_cancel && !cancel_assigned {
                cancel_assigned = true;
                TASKDIALOG_IDCANCEL
            } else {
                FIRST_CUSTOM_BUTTON_ID + i as i32
            }
        })
        .collect()
}

/// `TaskDialogIndirect` — the common-controls v6 alert,
/// preferred over the legacy `MessageBoxW` per this module's doc
/// comment. Blocking: returns once the user picks a button or dismisses
/// the dialog.
#[cfg(target_os = "windows")]
fn win_show_message_dialog(
    owner: Option<HWND>,
    opts: &MessageDialogOptions,
) -> Option<MessageDialogChoice> {
    // `TaskDialogIndirect` needs `CoInitializeEx`/`OleInitialize` to have
    // run on this thread (Microsoft's own docs on the API), same
    // requirement as the `IFileOpenDialog`/`IFileSaveDialog` calls below
    // and the WIC decode path in `image.rs` — see `ensure_com_initialized`'s
    // doc comment for why every native-dialog-style call on this thread
    // goes through it first.
    ensure_com_initialized();

    let ids = assign_button_ids(&opts.buttons);

    // Every wide buffer referenced by pointer below (`title_wide`,
    // `body_wide`, `label_wides`) must outlive the `TaskDialogIndirect`
    // call — kept alive as locals for the rest of this function, same
    // contract as `configure_file_dialog`'s `buffers`.
    let title_wide = wide_nul_terminated(&opts.title);
    let body_wide = wide_nul_terminated(&opts.body);
    let label_wides: Vec<Vec<u16>> = opts
        .buttons
        .iter()
        .map(|b| wide_nul_terminated(&b.label))
        .collect();
    let button_specs: Vec<TASKDIALOG_BUTTON> = ids
        .iter()
        .zip(label_wides.iter())
        .map(|(&id, wide)| TASKDIALOG_BUTTON {
            nButtonID: id,
            pszButtonText: PCWSTR::from_raw(wide.as_ptr()),
        })
        .collect();

    let default_id = opts
        .buttons
        .iter()
        .zip(ids.iter())
        .find(|(b, _)| b.is_default)
        .map(|(_, &id)| id)
        .unwrap_or(0);

    // `TaskDialogIndirect` has no question-mark icon slot at all — the
    // Vista UX guidelines that introduced it deliberately dropped the
    // classic `MB_ICONQUESTION` mark, so `Question` degrades to neutral
    // here (same as `None`).
    let icon = match opts.severity {
        Some(DialogSeverity::Error) => TD_ERROR_ICON,
        Some(DialogSeverity::Warning) => TD_WARNING_ICON,
        Some(DialogSeverity::Info) => TD_INFORMATION_ICON,
        Some(DialogSeverity::Question) | None => PCWSTR::null(),
    };

    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.unwrap_or_default(),
        // Always on: this is what makes `TaskDialogIndirect` resolve
        // Escape / Alt-F4 / the close box to `TASKDIALOG_IDCANCEL`
        // rather than leaving the dialog unclosable — see
        // `TASKDIALOG_IDCANCEL`'s doc comment for how that then maps
        // back to a caller button (or `None`, matching this trait
        // method's documented "dismissed without choosing" contract).
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszMainInstruction: PCWSTR::from_raw(title_wide.as_ptr()),
        pszContent: PCWSTR::from_raw(body_wide.as_ptr()),
        cButtons: button_specs.len() as u32,
        pButtons: button_specs.as_ptr(),
        nDefaultButton: default_id,
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: icon },
        ..Default::default()
    };

    let mut chosen: i32 = 0;
    // SAFETY: `config`'s `pszMainInstruction`/`pszContent` and every
    // `pButtons` entry's `pszButtonText` point at `title_wide`/
    // `body_wide`/`label_wides`, all locals that outlive this call
    // (dropped only when this function returns, after
    // `TaskDialogIndirect` — which is blocking, per this function's own
    // doc comment — has already completed). `chosen` is a plain stack
    // `i32` the call writes through.
    unsafe {
        TaskDialogIndirect(&config, Some(&mut chosen), None, None).ok()?;
    }
    ids.iter()
        .position(|&id| id == chosen)
        .map(|i| opts.buttons[i].id)
}

// ─── Notifications ──────────────────────────────────────────────────────

/// Monotonic per-process tray-icon id, so two notifications fired close
/// together (before the first's removal thread wakes up) each get their
/// own `Shell_NotifyIconW` slot instead of colliding on `NIM_ADD` with an
/// id that's already in use.
#[cfg(target_os = "windows")]
static NEXT_NOTIFICATION_ID: AtomicU32 = AtomicU32::new(1);

/// How long the tray icon backing a balloon tip stays alive before this
/// module removes it again. The OS auto-dismisses the *balloon* well
/// before this, but the icon itself is separate state that would
/// otherwise accumulate in the notification area for the rest of the
/// process's life — there is no "just show a balloon and forget it" API,
/// so cleanup is this module's job.
#[cfg(target_os = "windows")]
const NOTIFICATION_ICON_LIFETIME: std::time::Duration = std::time::Duration::from_secs(8);

/// Escape the five XML-significant characters so `n.title`/`n.body`/an
/// action's label can't break a `<text>`/`arguments` attribute they're
/// interpolated into in [`build_toast_xml`] — `XmlDocument::LoadXml` would
/// otherwise reject (or misparse) a title containing e.g. `&` or `<`. Also
/// drops C0 control characters other than tab/`\n`/`\r` (none of which are
/// legal in well-formed XML 1.0, escaped or not) rather than let one poison
/// `LoadXml` and demote the whole call to the balloon fallback.
///
/// Host-independent (no Windows types involved) — unit-tested off Windows,
/// same as [`decode_dib`]/[`encode_dibv5`].
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\u{9}' | '\u{a}' | '\u{d}' => out.push(c),
            '\u{fffe}' | '\u{ffff}' => {}
            c if c.is_control() => {}
            _ => out.push(c),
        }
    }
    out
}

/// Build the `ToastGeneric` XML [`win_send_notification_toast`] loads into
/// an `XmlDocument`. Each registered action becomes an `<action>` button
/// whose `arguments` attribute is that action's `WidgetId::as_str()`; the
/// toast's own `launch` attribute (what `Activated` reports when the body
/// itself, not a button, is clicked) is left empty. Empty-string-means-
/// `None` is the same convention `gtk::services::decode_notification_target`
/// uses for its `(ss)` variant, so the `Activated` handler below can tell
/// "body click" and "button click" apart with one `is_empty` check on
/// whatever `ToastActivatedEventArgs::Arguments` returns. `is_silent()`
/// becomes a `<audio silent="true"/>` sibling of `<visual>`/`<actions>` —
/// `ToastGeneric`'s documented way to suppress the notification sound,
/// the toast-path equivalent of the balloon fallback's `NIIF_NOSOUND`.
///
/// Host-independent (no Windows types involved) — unit-tested off Windows,
/// same as [`decode_dib`]/[`encode_dibv5`].
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_toast_xml(n: &Notification) -> String {
    use std::fmt::Write as _;

    let mut actions = String::new();
    for (id, label) in n.actions() {
        let _ = write!(
            actions,
            "<action content=\"{}\" arguments=\"{}\" activationType=\"foreground\"/>",
            xml_escape(label),
            xml_escape(id.as_str()),
        );
    }
    let actions_block = if actions.is_empty() {
        String::new()
    } else {
        format!("<actions>{actions}</actions>")
    };
    let audio_block = if n.is_silent() {
        "<audio silent=\"true\"/>"
    } else {
        ""
    };
    format!(
        "<toast launch=\"\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>{audio_block}{actions_block}</toast>",
        xml_escape(&n.title),
        xml_escape(&n.body),
    )
}

/// Id [`WinPlatformServices::active_toasts`] is keyed on — private to this
/// module, since a toast's own `Tag` is caller-supplied and optional and
/// so can't serve as the map key by itself.
#[cfg(target_os = "windows")]
static NEXT_TOAST_ID: AtomicU64 = AtomicU64::new(1);

/// Longest `Tag` string `ToastNotification::SetTag` accepts (Windows 10
/// version 1709+; older releases cap at 16). `SetTag` errors outright
/// above this, which would otherwise demote the whole call to the
/// balloon fallback — losing action buttons and click-through — just
/// because a caller passed a longer tag than Windows' own `Tag` property
/// can hold.
#[cfg(target_os = "windows")]
const TOAST_TAG_MAX_LEN: usize = 64;

/// Primary path for [`WinPlatformServices::send_notification`]: build a
/// `ToastGeneric` toast from `n` (see [`build_toast_xml`]), show it
/// through a notifier scoped to `aumid`, and wire its `Activated` event to
/// push a [`UiEvent::NotificationActivated`] onto `queue` and wake the UI
/// thread with the same `WM_QUADRAUI_USER_EVENT` nudge
/// [`crate::backend::Backend::waker`] uses — `Activated` fires on an
/// arbitrary COM thread, not the window's own.
///
/// The notifier and toast are inserted into `active_toasts` *before*
/// `Show` is called, and removed again once the toast can no longer be
/// activated — see that field's own doc for the exact rule (it survives
/// an ordinary popup timeout, since the toast is then still live in
/// Action Center). Dropping either COM object immediately unregisters
/// the `Activated` handler just wired up below, which would otherwise
/// break click-through the instant this function returns. Any earlier
/// entry sharing the new toast's `tag` is pruned first, mirroring the
/// same "replace, don't stack" behavior Windows itself gives `Tag` in
/// Action Center.
///
/// Errors (the caller falls back to [`win_send_notification_balloon`] on
/// any of them) if WinRT toast notifications aren't available on this
/// system, or if `aumid` isn't a notifier id WinRT recognizes —
/// `CreateToastNotifierWithId` is the call here most likely to fail that
/// way.
#[cfg(target_os = "windows")]
fn win_send_notification_toast(
    owner: Option<HWND>,
    aumid: &str,
    queue: Arc<Mutex<VecDeque<UiEvent>>>,
    active_toasts: ActiveToasts,
    n: &Notification,
) -> windows::core::Result<()> {
    // WinRT activation (`RoActivateInstance`, underneath every call below)
    // needs COM initialized on this thread first, or it fails outright
    // with `CO_E_NOTINITIALIZED`.
    ensure_com_initialized();

    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(build_toast_xml(n)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;

    // `tag` is what `UiEvent::NotificationActivated` must report and what
    // the same-tag `retain` below matches against — both documented to
    // round-trip the caller's `Notification::tag()` unchanged. Only the
    // `SetTag` call itself needs the length-capped copy, since Windows
    // errors outright above `TOAST_TAG_MAX_LEN`.
    let tag = n.tag().map(str::to_string);
    if let Some(tag) = &tag {
        let truncated_tag = tag.chars().take(TOAST_TAG_MAX_LEN).collect::<String>();
        toast.SetTag(&HSTRING::from(truncated_tag.as_str()))?;
    }

    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))?;
    let id = NEXT_TOAST_ID.fetch_add(1, Ordering::Relaxed);

    // `HWND` isn't `Send`; carry its raw value into the closures below and
    // reconstruct it there, same as the balloon-removal thread does.
    let hwnd_value = owner.map(|h| h.0 as isize);

    let activated_tag = tag.clone();
    let activated_toasts = Arc::clone(&active_toasts);
    let activated_handler =
        TypedEventHandler::<ToastNotification, windows::core::IInspectable>::new(
            move |_sender, args| -> windows::core::Result<()> {
                // The user has acted on this toast, so it is no longer
                // live in Action Center either way — safe to release the
                // retained `ToastNotifier`/`ToastNotification` pair now.
                activated_toasts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);

                let Some(args) = args.as_ref() else {
                    return Ok(());
                };
                let Ok(args) = args.cast::<ToastActivatedEventArgs>() else {
                    return Ok(());
                };
                let arguments = args.Arguments().unwrap_or_default().to_string();
                let action = if arguments.is_empty() {
                    None
                } else {
                    Some(WidgetId::new(arguments))
                };
                queue
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push_back(UiEvent::NotificationActivated {
                        tag: activated_tag.clone(),
                        action,
                    });
                if let Some(hwnd_value) = hwnd_value {
                    // SAFETY: `hwnd_value` is a raw `HWND` value captured by
                    // value before the toast was shown; `PostMessageW` only
                    // enqueues a message for the window identified by that
                    // handle, it never dereferences anything beyond it.
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_value as *mut core::ffi::c_void)),
                            crate::win::backend::WM_QUADRAUI_USER_EVENT,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
                Ok(())
            },
        );
    toast.Activated(&activated_handler)?;

    // `Dismissed` fires whenever the toast *leaves the screen*, which
    // includes the ordinary popup timeout (`ToastDismissalReason::
    // TimedOut`) as well as the user explicitly closing it
    // (`UserCanceled`) or the notification platform hiding it
    // (`ApplicationHidden`). A `TimedOut` dismissal just means the toast
    // has moved from the popup banner into Action Center — it is still
    // live there, waiting for a click, so the retained pair must survive
    // it. Only `UserCanceled`/`ApplicationHidden` mean the toast is truly
    // gone with no further chance of activation; those are the only
    // reasons this handler prunes `active_toasts`. The remaining case —
    // the user activating the toast — is pruned by the `Activated`
    // handler above instead, so every live toast still ends up bounded.
    let dismissed_toasts = Arc::clone(&active_toasts);
    let dismissed_handler = TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(
        move |_sender, args| -> windows::core::Result<()> {
            let timed_out = args
                .as_ref()
                .and_then(|args| args.Reason().ok())
                .is_some_and(|reason| reason == ToastDismissalReason::TimedOut);
            if !timed_out {
                dismissed_toasts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
            }
            Ok(())
        },
    );
    toast.Dismissed(&dismissed_handler)?;

    let failed_toasts = Arc::clone(&active_toasts);
    let failed_handler = TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
        move |_sender, _args| -> windows::core::Result<()> {
            failed_toasts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            Ok(())
        },
    );
    toast.Failed(&failed_handler)?;

    {
        let mut toasts = active_toasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(tag) = &tag {
            toasts.retain(|_, (existing_tag, ..)| existing_tag.as_deref() != Some(tag.as_str()));
        }
        toasts.insert(id, (tag.clone(), notifier.clone(), toast.clone()));
    }

    // A toast that never shows will never fire `Dismissed`/`Activated`/
    // `Failed` either, so on a synchronous `Show` error the entry just
    // inserted above has to be removed here instead — otherwise it (and
    // the two COM objects it holds) leaks for the rest of the process.
    if let Err(e) = notifier.Show(&toast) {
        active_toasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
        return Err(e);
    }
    Ok(())
}

/// Fallback path for [`WinPlatformServices::send_notification`]: add a
/// transient tray icon carrying `n`'s title/body as an `NIF_INFO` balloon,
/// then remove it again after [`NOTIFICATION_ICON_LIFETIME`] (see that
/// constant's docs). A no-op if no window has attached yet (`owner` is
/// `None`) — there's no `HWND` to own the tray icon. Used only when the
/// WinRT toast path ([`win_send_notification_toast`]) isn't available (no
/// AUMID registered) or fails.
#[cfg(target_os = "windows")]
fn win_send_notification_balloon(owner: Option<HWND>, n: &Notification) {
    let Some(hwnd) = owner else {
        return;
    };
    // SAFETY: `hwnd` came from the caller as a live window handle.
    // `LoadIconW(None, ..)` loads a shared system icon (no handle to
    // free). `data.szInfo`/`szInfoTitle` are only ever written through
    // `copy_wide_truncated`, which bounds every write to the fixed-size
    // array itself — never a raw pointer. The spawned thread below
    // reconstructs its own `HWND` from a plain `isize`, which is sound
    // because `HWND` is itself just a wrapped pointer value with no
    // per-thread state; it and the moved `uid` only feed a
    // `Shell_NotifyIconW(NIM_DELETE, ..)` call that needs no live
    // reference back into this function's stack.
    unsafe {
        let icon_resource = if n.urgent { IDI_ERROR } else { IDI_INFORMATION };
        let icon = LoadIconW(None, icon_resource).unwrap_or(HICON(std::ptr::null_mut()));
        let uid = NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);

        // Only `silent` from the extended `Notification` fields has a
        // place on this balloon-tip path — `NIIF_NOSOUND` is a real,
        // documented `dwInfoFlags` bit. `icon`, `actions`, and `tag` are
        // dropped: a balloon has no caller-supplied icon slot beyond the
        // severity icon, no button/action UI, and no click-through channel
        // to report an activation. `win_send_notification_toast` covers all
        // three when a toast notifier is available; this path only runs as
        // its fallback.
        let mut severity_flags = if n.urgent { NIIF_ERROR } else { NIIF_INFO };
        if n.is_silent() {
            severity_flags |= NIIF_NOSOUND;
        }
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: uid,
            uFlags: NIF_ICON | NIF_INFO,
            hIcon: icon,
            dwInfoFlags: severity_flags,
            ..Default::default()
        };
        copy_wide_truncated(&mut data.szInfo, &n.body);
        copy_wide_truncated(&mut data.szInfoTitle, &n.title);

        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return;
        }

        // `HWND` isn't `Send`; carry its raw value across the thread
        // boundary and reconstruct it there instead.
        let hwnd_value = hwnd.0 as isize;
        std::thread::spawn(move || {
            std::thread::sleep(NOTIFICATION_ICON_LIFETIME);
            let removal = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: HWND(hwnd_value as *mut core::ffi::c_void),
                uID: uid,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &removal);
        });
    }
}

// ─── open_url / open_path / open_url_result ──────────────────────────────
//
// Both go through `crate::desktop::open_with_default` — the
// crate-wide opener shared with `tui::services`/`macos::services` — rather
// than a Win-local `ShellExecuteW` call: it hands `"open"` + `target` to
// `ShellExecuteW` through the same minimal hand-written FFI declaration on
// every backend, so there is nothing Win-specific left to write here. See
// that function's doc for why it's a hand-written FFI call rather than the
// `windows` crate's own `ShellExecuteW` binding.

// ─── reveal_in_file_manager ──────────────────────────────────────────────

/// `SHOpenFolderAndSelectItems` — "Reveal in Explorer." Needs two PIDLs:
/// the *containing folder*'s absolute PIDL, and the *item*'s PIDL
/// relative to it. `ILCreateFromPathW` builds one absolute PIDL for the
/// whole path; `ILFindLastID` finds the pointer to its last segment
/// (the item itself) without copying, and `ILRemoveLastID` truncates a
/// PIDL down to its parent *in place*, by zeroing the length prefix of
/// what was the last segment.
///
/// That in-place zeroing is why this clones the full PIDL before
/// truncating rather than truncating `full_pidl` directly: `pidl_last`
/// (from `ILFindLastID`) points *into* `full_pidl`'s own buffer, so
/// truncating that same buffer would zero out the very segment
/// `pidl_last` still needs to hand `SHOpenFolderAndSelectItems` a valid
/// length-prefixed item id. Truncating an independent `ILClone` instead
/// leaves `full_pidl` — and the `pidl_last` pointer into it — untouched.
/// This is the documented Win32 idiom for this API (Microsoft's own
/// samples pair `SHOpenFolderAndSelectItems` with exactly this
/// clone-then-truncate sequence), not a novel workaround.
#[cfg(target_os = "windows")]
fn win_reveal_in_file_manager(path: &Path) -> ServiceResult<()> {
    ensure_com_initialized();
    let wide = wide_nul_terminated(&path.to_string_lossy());
    // SAFETY: `wide` outlives the single `ILCreateFromPathW` call that
    // reads it. `full_pidl`/`folder_pidl` are checked for null before
    // use; `pidl_last` points into `full_pidl`'s buffer (per
    // `ILFindLastID`'s contract) and is never dereferenced after
    // `full_pidl` is freed — the doc comment above explains why
    // `folder_pidl` is truncated instead of `full_pidl` to keep that
    // pointer valid through the `SHOpenFolderAndSelectItems` call. Both
    // PIDLs are freed exactly once each via `ILFree`, on every path
    // (including the two early `Err` branches, which only skip the
    // `SHOpenFolderAndSelectItems` call — `folder_pidl`/`full_pidl` are
    // still allocated by then and still need freeing, which the
    // `result` variable computed by the `if`/`else` lets the two
    // trailing `ILFree` calls handle uniformly).
    unsafe {
        let full_pidl = ILCreateFromPathW(PCWSTR::from_raw(wide.as_ptr()));
        if full_pidl.is_null() {
            return Err(BackendError::PlatformFailure {
                context: "ILCreateFromPathW returned null".to_string(),
            });
        }
        let pidl_last: *const ITEMIDLIST = ILFindLastID(full_pidl);
        let folder_pidl = ILClone(full_pidl);
        let result = if folder_pidl.is_null() {
            Err(BackendError::PlatformFailure {
                context: "ILClone returned null".to_string(),
            })
        } else if !ILRemoveLastID(Some(folder_pidl)).as_bool() {
            // `false` means `folder_pidl` had no last id to remove at
            // all — i.e. `path` resolved to the desktop root, with no
            // parent folder to reveal it in.
            Err(BackendError::PlatformFailure {
                context: "ILRemoveLastID: path has no parent folder".to_string(),
            })
        } else {
            let children: [*const ITEMIDLIST; 1] = [pidl_last];
            SHOpenFolderAndSelectItems(folder_pidl, Some(&children), 0).map_err(|e| {
                BackendError::PlatformFailure {
                    context: format!("SHOpenFolderAndSelectItems: {e}"),
                }
            })
        };
        ILFree(Some(folder_pidl));
        ILFree(Some(full_pidl));
        result
    }
}

// ─── beep ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_beep() -> ServiceResult<()> {
    // SAFETY: `MessageBeep` takes no pointers and has no preconditions
    // beyond a valid sound-type flag; `MB_OK` is one of the documented
    // constants.
    unsafe { MessageBeep(MB_OK) }.map_err(|e| BackendError::PlatformFailure {
        context: format!("MessageBeep: {e}"),
    })
}

// ─── System theme ───────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_system_theme() -> ServiceResult<SystemTheme> {
    let platform_failure = |context: &str| BackendError::PlatformFailure {
        context: context.to_string(),
    };
    let settings = UISettings::new()
        .map_err(|_| platform_failure("windows::UI::ViewManagement::UISettings::new"))?;
    let bg = settings
        .GetColorValue(UIColorType::Background)
        .map_err(|_| platform_failure("UISettings::GetColorValue(Background)"))?;
    let accent = settings
        .GetColorValue(UIColorType::Accent)
        .map_err(|_| platform_failure("UISettings::GetColorValue(Accent)"))?;
    // High contrast is best-effort: a failure to read it degrades to
    // `false` rather than failing the whole query — the caller asked for
    // the theme, and background/accent above already answered that; one
    // missing accessibility flag shouldn't turn a real answer into
    // `Unsupported`.
    let high_contrast = AccessibilitySettings::new()
        .and_then(|a| a.HighContrast())
        .unwrap_or(false);
    // Same best-effort posture as `high_contrast` above: a failure to read
    // the client-area-animation setting degrades to "animations on" (i.e.
    // `reduce_motion: false`) rather than failing the whole query.
    let reduce_motion = !win_client_area_animation_enabled().unwrap_or(true);
    Ok(system_theme_from_ui_colors(
        (bg.R, bg.G, bg.B),
        (accent.R, accent.G, accent.B),
        high_contrast,
        reduce_motion,
    ))
}

/// `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)` — Windows' own
/// reduce-motion signal (Settings → Accessibility → Visual effects →
/// "Animation effects"). `true` means animations are enabled (motion is
/// *not* reduced); [`win_system_theme`] inverts this into
/// [`SystemTheme::reduce_motion`].
#[cfg(target_os = "windows")]
fn win_client_area_animation_enabled() -> ServiceResult<bool> {
    let mut enabled = BOOL(0);
    // SAFETY: `SPI_GETCLIENTAREAANIMATION` is a `BOOL`-out query — `pvparam`
    // must point at a writable `BOOL`, which `enabled` is; `fwinini` is
    // ignored for "get" actions so `0` (no `SPIF_*` flags) is correct here.
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map_err(|e| BackendError::PlatformFailure {
        context: format!("SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION): {e}"),
    })?;
    Ok(enabled.as_bool())
}

/// Pure mapping from `UISettings`' background/accent RGB triples plus the
/// high-contrast and reduce-motion flags to [`SystemTheme`] — split out so
/// it's unit-testable without the real WinRT
/// `UISettings`/`AccessibilitySettings` classes, which this crate can't
/// construct on a non-Windows host (same cross-platform-testable-helper
/// posture as [`wide_nul_terminated`] above; see that function's doc for
/// why this is `allow`-gated rather than `cfg`-gated).
///
/// `dark` is derived from `bg`'s perceptual luminance (ITU-R BT.601 luma,
/// integer arithmetic) rather than a dedicated "is dark mode" WinRT
/// property — `UISettings` only exposes raw colour values, not a boolean.
/// A luma below 50% reads as the dark theme, matching the threshold
/// Windows' own light/dark background colours (`#FFFFFF` vs `#000000`)
/// land unambiguously on either side of.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn system_theme_from_ui_colors(
    bg: (u8, u8, u8),
    accent: (u8, u8, u8),
    high_contrast: bool,
    reduce_motion: bool,
) -> SystemTheme {
    let (r, g, b) = bg;
    let luma = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    SystemTheme {
        dark: luma < 128,
        accent: Some(Color::rgb(accent.0, accent.1, accent.2)),
        high_contrast,
        reduce_motion,
    }
}

// ─── Displays ───────────────────────────────────────────────────────────

/// `EnumDisplayMonitors` callback: appends the enumerated `HMONITOR` to
/// the `Vec<HMONITOR>` `lparam` points at. Collecting handles first and
/// querying each with `GetMonitorInfoW`/`GetDpiForMonitor` afterwards —
/// rather than doing that work inside the callback itself, which runs on
/// an arbitrary call stack inside `EnumDisplayMonitors` — keeps this
/// `extern "system"` trampoline to the bare minimum FFI surface.
///
/// # Safety
///
/// `lparam` must be `LPARAM(&mut Vec<HMONITOR> as *mut _ as isize)` —
/// the exact contract [`win_displays`], its only caller, upholds.
#[cfg(target_os = "windows")]
unsafe extern "system" fn win_collect_monitor(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _clip_rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    monitors.push(hmonitor);
    // `TRUE` — continue enumeration; returning `FALSE` would stop
    // `EnumDisplayMonitors` early and silently drop the remaining
    // monitors. `windows::core::BOOL` is a newtype over `i32` in this
    // crate version (re-exported from `windows-result`), not a bare
    // `i32` alias, so the value has to be wrapped rather than returned
    // as a plain integer literal.
    BOOL(1)
}

/// `EnumDisplayMonitors` + `GetMonitorInfoW` for
/// `bounds`/`work_area`/`primary`, `GetDpiForMonitor` for `scale`
/// — see [`PlatformServices::displays`]'s doc for the full
/// field-by-field contract.
#[cfg(target_os = "windows")]
fn win_displays() -> ServiceResult<Vec<Display>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `win_collect_monitor` only ever dereferences `lparam` as
    // the `Vec<HMONITOR>` constructed on the line above, which outlives
    // the call (`EnumDisplayMonitors` is synchronous — it returns only
    // after every callback invocation has completed).
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(win_collect_monitor),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        );
    }
    let mut out = Vec::with_capacity(handles.len());
    for hmonitor in handles {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info.monitorInfo.cbSize` is set to `MONITORINFOEXW`'s
        // size immediately above, which is how a caller tells
        // `GetMonitorInfoW` the extended (`szDevice`-carrying) struct was
        // passed rather than the base `MONITORINFO` — the documented
        // Win32 idiom for this API.
        let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo) };
        // `windows::core::BOOL` is a newtype, not a bare `i32` — compare
        // through `as_bool()` rather than against a `0` literal.
        if !ok.as_bool() {
            // A monitor that vanished (unplugged) between `EnumDisplayMonitors`
            // enumerating its handle and this query is skipped rather than
            // failing the whole call — the same "one bad entry doesn't
            // discard the rest" posture `win_system_theme`'s high-contrast
            // fallback already takes.
            continue;
        }
        // DPI is likewise best-effort per monitor: a failure degrades
        // `scale` to 1.0 (`USER_DEFAULT_SCREEN_DPI`'s own ratio) rather
        // than dropping the monitor.
        let mut dpi_x = USER_DEFAULT_SCREEN_DPI;
        let mut dpi_y = USER_DEFAULT_SCREEN_DPI;
        // SAFETY: `hmonitor` came from `EnumDisplayMonitors` above and
        // `GetMonitorInfoW` just proved it's still valid; `dpi_x`/`dpi_y`
        // are plain stack `u32`s `GetDpiForMonitor` writes through.
        let _ = unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let r = info.monitorInfo.rcMonitor;
        let w = info.monitorInfo.rcWork;
        out.push(display_from_monitor_rects(
            (r.left, r.top, r.right, r.bottom),
            (w.left, w.top, w.right, w.bottom),
            (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
            dpi_x,
        ));
    }
    Ok(out)
}

/// `GetCursorPos` — the global cursor position in
/// virtual-screen coordinates, the same coordinate space
/// [`win_displays`]'s `bounds`/`work_area` use (both come from Win32's
/// one virtual-screen coordinate system).
#[cfg(target_os = "windows")]
fn win_cursor_screen_point() -> ServiceResult<Point> {
    let mut point = POINT::default();
    // SAFETY: `point` is a plain stack `POINT` `GetCursorPos` writes
    // through; no other precondition.
    unsafe { GetCursorPos(&mut point) }.map_err(|e| BackendError::PlatformFailure {
        context: format!("GetCursorPos: {e}"),
    })?;
    Ok(Point::new(point.x as f32, point.y as f32))
}

/// Pure mapping from a monitor's `(left, top, right, bottom)` full and
/// work rects, its primary flag, and a horizontal DPI reading, to
/// [`Display`] — split out so the field arithmetic is unit-testable
/// without the real `GetMonitorInfoW`/`GetDpiForMonitor` calls, which
/// this crate can't make on a non-Windows host (same
/// cross-platform-testable-helper posture as [`system_theme_from_ui_colors`]
/// above; see [`wide_nul_terminated`]'s doc for why this is
/// `allow`-gated rather than `cfg`-gated). Takes raw tuples rather than
/// the native `RECT`/`MONITORINFO` types for the same reason —
/// those types themselves are only defined under `cfg(target_os =
/// "windows")`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn display_from_monitor_rects(
    monitor_rect: (i32, i32, i32, i32),
    work_rect: (i32, i32, i32, i32),
    primary: bool,
    dpi: u32,
) -> Display {
    let (ml, mt, mr, mb) = monitor_rect;
    let (wl, wt, wr, wb) = work_rect;
    Display {
        bounds: Rect::new(ml as f32, mt as f32, (mr - ml) as f32, (mb - mt) as f32),
        work_area: Rect::new(wl as f32, wt as f32, (wr - wl) as f32, (wb - wt) as f32),
        // 96.0: `USER_DEFAULT_SCREEN_DPI`'s value, Win32's un-scaled DPI
        // baseline (100% scaling). Inlined rather than imported so this
        // function stays free of the `cfg(target_os = "windows")`-gated
        // `windows` crate import — see this function's own doc.
        scale: dpi as f32 / 96.0,
        primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_win_gui() {
        let svc = WinPlatformServices::new();
        assert_eq!(svc.platform_name(), "win-gui");
    }

    // ── system_theme_from_ui_colors ─────────────────────────────────────

    #[test]
    fn system_theme_from_ui_colors_black_background_is_dark() {
        let theme = system_theme_from_ui_colors((0, 0, 0), (0, 120, 215), false, false);
        assert!(theme.dark);
        assert_eq!(theme.accent, Some(Color::rgb(0, 120, 215)));
        assert!(!theme.high_contrast);
        assert!(!theme.reduce_motion);
    }

    #[test]
    fn system_theme_from_ui_colors_white_background_is_light() {
        let theme = system_theme_from_ui_colors((255, 255, 255), (0, 120, 215), false, false);
        assert!(!theme.dark);
    }

    #[test]
    fn system_theme_from_ui_colors_reports_high_contrast() {
        assert!(system_theme_from_ui_colors((0, 0, 0), (255, 255, 0), true, false).high_contrast);
    }

    #[test]
    fn system_theme_from_ui_colors_reports_reduce_motion() {
        let theme = system_theme_from_ui_colors((0, 0, 0), (255, 255, 0), false, true);
        assert!(theme.reduce_motion);
        let theme = system_theme_from_ui_colors((0, 0, 0), (255, 255, 0), false, false);
        assert!(!theme.reduce_motion);
    }

    // ── display_from_monitor_rects ───────────────────────────────────────

    #[test]
    fn display_from_monitor_rects_converts_rects_to_bounds() {
        // A 1920x1080 primary monitor at the virtual-desktop origin, work
        // area shrunk by a 40px taskbar along the bottom.
        let d = display_from_monitor_rects((0, 0, 1920, 1080), (0, 0, 1920, 1040), true, 96);
        assert_eq!(d.bounds, Rect::new(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(d.work_area, Rect::new(0.0, 0.0, 1920.0, 1040.0));
        assert!(d.primary);
        assert_eq!(d.scale, 1.0);
    }

    #[test]
    fn display_from_monitor_rects_handles_negative_origin_and_non_primary() {
        // A secondary monitor to the left of the primary — negative `x`,
        // using the virtual-desktop coordinate space every monitor shares.
        let d = display_from_monitor_rects((-1920, 0, 0, 1080), (-1920, 0, 0, 1080), false, 96);
        assert_eq!(d.bounds, Rect::new(-1920.0, 0.0, 1920.0, 1080.0));
        assert!(!d.primary);
    }

    #[test]
    fn display_from_monitor_rects_scales_by_dpi() {
        // 192 DPI is Windows' 200% scaling preset.
        let d = display_from_monitor_rects((0, 0, 3840, 2160), (0, 0, 3840, 2120), true, 192);
        assert_eq!(d.scale, 2.0);
    }

    #[test]
    fn wide_round_trips_through_decode() {
        let wide = wide_nul_terminated("hello");
        // encode_utf16 + the trailing NUL this function adds.
        assert_eq!(wide.len(), "hello".len() + 1);
        assert_eq!(*wide.last().unwrap(), 0);
        assert_eq!(decode_wide_nul_terminated(&wide), "hello");
    }

    #[test]
    fn decode_stops_at_first_nul_not_slice_end() {
        let mut wide = wide_nul_terminated("hi");
        wide.extend_from_slice(&[b'X' as u16, b'Y' as u16]);
        assert_eq!(decode_wide_nul_terminated(&wide), "hi");
    }

    #[test]
    fn decode_handles_a_slice_with_no_nul_at_all() {
        let wide: Vec<u16> = "no-nul".encode_utf16().collect();
        assert_eq!(decode_wide_nul_terminated(&wide), "no-nul");
    }

    #[test]
    fn copy_wide_truncated_nul_terminates_within_bounds() {
        let mut dst = [0u16; 8];
        copy_wide_truncated(&mut dst, "hi");
        assert_eq!(decode_wide_nul_terminated(&dst), "hi");
    }

    #[test]
    fn copy_wide_truncated_truncates_text_longer_than_dst() {
        let mut dst = [0u16; 4];
        copy_wide_truncated(&mut dst, "abcdefgh");
        // 4-slot buffer: 3 chars of payload + a forced trailing NUL.
        assert_eq!(decode_wide_nul_terminated(&dst), "abc");
        assert_eq!(dst[3], 0);
    }

    #[test]
    fn copy_wide_truncated_empty_dst_does_not_panic() {
        let mut dst: [u16; 0] = [];
        copy_wide_truncated(&mut dst, "anything");
    }

    // ── extract_channel ──────────────────────────────────────────────────

    #[test]
    fn extract_channel_8bit_mask_round_trips_every_byte_value() {
        // `encode_dibv5`'s own masks are all 8 bits wide — confirm the
        // scaling formula is an exact identity for that width, not just
        // approximately right, for every possible byte.
        for value in 0u32..=255 {
            let pixel = value << 16; // placed at the red mask's position
            assert_eq!(extract_channel(pixel, 0x00FF_0000), value as u8);
        }
    }

    #[test]
    fn extract_channel_zero_mask_is_zero() {
        assert_eq!(extract_channel(0xFFFF_FFFF, 0), 0);
    }

    // ── decode_dib / encode_dibv5 ────────────────────────────────────────

    fn sample_image() -> RgbaImage {
        // 2x2, every pixel a distinct RGBA combination so a transposed
        // row/column or channel swap would be caught.
        RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, // top-left: opaque red
                0, 255, 0, 128, // top-right: half-alpha green
                0, 0, 255, 64, // bottom-left: quarter-alpha blue
                10, 20, 30, 0, // bottom-right: fully transparent
            ],
        }
    }

    #[test]
    fn encode_dibv5_round_trips_through_decode_dib() {
        let image = sample_image();
        let bytes = encode_dibv5(&image);
        let decoded =
            decode_dib(&bytes).expect("decode_dib should accept its own encode_dibv5 output");
        assert_eq!(decoded, image);
    }

    #[test]
    fn decode_dib_handles_bottom_up_bi_bitfields() {
        // `encode_dibv5` always writes top-down (negative height); flip
        // the sign and reverse the row order by hand to exercise
        // `decode_dib`'s bottom-up branch too.
        let image = sample_image();
        let mut bytes = encode_dibv5(&image);
        // Height is a little-endian i32 at byte offset 8.
        let height = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
        assert!(height < 0);
        bytes[8..12].copy_from_slice(&(-height).to_le_bytes());
        let row_bytes = image.width as usize * 4;
        let pixel_start = 124;
        let mut rows: Vec<&[u8]> = bytes[pixel_start..].chunks_exact(row_bytes).collect();
        rows.reverse();
        let flipped_pixels: Vec<u8> = rows.into_iter().flatten().copied().collect();
        bytes[pixel_start..].copy_from_slice(&flipped_pixels);

        let decoded = decode_dib(&bytes).expect("bottom-up BI_BITFIELDS should decode");
        assert_eq!(decoded, image);
    }

    #[test]
    fn decode_dib_classic_24bpp_bi_rgb_is_opaque() {
        // Hand-build a minimal bottom-up 24bpp BI_RGB buffer: a 1x2 image,
        // bottom row red, top row green — classic Explorer/Paint
        // image-copy shape.
        let width = 1u32;
        let height = 2i32;
        let stride = dib_stride_bytes(width, 24);
        let mut buf = vec![0u8; 40 + stride * 2];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes());
        buf[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        buf[8..12].copy_from_slice(&height.to_le_bytes());
        buf[14..16].copy_from_slice(&24u16.to_le_bytes());
        buf[16..20].copy_from_slice(&BI_RGB_VALUE.to_le_bytes());
        // Bottom-up: row 0 in the buffer is the image's bottom row (red,
        // BGR order), row 1 is the top row (green).
        buf[40..43].copy_from_slice(&[0, 0, 255]); // bottom row: red
        buf[40 + stride..40 + stride + 3].copy_from_slice(&[0, 255, 0]); // top row: green

        let decoded = decode_dib(&buf).expect("classic 24bpp BI_RGB should decode");
        assert_eq!(decoded.width, 1);
        assert_eq!(decoded.height, 2);
        assert_eq!(&decoded.pixels[0..4], &[0, 255, 0, 255]); // top row: opaque green
        assert_eq!(&decoded.pixels[4..8], &[255, 0, 0, 255]); // bottom row: opaque red
    }

    #[test]
    fn decode_dib_rejects_unsupported_bit_depth() {
        let mut buf = vec![0u8; 40];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes());
        buf[4..8].copy_from_slice(&1i32.to_le_bytes());
        buf[8..12].copy_from_slice(&1i32.to_le_bytes());
        buf[14..16].copy_from_slice(&8u16.to_le_bytes()); // 8bpp paletted
        assert_eq!(decode_dib(&buf), None);
    }

    #[test]
    fn decode_dib_rejects_truncated_buffer() {
        assert_eq!(decode_dib(&[0u8; 10]), None);
    }

    /// Build the other `BI_BITFIELDS` layout a producer may hand over: a
    /// plain 40-byte `BITMAPINFOHEADER` with three R/G/B mask `DWORD`s in
    /// place of a color table, so the pixel data starts at offset 52.
    fn bitfields_40_byte_header_dib(image: &RgbaImage) -> Vec<u8> {
        let mut buf = vec![0u8; 52];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes());
        buf[4..8].copy_from_slice(&(image.width as i32).to_le_bytes());
        // Negative height: top-down, matching `encode_32bpp_bgra`'s rows.
        buf[8..12].copy_from_slice(&(-(image.height as i32)).to_le_bytes());
        buf[12..14].copy_from_slice(&1u16.to_le_bytes());
        buf[14..16].copy_from_slice(&32u16.to_le_bytes());
        buf[16..20].copy_from_slice(&BI_BITFIELDS_VALUE.to_le_bytes());
        buf[40..44].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
        buf[44..48].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
        buf[48..52].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
        buf.extend_from_slice(&encode_32bpp_bgra(image));
        buf
    }

    #[test]
    fn decode_dib_reads_bitfields_masks_from_a_40_byte_header() {
        // The three-mask layout has no alpha mask at all, so every pixel
        // must come back opaque regardless of the source alpha byte —
        // but the colour channels still have to land in the right place,
        // which only happens if the pixel data is read from offset 52
        // (past the masks) and not from offset 40.
        let image = sample_image();
        let buf = bitfields_40_byte_header_dib(&image);
        let decoded = decode_dib(&buf).expect("40-byte-header BI_BITFIELDS should decode");
        assert_eq!(decoded.width, image.width);
        assert_eq!(decoded.height, image.height);
        for (decoded_px, source_px) in decoded
            .pixels
            .chunks_exact(4)
            .zip(image.pixels.chunks_exact(4))
        {
            assert_eq!(
                decoded_px,
                [source_px[0], source_px[1], source_px[2], 255].as_slice()
            );
        }
    }

    #[test]
    fn decode_dib_rejects_bitfields_header_with_truncated_masks() {
        // 40-byte header claiming `BI_BITFIELDS` but no room for the
        // three mask `DWORD`s that layout requires.
        let image = sample_image();
        let mut buf = bitfields_40_byte_header_dib(&image);
        buf.truncate(48);
        assert_eq!(decode_dib(&buf), None);
    }

    // ── build_cf_html ────────────────────────────────────────────────────

    #[test]
    fn build_cf_html_offsets_point_at_the_right_bytes() {
        let out = build_cf_html("<b>hi</b>");
        let get_offset = |field: &str| -> usize {
            let start = out.find(field).unwrap() + field.len();
            out[start..start + 10].parse().unwrap()
        };
        let start_html = get_offset("StartHTML:");
        let end_html = get_offset("EndHTML:");
        let start_fragment = get_offset("StartFragment:");
        let end_fragment = get_offset("EndFragment:");

        // `StartHTML`/`EndHTML` bound the `<html>...</html>` document,
        // which sits after the CF_HTML header itself — not at byte 0.
        assert_eq!(&out[start_html..start_html + 6], "<html>");
        assert_eq!(end_html, out.len());
        assert_eq!(&out[start_fragment..end_fragment], "<b>hi</b>");
        assert!(out.contains("<!--StartFragment-->"));
        assert!(out.contains("<!--EndFragment-->"));
    }

    #[test]
    fn cf_html_block_is_nul_terminated_past_the_declared_end_html() {
        let block = cf_html_block("<b>hi</b>");
        let payload = build_cf_html("<b>hi</b>");
        assert_eq!(block.last(), Some(&0));
        assert_eq!(&block[..block.len() - 1], payload.as_bytes());
        // `EndHTML` still measures the payload without the terminator, so
        // a consumer honouring it never reads the NUL as document bytes.
        let field = "EndHTML:";
        let start = payload.find(field).unwrap() + field.len();
        let end_html: usize = payload[start..start + 10].parse().unwrap();
        assert_eq!(end_html, block.len() - 1);
    }

    #[test]
    fn build_cf_html_handles_empty_html() {
        let out = build_cf_html("");
        assert!(out.contains("<!--StartFragment--><!--EndFragment-->"));
    }

    /// Off-Windows, every `PlatformServices` method degrades to the
    /// original graceful no-op — mirrors `TuiPlatformServices`'s
    /// unconditional `None`/no-op shape so a plain `cargo test` (no `win`
    /// feature quirks) still exercises the fallback path.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_stubs_degrade_gracefully() {
        let svc = WinPlatformServices::new();
        assert!(svc.clipboard().read_text().is_none());
        svc.clipboard().write_text("ignored");
        // Every `WinClipboard` method that returns a `ServiceResult`
        // degrades to `Unsupported`/`Ok(())` off Windows — `clear` is the
        // one exception, since it has nothing platform-specific to fail
        // on when there's no real clipboard to clear.
        assert_eq!(svc.clipboard().read_image(), Err(BackendError::Unsupported));
        assert_eq!(
            svc.clipboard().write_image(&RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![0, 0, 0, 255],
            }),
            Err(BackendError::Unsupported)
        );
        assert_eq!(
            svc.clipboard().write_html("<b>hi</b>", "hi"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(
            svc.clipboard().read_file_list(),
            Err(BackendError::Unsupported)
        );
        assert_eq!(svc.clipboard().clear(), Ok(()));
        assert!(svc
            .show_file_open_dialog(FileDialogOptions::default())
            .is_none());
        assert!(svc
            .show_file_save_dialog(FileDialogOptions::default())
            .is_none());
        assert!(svc
            .show_folder_open_dialog(FileDialogOptions::default())
            .is_none());
        svc.send_notification(Notification::new("t", "b"));
        assert!(svc
            .show_message_dialog(MessageDialogOptions {
                title: "t".to_string(),
                body: "b".to_string(),
                buttons: Vec::new(),
                severity: None,
            })
            .is_none());
        svc.open_url("https://example.com");
        // `open_url_result` now genuinely overrides the trait's
        // `Ok(())` default (`open_url` then always succeed) — off Windows
        // it honestly reports `Unsupported` rather than silently lying
        // about a launch that never happened.
        assert_eq!(
            svc.open_url_result("https://example.com"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(svc.system_theme(), Err(BackendError::Unsupported));
        // Every one of these four *is* fully implemented on real
        // Windows (unlike `open_url` above, which stays a fire-and-forget
        // no-op everywhere), but off-Windows they degrade the same honest
        // way the rest of this stub does.
        let scratch = std::path::Path::new("ignored");
        assert_eq!(
            svc.reveal_in_file_manager(scratch),
            Err(BackendError::Unsupported)
        );
        assert_eq!(svc.open_path(scratch), Err(BackendError::Unsupported));
        assert_eq!(svc.move_to_trash(scratch), Err(BackendError::Unsupported));
        assert_eq!(svc.beep(), Err(BackendError::Unsupported));
        // `displays`/`cursor_screen_point` are likewise fully
        // implemented on real Windows but degrade the same honest way
        // off it.
        assert_eq!(svc.displays(), Err(BackendError::Unsupported));
        assert_eq!(svc.cursor_screen_point(), Err(BackendError::Unsupported));
    }

    /// `open_url_result`/`open_path` report a real failure when
    /// `ShellExecuteW` itself fails. The target is an absolute path
    /// under a non-existent directory with an unregistered extension —
    /// `ShellExecuteW` answers `SE_ERR_FNF` immediately. Runs only on
    /// `windows-latest` CI — `cargo test --features win` on `ubuntu-latest`
    /// never compiles this arm.
    #[cfg(target_os = "windows")]
    #[test]
    fn open_url_result_and_open_path_report_a_real_shell_execute_failure() {
        let unopenable = format!(
            "C:\\quadraui-1087-no-such-directory-{}\\x.quadraui1087nohandler",
            std::process::id()
        );
        let svc = WinPlatformServices::new();
        assert!(svc.open_url_result(&unopenable).is_err());
        assert!(svc.open_path(Path::new(&unopenable)).is_err());
    }

    /// `assign_button_ids` is pure id-assignment logic, host-independent
    /// in principle, but gated on `target_os = "windows"` anyway since
    /// it's only compiled in under that cfg (see the `#[cfg(target_os =
    /// "windows")]` on its own definition) — these run on the
    /// `windows-latest` CI leg alongside the rest of this module's
    /// WinAPI-backed tests.
    #[cfg(target_os = "windows")]
    fn win_button(id: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
        MessageDialogButton {
            id: crate::types::WidgetId::new(id),
            label: id.to_string(),
            is_default,
            is_cancel,
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn assign_button_ids_starts_at_100_with_no_cancel_button() {
        let buttons = [
            win_button("ok", true, false),
            win_button("retry", false, false),
        ];
        assert_eq!(assign_button_ids(&buttons), vec![100, 101]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn assign_button_ids_gives_the_first_cancel_button_idcancel() {
        let buttons = [
            win_button("save", false, false),
            win_button("dont_save", false, false),
            win_button("cancel", false, true),
        ];
        assert_eq!(
            assign_button_ids(&buttons),
            vec![100, 101, TASKDIALOG_IDCANCEL]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn assign_button_ids_only_the_first_cancel_button_gets_idcancel() {
        // A second `is_cancel` button is unusual input, but must still
        // resolve to a distinct id rather than colliding with the first.
        let buttons = [win_button("a", false, true), win_button("b", false, true)];
        assert_eq!(assign_button_ids(&buttons), vec![TASKDIALOG_IDCANCEL, 101]);
    }

    // ── xml_escape / build_toast_xml ─────────────────────────────────────

    #[test]
    fn xml_escape_escapes_every_reserved_character() {
        assert_eq!(
            xml_escape("<a>&\"'b\""),
            "&lt;a&gt;&amp;&quot;&apos;b&quot;"
        );
    }

    #[test]
    fn xml_escape_leaves_plain_text_untouched() {
        assert_eq!(xml_escape("plain text 123"), "plain text 123");
    }

    #[test]
    fn xml_escape_drops_c0_control_characters_but_keeps_tab_and_newlines() {
        assert_eq!(xml_escape("a\u{0}b\u{7}\tc\nd\re"), "ab\tc\nd\re");
    }

    #[test]
    fn build_toast_xml_has_no_actions_block_without_actions() {
        let n = Notification::new("Title", "Body");
        let xml = build_toast_xml(&n);
        assert!(xml.contains("<text>Title</text>"));
        assert!(xml.contains("<text>Body</text>"));
        assert!(!xml.contains("<actions>"));
    }

    #[test]
    fn build_toast_xml_has_no_audio_element_by_default() {
        let n = Notification::new("Title", "Body");
        assert!(!build_toast_xml(&n).contains("<audio"));
    }

    #[test]
    fn build_toast_xml_adds_silent_audio_element_when_requested() {
        let n = Notification::new("Title", "Body").with_silent(true);
        assert!(build_toast_xml(&n).contains("<audio silent=\"true\"/>"));
    }

    #[test]
    fn build_toast_xml_adds_one_action_per_registered_action() {
        let n = Notification::new("Title", "Body")
            .with_action(crate::types::WidgetId::new("snooze"), "Snooze")
            .with_action(crate::types::WidgetId::new("dismiss"), "Dismiss");
        let xml = build_toast_xml(&n);
        assert!(xml.contains("<actions>"));
        assert!(xml.contains("content=\"Snooze\" arguments=\"snooze\""));
        assert!(xml.contains("content=\"Dismiss\" arguments=\"dismiss\""));
    }

    #[test]
    fn build_toast_xml_escapes_title_body_and_action_label() {
        let n = Notification::new("<Title>", "Body & more")
            .with_action(crate::types::WidgetId::new("a&b"), "Snooze <5m>");
        let xml = build_toast_xml(&n);
        assert!(xml.contains("<text>&lt;Title&gt;</text>"));
        assert!(xml.contains("<text>Body &amp; more</text>"));
        assert!(xml.contains("content=\"Snooze &lt;5m&gt;\""));
        assert!(xml.contains("arguments=\"a&amp;b\""));
    }
}
