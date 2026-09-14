//! Tray / status-bar icon control for Win-GUI (issue #953).
//!
//! `Shell_NotifyIconW` backs `Backend::tray`'s
//! [`crate::backend::TrayService`] surface — see
//! `impl TrayService for WinBackend` in `super::backend`, which
//! delegates every method to the free functions here (same "thin impl
//! on the struct, real logic in a sibling module" shape
//! `super::services`/`super::image` already use).
//!
//! ## Icon
//!
//! [`set_icon`] decodes `ImageSource` through the same WIC pipeline
//! `super::image::draw_image` uses (`IWICImagingFactory` →
//! `IWICBitmapDecoder` → `IWICFormatConverter` into 32bpp premultiplied
//! BGRA), then builds a `HICON` from the raw pixels via
//! `CreateDIBSection` (the exact 32bpp top-down DIB shape
//! `super::testing::HeadlessSurface` already uses for its render
//! target) + an all-zero AND-mask `HBITMAP` + `CreateIconIndirect`. The
//! all-zero mask means "never masked away" — correct because the DIB's
//! own alpha channel (WIC's premultiplied-alpha conversion) is what
//! actually carries transparency; Windows has alpha-blended 32bpp icons
//! since XP.
//!
//! ## Menu vs. plain click
//!
//! Unlike macOS's `NSStatusItem.menu` (see `macos::tray`'s module doc),
//! Win32 has no "attach a menu, the OS shows it automatically" API —
//! [`Shell_NotifyIconW`]'s callback message fires for every click
//! regardless of whether [`set_menu`] has been called, and it is this
//! module's own `win::run` wndproc arm that decides what to do with it:
//! if a menu is attached, show it via [`track_menu`] and stop there
//! (mirroring `NSStatusItem`'s real behaviour without needing AppKit's
//! help to get it); otherwise dispatch `UiEvent::TrayClicked`. Because
//! this backend calls `TrackPopupMenuEx` itself (unlike macOS's
//! `NSStatusItem.menu`, which hands the whole click-track-close cycle to
//! AppKit), it can push `UiEvent::ContextMenuDismissed` the instant the
//! call returns — the same [`super::menu_bar_install`]-shaped
//! `Backend::show_context_menu`... this backend has no such reference
//! implementation, so [`track_menu`]'s doc spells out the sequence
//! directly.

use crate::event::{MouseButton, UiEvent};

#[cfg(target_os = "windows")]
use crate::backend::{BackendError, ServiceResult};
#[cfg(target_os = "windows")]
use crate::primitives::context_menu::{ContextMenu, ContextMenuItem};
#[cfg(target_os = "windows")]
use crate::primitives::image::ImageSource;
#[cfg(target_os = "windows")]
use crate::types::WidgetId;

#[cfg(target_os = "windows")]
use windows::core::{IUnknown, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, POINT, TRUE};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::{
    SHCreateMemStream, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_MODIFY,
    NOTIFYICONDATAW,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    SetForegroundWindow, TrackPopupMenuEx, HICON, HMENU, ICONINFO, MF_CHECKED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
};

#[cfg(target_os = "windows")]
use super::backend::WM_QUADRAUI_TRAY_CALLBACK;
#[cfg(target_os = "windows")]
use super::services::{copy_wide_truncated, ensure_com_initialized};

/// This backend owns exactly one tray icon, so its `NOTIFYICONDATAW::uID`
/// never needs to vary — unlike `super::services`'s transient balloon
/// notifications (`NEXT_NOTIFICATION_ID`), which mint a fresh id per
/// call so back-to-back notifications don't collide on `NIM_ADD`. `0` is
/// reserved for this permanent icon specifically because
/// `NEXT_NOTIFICATION_ID` starts at `1` and only ever increments, so the
/// two ids can never collide on the same `hWnd`.
#[cfg(target_os = "windows")]
const TRAY_ICON_UID: u32 = 0;

