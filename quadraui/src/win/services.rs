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
//! - **Notifications** — a transient `Shell_NotifyIconW` balloon tip:
//!   add a tray icon with `NIF_INFO` set, then remove it a few seconds
//!   later from a spawned thread (a balloon has no lifetime of its own
//!   independent of the icon it's attached to, and this backend has no
//!   persistent tray icon to hang it off).
//! - **Message dialogs** (#744) — `TaskDialogIndirect`, the modern
//!   common-controls v6 alert (preferred over the legacy `MessageBoxW`
//!   for its richer, arbitrarily-labelled button row — `MessageDialogOptions`
//!   carries caller-declared buttons, not a fixed OK/Cancel/Yes/No set).
//!   Blocking and synchronous, same as the file dialogs above, so it
//!   needs no [`crate::desktop::ModalPumpGuard`] of its own either — see
//!   this module's `#702` audit note below, which applies identically
//!   here.
//! - **`open_url`** — `ShellExecuteW(NULL, "open", url, ...)`.
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

use std::path::PathBuf;

#[cfg(target_os = "windows")]
use crate::backend::MessageDialogButton;
use crate::backend::{
    Clipboard, FileDialogOptions, MessageDialogChoice, MessageDialogOptions, Notification,
    PlatformServices,
};
#[cfg(target_os = "windows")]
use crate::primitives::dialog::DialogSeverity;

#[cfg(target_os = "windows")]
use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_os = "windows")]
use windows::core::{IUnknown, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
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
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Ole::CF_UNICODETEXT;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON,
    TDF_ALLOW_DIALOG_CANCELLATION, TD_ERROR_ICON, TD_INFORMATION_ICON, TD_WARNING_ICON,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
    SHCreateItemFromParsingName, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIIF_ERROR, NIIF_INFO,
    NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, SIGDN_FILESYSPATH,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    LoadIconW, HICON, IDI_ERROR, IDI_INFORMATION, SW_SHOWNORMAL,
};

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
            win_clipboard_write(text);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
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
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn copy_wide_truncated(dst: &mut [u16], text: &str) {
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

#[cfg(target_os = "windows")]
fn win_clipboard_write(text: &str) {
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let wide = wide_nul_terminated(text);
        let bytes = wide.len() * std::mem::size_of::<u16>();
        if let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let ptr = GlobalLock(hglobal) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(hglobal));
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                let _ = GlobalUnlock(hglobal);
                // On success the system now owns `hglobal` — it must NOT
                // be `GlobalFree`d here. On failure nothing took
                // ownership, so this is the only chance to release it.
                if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hglobal.0))).is_err() {
                    let _ = GlobalFree(Some(hglobal));
                }
            }
        }
        let _ = CloseClipboard();
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

        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: uid,
            uFlags: NIF_ICON | NIF_INFO,
            hIcon: icon,
            dwInfoFlags: if n.urgent { NIIF_ERROR } else { NIIF_INFO },
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

// ─── open_url (#23) ─────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn win_open_url(url: &str) {
    let operation = wide_nul_terminated("open");
    let file = wide_nul_terminated(url);
    unsafe {
        let _ = windows::Win32::UI::Shell::ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
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
        assert!(svc
            .show_file_open_dialog(FileDialogOptions::default())
            .is_none());
        assert!(svc
            .show_file_save_dialog(FileDialogOptions::default())
            .is_none());
        svc.send_notification(Notification {
            title: "t".to_string(),
            body: "b".to_string(),
            urgent: false,
        });
        assert!(svc
            .show_message_dialog(MessageDialogOptions {
                title: "t".to_string(),
                body: "b".to_string(),
                buttons: Vec::new(),
                severity: None,
            })
            .is_none());
        svc.open_url("https://example.com");
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
