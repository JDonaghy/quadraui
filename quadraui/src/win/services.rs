//! Win-GUI implementation of [`quadraui::PlatformServices`] (#23).
//!
//! - **Clipboard** — Win32 `OpenClipboard`/`GetClipboardData`/
//!   `SetClipboardData` against `CF_UNICODETEXT`, with the payload backed
//!   by a `GlobalAlloc(GMEM_MOVEABLE)` block per the classic Win32
//!   clipboard contract (the system takes ownership of the block once
//!   `SetClipboardData` succeeds — it must never be `GlobalFree`d by the
//!   caller after that point).
//! - **File dialogs** — the Vista+ common file dialog: `IFileOpenDialog`
//!   / `IFileSaveDialog` via COM (`CoCreateInstance`), parented to the
//!   live `HWND` [`WinBackend::attach_surface`][super::backend::WinBackend::attach_surface]
//!   hands to [`WinPlatformServices::set_window`] — same role as
//!   `GtkPlatformServices::window`.
//! - **Folder dialog** (quadraui#935) — the same `IFileOpenDialog`, with
//!   `FOS_PICKFOLDERS` set on its options: Windows has no separate
//!   directory-picker COM object, so this is the documented way to turn
//!   the file-open dialog into one.
//! - **Notifications** — a transient `Shell_NotifyIconW` balloon tip:
//!   add a tray icon with `NIF_INFO` set, then remove it a few seconds
//!   later from a spawned thread (a balloon has no lifetime of its own
//!   independent of the icon it's attached to, and this backend has no
//!   persistent tray icon to hang it off). Issue #955's `silent` field
//!   maps onto `NIIF_NOSOUND`; `icon`/`actions`/`tag` don't fit a
//!   balloon's shape and are dropped — see `win_send_notification`'s own
//!   doc. A real fix is WinRT `ToastNotificationManager` (needs an
//!   AUMID/shortcut), kept as a documented follow-up rather than
//!   implemented blind — this module has no Windows host to build/verify
//!   the WinRT toast XML + activation-token plumbing against beyond
//!   `cargo check`'s type-check, and a toast's click-through is exactly
//!   the kind of behavioural correctness `cargo check` can't see (see
//!   `CLAUDE.md`'s "Win-GUI: building and testing for real" section).
//!   The balloon stays the fallback either way, per this issue's own
//!   note.
//! - **Message dialogs** (#744) — `TaskDialogIndirect`, the modern
//!   common-controls v6 alert (preferred over the legacy `MessageBoxW`
//!   for its richer, arbitrarily-labelled button row — `MessageDialogOptions`
//!   carries caller-declared buttons, not a fixed OK/Cancel/Yes/No set).
//!   Blocking and synchronous, same as the file dialogs above, so it
//!   needs no [`crate::desktop::ModalPumpGuard`] of its own either — see
//!   this module's `#702` audit note below, which applies identically
//!   here.
//! - **`open_url`** — `ShellExecuteW(NULL, "open", url, ...)`.
//! - **`shell.*` parity (#956)** — `reveal_in_file_manager`
//!   (`SHOpenFolderAndSelectItems`, see `win_reveal_in_file_manager`'s own
//!   doc for the PIDL dance it takes), `open_path` (the same
//!   `ShellExecuteW` call `open_url` makes, factored into
//!   `win_shell_execute_open` and shared by both), `move_to_trash`
//!   (delegates to [`crate::desktop::move_to_trash`] — the cross-platform
//!   `trash` crate, not a hand-rolled `SHFileOperationW`, see that
//!   function's doc for why), and `beep` (`MessageBeep(MB_OK)`).
//! - **Displays (#959)** — `displays` uses `EnumDisplayMonitors` +
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
    Notification, PlatformServices, ServiceResult, SystemTheme,
};
use crate::event::{Point, Rect};
#[cfg(target_os = "windows")]
use crate::primitives::dialog::DialogSeverity;
use crate::types::Color;