/// Per-[`super::backend::WinBackend`] tray state (issue #953).
#[cfg(target_os = "windows")]
#[derive(Default)]
pub(crate) struct WinTrayState {
    /// Whether `NIM_ADD` has succeeded — `Shell_NotifyIconW` rejects
    /// `NIM_MODIFY` for an icon that was never added, so every method
    /// below must add-or-modify depending on this flag (mirrors
    /// `super::services::win_send_notification`'s single `NIM_ADD`, but
    /// this icon is long-lived rather than removed after a timeout).
    added: bool,
    /// The current icon, kept alive for as long as it's shown —
    /// `Shell_NotifyIconW` does not take ownership of `hIcon` (MSDN:
    /// the caller must keep it valid until changed or the icon is
    /// removed) and `CreateIconIndirect`-built icons are not
    /// system-shared, so this backend must `DestroyIcon` the previous
    /// one itself when replacing it.
    icon: Option<HICON>,
    /// The attached menu, if any — see this module's doc on why the
    /// menu-vs-plain-click decision happens in `win::run`'s wndproc, not
    /// here. `None` (the default) means a plain click dispatches
    /// `UiEvent::TrayClicked`; `Some` (even with an empty
    /// [`ContextMenu::items`] — `TrayService::set_menu`'s own doc calls
    /// this out) means [`track_menu`] runs instead.
    menu: Option<ContextMenu>,
}

/// [`crate::backend::TrayService::set_icon`]'s Win-GUI implementation.
#[cfg(target_os = "windows")]
pub(crate) fn set_icon(
    state: &mut WinTrayState,
    hwnd: HWND,
    icon: ImageSource,
) -> ServiceResult<()> {
    let hicon = decode_hicon(&icon).ok_or(BackendError::PlatformFailure {
        context: "WIC/GDI failed to decode the tray icon source into a HICON".to_string(),
    })?;

    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_UID,
        uFlags: NIF_ICON | NIF_MESSAGE,
        uCallbackMessage: WM_QUADRAUI_TRAY_CALLBACK,
        hIcon: hicon,
        ..Default::default()
    };
    let message = if state.added { NIM_MODIFY } else { NIM_ADD };
    let ok = unsafe { Shell_NotifyIconW(message, &data) }.as_bool();
    if !ok {
        unsafe {
            let _ = DestroyIcon(hicon);
        }
        return Err(BackendError::PlatformFailure {
            context: "Shell_NotifyIconW(NIM_ADD/NIM_MODIFY, NIF_ICON) failed".to_string(),
        });
    }
    state.added = true;
    // The old icon (if any) has now been replaced in the shell's own
    // notification-area state, and `CreateIconIndirect`'s own copy
    // semantics mean nothing upstream still references it — safe to
    // destroy immediately, unlike `hicon` above (which must survive the
    // `Shell_NotifyIconW` call it's passed to).
    if let Some(old) = state.icon.replace(hicon) {
        unsafe {
            let _ = DestroyIcon(old);
        }
    }
    Ok(())
}

/// [`crate::backend::TrayService::set_tooltip`]'s Win-GUI implementation.
#[cfg(target_os = "windows")]
pub(crate) fn set_tooltip(
    state: &mut WinTrayState,
    hwnd: HWND,
    tooltip: &str,
) -> ServiceResult<()> {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_UID,
        uFlags: NIF_TIP | NIF_MESSAGE,
        uCallbackMessage: WM_QUADRAUI_TRAY_CALLBACK,
        ..Default::default()
    };
    copy_wide_truncated(&mut data.szTip, tooltip);
    let message = if state.added { NIM_MODIFY } else { NIM_ADD };
    let ok = unsafe { Shell_NotifyIconW(message, &data) }.as_bool();
    if !ok {
        return Err(BackendError::PlatformFailure {
            context: "Shell_NotifyIconW(NIM_ADD/NIM_MODIFY, NIF_TIP) failed".to_string(),
        });
    }
    state.added = true;
    Ok(())
}

