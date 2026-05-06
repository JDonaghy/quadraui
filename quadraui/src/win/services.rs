//! Win-GUI platform services stub.

use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification, PlatformServices};

pub struct WinClipboard;

impl Clipboard for WinClipboard {
    fn read_text(&self) -> Option<String> {
        todo!("Win32 clipboard: GetClipboardData(CF_UNICODETEXT)")
    }
    fn write_text(&self, _text: &str) {
        todo!("Win32 clipboard: SetClipboardData(CF_UNICODETEXT)")
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
        todo!("IFileOpenDialog / GetOpenFileName")
    }

    fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        todo!("IFileSaveDialog / GetSaveFileName")
    }

    fn send_notification(&self, _n: Notification) {
        todo!("Win32 toast notification or balloon tip")
    }

    fn open_url(&self, _url: &str) {
        todo!("ShellExecute(NULL, \"open\", url, ...)")
    }
}