#[cfg(target_os = "windows")]
use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_os = "windows")]
use windows::core::{IUnknown, BOOL, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IBindCtx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
#[cfg(target_os = "windows")]
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
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
    SHCreateItemFromParsingName, SHOpenFolderAndSelectItems, ShellExecuteW, Shell_NotifyIconW,
    FOS_PICKFOLDERS, HDROP, NIF_ICON, NIF_INFO, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND, NIM_ADD,
    NIM_DELETE, NOTIFYICONDATAW, SIGDN_FILESYSPATH,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, LoadIconW, HICON, IDI_ERROR, IDI_INFORMATION, MB_OK, MONITORINFOF_PRIMARY,
    SW_SHOWNORMAL, USER_DEFAULT_SCREEN_DPI,
};
// WinRT (not Win32) — `system_theme` (quadraui#952). `UISettings` is the
// same class the issue names (`UISettings::GetColorValue`); `AccessibilitySettings`
// is its sibling in the same `Windows.UI.ViewManagement` namespace and the
// natural WinRT source for "is high contrast active" — no separate Win32
// `SystemParametersInfoW(SPI_GETHIGHCONTRAST)` call (and no extra
// `Win32_UI_Accessibility` Cargo feature) needed.
#[cfg(target_os = "windows")]
use windows::UI::ViewManagement::{AccessibilitySettings, UIColorType, UISettings};

/// System clipboard via raw Win32 calls (issue #23). Stateless — every
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
    /// surfaced instead of silently discarded (issue #805) — the one
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

    /// `CF_HDROP` file-path list — a Finder/Explorer copy (issue #954).
    /// `read_image`/`write_image`/`write_html` stay at the trait's
    /// `Unsupported` default here: unlike `CF_HDROP` (one well-known
    /// fixed-layout struct, `DROPFILES` + a `\0`-separated `\0\0`-terminated
    /// path list, decoded below with no format ambiguity), `CF_DIB`
    /// (bottom-up, row-padded, BGR-order pixels, no alpha in the classic
    /// 24bpp case) and `CF_HTML` (a registered format —
    /// `RegisterClipboardFormatW(L"HTML Format")` — wrapping the payload
    /// in a byte-offset text header, not a `CF_*` constant at all) are
    /// real reverse-engineering-adjacent native formats this crate has no
    /// way to exercise against a live clipboard from this repo's Linux
    /// CI (`cargo check`/`cargo test --features win` type-check the
    /// `cfg(target_os = "windows")` arms but never execute them — see
    /// `CLAUDE.md`'s Win-GUI section) or, in this dispatch, a live
    /// Windows host either. Landing untested byte-level clipboard parsing
    /// that only ever *compiles* is worse than an honest `Unsupported`;
    /// left for a follow-up dispatched to Win-GUI's real-hardware lane.
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

    /// `EmptyClipboard` — clears every format, not just text (issue #954).
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
}

pub struct WinPlatformServices {
    clipboard: WinClipboard,
    /// Top-level window used to parent file dialogs (`IModalWindow::Show`)
    /// and host the notification tray icon's owning `HWND`. `None` until
    /// [`WinBackend::attach_surface`][super::backend::WinBackend::attach_surface]
    /// calls [`Self::set_window`] with a live window — mirrors
    /// `GtkPlatformServices::window`'s "unparented until the first
    /// attach" lifecycle. `Cell`, not `RefCell<Option<_>>` like GTK's,
    /// since `HWND` is `Copy` and there's nothing here that needs to hold
    /// a borrow across a call.
    #[cfg(target_os = "windows")]
    window: Cell<Option<HWND>>,
}