/// [`crate::backend::TrayService::set_menu`]'s Win-GUI implementation —
/// just records `menu` for `win::run`'s wndproc to read back via
/// [`super::backend::WinBackend::tray_menu`] the next time a click
/// arrives. See this module's doc for why the actual `HMENU`/
/// `TrackPopupMenuEx` work happens at click time ([`track_menu`]) rather
/// than here: there is no persistent native menu handle to build and
/// keep alive between calls, only a `ContextMenu` value to remember.
#[cfg(target_os = "windows")]
pub(crate) fn set_menu(
    state: &mut WinTrayState,
    _hwnd: HWND,
    menu: &ContextMenu,
) -> ServiceResult<()> {
    state.menu = if menu.items.is_empty() {
        None
    } else {
        Some(menu.clone())
    };
    Ok(())
}

/// The currently-attached tray menu, if any — read by `win::run`'s
/// `WM_QUADRAUI_TRAY_CALLBACK` wndproc arm through
/// [`super::backend::WinBackend::tray_menu`] to decide whether a click
/// should show a menu ([`track_menu`]) or dispatch
/// `UiEvent::TrayClicked`.
#[cfg(target_os = "windows")]
pub(crate) fn attached_menu(state: &WinTrayState) -> Option<ContextMenu> {
    state.menu.clone()
}

