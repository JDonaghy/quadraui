//! macOS implementation of [`quadraui::PlatformServices`].
//!
//! All surfaces ship as **stubs** in #35 so the `Backend` trait wiring
//! compiles end-to-end. **#36 replaces these with real implementations**:
//!
//! - Clipboard → `NSPasteboard` (general pasteboard, plain-text only
//!   for the first cut).
//! - File dialogs → `NSOpenPanel` / `NSSavePanel`.
//! - Notifications → `UNUserNotificationCenter` (macOS 10.14+).
//! - `open_url` → `NSWorkspace::openURL:`.
//!
//! `platform_name()` returns `"macos"` here today — the only piece
//! of the trait that `quadraui::dispatch` already consults during
//! `BackendNative` routing.

use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification};
use crate::PlatformServices;

/// macOS platform-services impl. Stubbed for #35; replaced in #36.
pub struct MacPlatformServices {
    clipboard: MacClipboard,
}

impl MacPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: MacClipboard,
        }
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

    fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        // #36 wires `NSOpenPanel::runModal`.
        None
    }

    fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        // #36 wires `NSSavePanel::runModal`.
        None
    }

    fn send_notification(&self, _n: Notification) {
        // #36 wires `UNUserNotificationCenter`.
    }

    fn open_url(&self, _url: &str) {
        // #36 wires `NSWorkspace::openURL:`.
    }

    fn platform_name(&self) -> &'static str {
        "macos"
    }
}

/// Stub clipboard. #36 replaces with an `NSPasteboard` impl.
pub struct MacClipboard;

impl Clipboard for MacClipboard {
    fn read_text(&self) -> Option<String> {
        None
    }

    fn write_text(&self, _text: &str) {}
}