impl WinPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: WinClipboard,
            #[cfg(target_os = "windows")]
            window: Cell::new(None),
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

    /// The shell folder-picker (`IFileOpenDialog` + `FOS_PICKFOLDERS`,
    /// quadraui#935) — see `win_show_folder_open_dialog` below.
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

    /// `TaskDialogIndirect` (#744) — see this module's doc comment for
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

    fn send_notification(&self, n: Notification) {
        #[cfg(target_os = "windows")]
        {
            win_send_notification(self.window.get(), &n);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = n;
        }
    }

    fn open_url(&self, url: &str) {
        #[cfg(target_os = "windows")]
        {
            win_open_url(url);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = url;
        }
    }

    /// `SHOpenFolderAndSelectItems` (issue #956) — see
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

    /// `ShellExecuteW(NULL, "open", path, ...)` (issue #956) — the exact
    /// same call [`Self::open_url`] makes above, just fed a filesystem
    /// path instead of a URL string; `ShellExecuteW`'s `"open"` verb
    /// already accepts either. See [`win_shell_execute_open`]'s doc for
    /// why this method, unlike `open_url`, surfaces the call's real
    /// success/failure instead of discarding it.
    fn open_path(&self, path: &Path) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            win_shell_execute_open(&path.to_string_lossy())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = path;
            Err(BackendError::Unsupported)
        }
    }

    /// [`crate::desktop::move_to_trash`] (issue #956) — see that
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

    /// `MessageBeep(MB_OK)` (issue #956) — the default system
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
    /// `AccessibilitySettings::HighContrast` (quadraui#952) — see this
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
    /// `GetDpiForMonitor` for `scale` (issue #959).
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

    /// `GetCursorPos` (issue #959) — the global cursor position in
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
/// payload, `NOTIFYICONDATAW`'s fixed-size fields).
///
/// Only called from `cfg(target_os = "windows")` code (plus this module's
/// own `#[cfg(test)]` block, which exercises it on every host) — `allow`
/// rather than `cfg`-gating the definition itself, same as `win::msg`'s
/// helpers, so a plain `cargo check --features win` on Linux still
/// type-checks the body instead of skipping it outright.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn wide_nul_terminated(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}

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