/// Build a native popup `HMENU` from `menu`, show it at the current
/// cursor position via `TrackPopupMenuEx` (`TPM_RETURNCMD` — the call
/// blocks and returns the selected item's assigned command id directly,
/// `0` if the user dismissed it with no selection, rather than needing a
/// separate `WM_COMMAND` round-trip), then tear the `HMENU` down.
/// Returns the activated item's `WidgetId`, or `None` on dismissal —
/// callers (`win::run`'s wndproc) push `UiEvent::ContextMenuItemActivated`
/// for `Some` and always follow up with `UiEvent::ContextMenuDismissed`
/// once this returns, mirroring the pair
/// [`super`]... this backend's own analogue of
/// `macos::menu_bar_install::show_context_menu`'s activation/dismissal
/// contract.
///
/// `SetForegroundWindow` first is the standard, MSDN-documented tray
/// fix: without it, `TrackPopupMenuEx` can fail to dismiss when the user
/// clicks away, leaving a stuck menu.
#[cfg(target_os = "windows")]
pub(crate) fn track_menu(hwnd: HWND, menu: &ContextMenu) -> Option<WidgetId> {
    let mut ids: Vec<WidgetId> = Vec::new();
    let hmenu = unsafe { CreatePopupMenu() }.ok()?;
    for item in &menu.items {
        append_hmenu_item(hmenu, item, &mut ids);
    }

    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    let mut cursor = POINT::default();
    let got_cursor = unsafe { GetCursorPos(&mut cursor) }.is_ok();

    let flags = (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0;
    let cmd = if got_cursor {
        unsafe { TrackPopupMenuEx(hmenu, flags, cursor.x, cursor.y, hwnd, None) }.0
    } else {
        0
    };

    unsafe {
        let _ = DestroyMenu(hmenu);
    }

    if cmd <= 0 {
        return None;
    }
    ids.into_iter().nth(cmd as usize - 1)
}

/// Append one [`ContextMenuItem`] (and any nested submenu) to `hmenu`,
/// assigning each leaf a 1-based command id and pushing its `WidgetId`
/// onto `ids` at the matching index — `ids[cmd - 1]` recovers the
/// activated item after `TrackPopupMenuEx` returns `cmd`. `ids` is
/// threaded through the whole recursive tree (not reset per submenu) so
/// ids stay globally unique across nested submenus, the same role
/// `macos::menu_bar_install::append_menu_item`'s `next_tag` counter
/// plays for `NSMenuItem.tag`.
#[cfg(target_os = "windows")]
fn append_hmenu_item(hmenu: HMENU, item: &ContextMenuItem, ids: &mut Vec<WidgetId>) {
    if item.is_separator() {
        unsafe {
            let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        }
        return;
    }

    let title: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
    let wide = win_wide_nul_terminated(&title);

    if let Some(nested) = item.submenu.as_ref() {
        let submenu = unsafe { CreatePopupMenu() };
        let Ok(submenu) = submenu else { return };
        for child in nested {
            append_hmenu_item(submenu, child, ids);
        }
        unsafe {
            let _ = AppendMenuW(
                hmenu,
                MF_POPUP | MF_STRING,
                submenu.0 as usize,
                PCWSTR::from_raw(wide.as_ptr()),
            );
        }
        return;
    }

    if item.id.is_some() {
        ids.push(item.id.clone().expect("checked Some above"));
        let cmd_id = ids.len();
        let mut flags = MF_STRING;
        if item.disabled {
            flags |= MF_GRAYED;
        }
        if let Some(true) = item.checked {
            flags |= MF_CHECKED;
        }
        unsafe {
            let _ = AppendMenuW(hmenu, flags, cmd_id, PCWSTR::from_raw(wide.as_ptr()));
        }
    }
}

/// Local `str` → NUL-terminated UTF-16 helper — this module's own copy
/// of the same two-line conversion `super::backend`/`super::services`
/// each already carry privately (`win_wide_nul_terminated`/
/// `wide_nul_terminated`); mirrors those files' existing choice to
/// duplicate rather than share a one-liner across module-privacy
/// boundaries.
#[cfg(target_os = "windows")]
fn win_wide_nul_terminated(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}

/// Decode `source`'s encoded bytes into a `HICON`, or `None` on any
/// failure (missing file, corrupt bytes, unrecognised format, GDI
/// allocation failure) — same collapse-to-`None` posture
/// `super::image::decode_bitmap` documents for the in-canvas `Image`
/// primitive's decode failures.
#[cfg(target_os = "windows")]
fn decode_hicon(source: &ImageSource) -> Option<HICON> {
    let bytes: Vec<u8> = match source {
        ImageSource::Path(path) => std::fs::read(path).ok()?,
        ImageSource::Bytes(bytes) => bytes.clone(),
    };

    ensure_com_initialized();
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(
            &CLSID_WICImagingFactory,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .ok()?;
    let stream = unsafe { SHCreateMemStream(Some(&bytes)) }?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
    }
    .ok()?;
    let frame = unsafe { decoder.GetFrame(0) }.ok()?;

    // Same premultiplied-BGRA normalisation `super::image::decode_bitmap`
    // runs before handing frames to Direct2D — `CreateDIBSection`'s
    // 32bpp DIB wants exactly this byte layout too.
    let converter = unsafe { factory.CreateFormatConverter() }.ok()?;
    unsafe {
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
    }
    .ok()?;

    let mut width = 0u32;
    let mut height = 0u32;
    unsafe { converter.GetSize(&mut width, &mut height) }.ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let stride = (width as usize) * 4;
    let mut pixels = vec![0u8; stride * height as usize];
    unsafe { converter.CopyPixels(std::ptr::null(), stride as u32, &mut pixels) }.ok()?;

    build_hicon_from_bgra(&pixels, width, height)
}

/// Build a `HICON` from a top-down, premultiplied-BGRA, 32-bit-per-pixel
/// buffer. Mirrors `super::testing::HeadlessSurface::new`'s
/// `CreateDIBSection` shape exactly (same `BITMAPINFOHEADER`, same
/// negative-`biHeight`-for-top-down convention) so the DIB byte layout
/// here is the one already proven correct by that headless render
/// target's own pixel-readback tests, not a second independent guess at
/// the same arithmetic.
#[cfg(target_os = "windows")]
fn build_hicon_from_bgra(pixels: &[u8], width: u32, height: u32) -> Option<HICON> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let hdc = unsafe { CreateCompatibleDC(None) };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let hbm_color =
        match unsafe { CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) } {
            Ok(bitmap) => bitmap,
            Err(_) => {
                unsafe {
                    let _ = DeleteDC(hdc);
                }
                return None;
            }
        };
    unsafe {
        let _ = DeleteDC(hdc);
    }
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(hbm_color.into());
        }
        return None;
    }
    // SAFETY: `bits` points at a `CreateDIBSection`-allocated buffer of
    // exactly `stride * height` bytes (32bpp, `width` wide, just
    // confirmed non-null above) — the same size `pixels` was decoded
    // into by `decode_hicon`.
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
    }

    // An all-zero AND-mask: every pixel stays unmasked, so the DIB's own
    // (already premultiplied) alpha channel is what determines
    // transparency — see this module's doc.
    let mask_stride = width.div_ceil(16) * 2;
    let mask_bits = vec![0u8; (mask_stride * height) as usize];
    let hbm_mask = unsafe {
        CreateBitmap(
            width as i32,
            height as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        )
    };

    let icon_info = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let hicon = unsafe { CreateIconIndirect(&icon_info) }.ok();

    // MSDN (`CreateIconIndirect`): the function copies both bitmaps into
    // the icon's own internal representation, so the caller is free (and
    // expected) to delete them immediately afterward regardless of
    // whether the call succeeded.
    unsafe {
        let _ = DeleteObject(hbm_color.into());
        let _ = DeleteObject(hbm_mask.into());
    }

    hicon
}

