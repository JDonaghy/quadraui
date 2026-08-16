//! Win-GUI platform services stub.

use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification, PlatformServices};

pub struct WinClipboard;

impl Clipboard for WinClipboard {
    fn read_text(&self) -> Option<String> {
        // TODO(#23): Win32 clipboard: GetClipboardData(CF_UNICODETEXT).
        // Graceful no-op until then — no clipboard content available.
        None
    }
    fn write_text(&self, _text: &str) {
        // TODO(#23): Win32 clipboard: SetClipboardData(CF_UNICODETEXT).
        // Graceful no-op until then.
    }
}

pub struct WinPlatformServices {
    clipboard: WinClipboard,
}

impl WinPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: WinClipboard,
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
        "windows"
    }

    fn clipboard(&self) -> &dyn Clipboard {
        &self.clipboard
    }

    fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        // TODO(#23): IFileOpenDialog / GetOpenFileName.
        // Graceful no-op until then — behaves like a cancelled dialog.
        None
    }

    fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        // TODO(#23): IFileSaveDialog / GetSaveFileName.
        // Graceful no-op until then — behaves like a cancelled dialog.
        None
    }

    fn send_notification(&self, _n: Notification) {
        // TODO(#23): Win32 toast notification or balloon tip.
        // Graceful no-op until then.
    }

    fn open_url(&self, _url: &str) {
        // TODO(#23): ShellExecute(NULL, "open", url, ...).
        // Graceful no-op until then.
    }
}