// ─── Clipboard (#23) ────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_clipboard_read() -> Option<String> {
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
/// the real failure instead of swallowing it (issue #805). Every early
/// return below leaves the clipboard exactly as `write_text`/
/// `write_text_result`'s callers have always observed it — this only
/// adds the `Err` a caller can now ask for via `write_text_result`;
/// `write_text` itself still discards it.
#[cfg(target_os = "windows")]
fn win_clipboard_write(text: &str) -> ServiceResult<()> {
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

/// Read the `CF_HDROP` file-path list off the clipboard (issue #954) — a
/// Finder/Explorer copy, on Windows a `DROPFILES` struct whose payload is
/// enumerated with `DragQueryFileW` rather than parsed by hand (the same
/// API a drag-and-drop `WM_DROPFILES` handler uses for the live-drag
/// case).
#[cfg(target_os = "windows")]
fn win_clipboard_read_file_list() -> ServiceResult<Vec<PathBuf>> {
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
/// of what's on it (issue #954). Unlike `win_clipboard_write`, there is
/// no payload to allocate/own, so this is the shortest possible
/// open/mutate/close cycle.
#[cfg(target_os = "windows")]
fn win_clipboard_clear() -> ServiceResult<()> {
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

// ─── File dialogs (#23) ─────────────────────────────────────────────────

/// `CoInitializeEx(COINIT_APARTMENTTHREADED)`, once per thread. File
/// dialogs are always shown synchronously from whatever thread calls
/// `show_file_open_dialog`/`show_file_save_dialog` — in practice the
/// Win32 message-loop thread `win::run` owns. Both `S_OK` (first call on
/// this thread) and `S_FALSE` (already initialized on this thread) are
/// "COM is ready"; a genuine failure has no good recovery here short of
/// not showing the dialog, which the subsequent `CoCreateInstance` call
/// surfaces on its own. Never paired with `CoUninitialize` — COM stays
/// initialized for the rest of the thread's life, same posture as the
/// GTK backend never tearing down its GLib main context.
///
/// `pub(crate)` (not private) since #739: `super::image`'s WIC decode
/// path also needs COM ready (`CoCreateInstance(CLSID_WICImagingFactory,
/// ..)`) before it can build a factory, same requirement as the file
/// dialogs below — one thread-local init guard for both call sites
/// rather than two copies of the same `Cell<bool>` dance.
#[cfg(target_os = "windows")]
pub(crate) fn ensure_com_initialized() {
    thread_local! {
        static COM_INITIALIZED: Cell<bool> = const { Cell::new(false) };
    }
    COM_INITIALIZED.with(|done| {
        if !done.get() {
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
                    // exists) shouldn't abort showing the dialog at all —
                    // degrade to the dialog's own default folder instead.
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
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        configure_file_dialog(&dialog, opts, None).ok()?;
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}

/// The shell folder-picker (quadraui#935): the same `IFileOpenDialog` as
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
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        configure_file_dialog(&dialog, opts, opts.initial_filename.as_deref()).ok()?;
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}

// ─── Message dialog (#744) ──────────────────────────────────────────────

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

/// `TaskDialogIndirect` (issue #744) — the common-controls v6 alert,
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
    unsafe {
        TaskDialogIndirect(&config, Some(&mut chosen), None, None).ok()?;
    }
    ids.iter()
        .position(|&id| id == chosen)
        .map(|i| opts.buttons[i].id.clone())
}

// ─── Notifications (#23) ────────────────────────────────────────────────

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

/// Add a transient tray icon carrying `n`'s title/body as an `NIF_INFO`
/// balloon, then remove it again after [`NOTIFICATION_ICON_LIFETIME`]
/// (see that constant's docs). A no-op if no window has attached yet
/// (`owner` is `None`) — there's no `HWND` to own the tray icon.
#[cfg(target_os = "windows")]
fn win_send_notification(owner: Option<HWND>, n: &Notification) {
    let Some(hwnd) = owner else {
        return;
    };
    unsafe {
        let icon_resource = if n.urgent { IDI_ERROR } else { IDI_INFORMATION };
        let icon = LoadIconW(None, icon_resource).unwrap_or(HICON(std::ptr::null_mut()));
        let uid = NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);

        // Issue #955 extended `Notification` with `icon`/`actions`/
        // `silent`/`tag`. Only `silent` has anywhere to go on this
        // balloon-tip path — `NIIF_NOSOUND` is a real, documented
        // `dwInfoFlags` bit (combined with the severity flag below, not
        // a replacement for it). `icon`, `actions`, and `tag` are
        // dropped: a balloon has no caller-supplied icon slot beyond the
        // severity icon already chosen above, no button/action UI at
        // all, and (like macOS's `osascript` fallback) no click-through
        // channel to report an activation back through — see this
        // module's doc and `UiEvent::NotificationActivated`'s own doc
        // for what a real fix (WinRT `ToastNotificationManager`, which
        // has all three) would need.
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

// ─── open_url (#23) / open_path (#956) ───────────────────────────────────

/// `ShellExecuteW(NULL, "open", target, ...)` — shared by [`win_open_url`]
/// (a URL string) and [`PlatformServices::open_path`]'s Windows arm (a
/// filesystem path): the `"open"` verb accepts either, so there is
/// nothing target-kind-specific left to branch on. Unlike `win_open_url`
/// (which predates issue #956's fallible `open_path` and keeps its
/// original fire-and-forget `void` shape for that reason — `open_url`'s
/// own signature is infallible, so its call site simply discards this
/// function's `Result`), this reports `ShellExecuteW`'s real outcome:
/// per its own docs, success is any return value greater than 32; the
/// low range 0..=32 is a documented failure code (e.g. `SE_ERR_FNF` = 2,
/// `SE_ERR_NOASSOC` = 31).
#[cfg(target_os = "windows")]
fn win_shell_execute_open(target: &str) -> ServiceResult<()> {
    let operation = wide_nul_terminated("open");
    let file = wide_nul_terminated(target);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result.0 as isize) > 32 {
        Ok(())
    } else {
        Err(BackendError::PlatformFailure {
            context: format!("ShellExecuteW returned {}", result.0 as isize),
        })
    }
}

#[cfg(target_os = "windows")]
fn win_open_url(url: &str) {
    let _ = win_shell_execute_open(url);
}

// ─── reveal_in_file_manager (issue #956) ─────────────────────────────────

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

// ─── beep (issue #956) ────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_beep() -> ServiceResult<()> {
    unsafe { MessageBeep(MB_OK) }.map_err(|e| BackendError::PlatformFailure {
        context: format!("MessageBeep: {e}"),
    })
}

// ─── System theme (quadraui#952) ────────────────────────────────────────

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
    Ok(system_theme_from_ui_colors(
        (bg.R, bg.G, bg.B),
        (accent.R, accent.G, accent.B),
        high_contrast,
    ))
}

/// Pure mapping from `UISettings`' background/accent RGB triples plus the
/// high-contrast flag to [`SystemTheme`] — split out so it's unit-testable
/// without the real WinRT `UISettings`/`AccessibilitySettings` classes,
/// which this crate can't construct on a non-Windows host (same
/// cross-platform-testable-helper posture as [`wide_nul_terminated`]
/// above; see that function's doc for why this is `allow`-gated rather
/// than `cfg`-gated).
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
) -> SystemTheme {
    let (r, g, b) = bg;
    let luma = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    SystemTheme {
        dark: luma < 128,
        accent: Some(Color::rgb(accent.0, accent.1, accent.2)),
        high_contrast,
    }
}

// ─── Displays (issue #959) ──────────────────────────────────────────────

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
    // Nonzero — continue enumeration. `windows_core::BOOL` is a bare
    // `i32` alias in this crate version, not `windows::Win32::Foundation::BOOL`'s
    // wrapper-struct shape other Win32 crates use.
    1
}