/// Decode `NOTIFYICONDATAW`'s classic (pre-`NOTIFYICON_VERSION_4`)
/// callback contract: `lparam` carries the raw Win32 mouse message
/// (`WM_LBUTTONUP` = `0x0202`, `WM_RBUTTONUP` = `0x0205`,
/// `WM_MBUTTONUP` = `0x0208`) that fired over the icon. `None` for
/// anything else (mouse-down/double-click/move — this backend only
/// reacts to a completed click, matching every other in-tree backend's
/// *-Up-based click semantics). Local numeric constants rather than
/// `windows::Win32::UI::WindowsAndMessaging::WM_*` imports so this
/// function (and its test coverage) compiles and runs on every host,
/// the same portability reasoning `win::run`'s local `MK_LBUTTON`/
/// `MK_RBUTTON`/`MK_MBUTTON` constants already document.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`: the only
/// non-test caller is `win::run`'s wndproc tray arm, which is
/// `cfg(target_os = "windows")`. The CLAUDE.md quality gate runs
/// `cargo check -p quadraui --features win` on Linux/macOS as a
/// type-check of the windows-gated arms, and there this would otherwise
/// be a `-D warnings` dead-code failure — same reasoning as
/// `win::msg`'s module header.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn tray_click_button(win32_message: u32) -> Option<MouseButton> {
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_MBUTTONUP: u32 = 0x0208;
    match win32_message {
        WM_LBUTTONUP => Some(MouseButton::Left),
        WM_RBUTTONUP => Some(MouseButton::Right),
        WM_MBUTTONUP => Some(MouseButton::Middle),
        _ => None,
    }
}

/// Build the `UiEvent` a tray callback should dispatch, given whether a
/// menu is currently attached. Pure glue extracted from `win::run`'s
/// wndproc arm so the click→event decision is unit-testable without a
/// live `HWND`/`TrackPopupMenuEx` — the actual menu tracking
/// ([`track_menu`]) still only runs on real Windows.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]` for the
/// same reason as [`tray_click_button`]'s: its only non-test caller is
/// the windows-gated wndproc arm.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn plain_click_event(button: MouseButton) -> UiEvent {
    UiEvent::TrayClicked { button }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_click_button_maps_the_three_up_messages() {
        assert_eq!(tray_click_button(0x0202), Some(MouseButton::Left));
        assert_eq!(tray_click_button(0x0205), Some(MouseButton::Right));
        assert_eq!(tray_click_button(0x0208), Some(MouseButton::Middle));
    }

    #[test]
    fn tray_click_button_ignores_down_and_move_messages() {
        // WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_LBUTTONDBLCLK — none of these
        // are a completed click.
        assert_eq!(tray_click_button(0x0201), None);
        assert_eq!(tray_click_button(0x0200), None);
        assert_eq!(tray_click_button(0x0203), None);
    }

    #[test]
    fn plain_click_event_carries_the_button_through() {
        assert_eq!(
            plain_click_event(MouseButton::Right),
            UiEvent::TrayClicked {
                button: MouseButton::Right
            }
        );
    }
}