/// `EnumDisplayMonitors` + `GetMonitorInfoW` for
/// `bounds`/`work_area`/`primary`, `GetDpiForMonitor` for `scale`
/// (issue #959) — see [`PlatformServices::displays`]'s doc for the full
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
        if ok == 0 {
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

/// `GetCursorPos` (issue #959) — the global cursor position in
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

    // ── system_theme_from_ui_colors (quadraui#952) ──────────────────────

    #[test]
    fn system_theme_from_ui_colors_black_background_is_dark() {
        let theme = system_theme_from_ui_colors((0, 0, 0), (0, 120, 215), false);
        assert!(theme.dark);
        assert_eq!(theme.accent, Some(Color::rgb(0, 120, 215)));
        assert!(!theme.high_contrast);
    }

    #[test]
    fn system_theme_from_ui_colors_white_background_is_light() {
        let theme = system_theme_from_ui_colors((255, 255, 255), (0, 120, 215), false);
        assert!(!theme.dark);
    }

    #[test]
    fn system_theme_from_ui_colors_reports_high_contrast() {
        assert!(system_theme_from_ui_colors((0, 0, 0), (255, 255, 0), true).high_contrast);
    }

    // ── display_from_monitor_rects (issue #959) ──────────────────────────

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
        // matching the virtual-desktop coordinate space every monitor
        // shares (quadraui#959, `Display`'s own doc).
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
        // #954: `read_image`/`write_image`/`write_html` are the trait's
        // own `Unsupported` default (`WinClipboard` never overrides
        // them — see `read_file_list`'s doc for why); `read_file_list`
        // and `clear` are overridden but degrade the same way off
        // Windows.
        assert_eq!(svc.clipboard().read_image(), Err(BackendError::Unsupported));
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
        assert_eq!(svc.system_theme(), Err(BackendError::Unsupported));
        // #956: every one of these four *is* fully implemented on real
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
        // #959: `displays`/`cursor_screen_point` are likewise fully
        // implemented on real Windows but degrade the same honest way
        // off it.
        assert_eq!(svc.displays(), Err(BackendError::Unsupported));
        assert_eq!(svc.cursor_screen_point(), Err(BackendError::Unsupported));
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
}
