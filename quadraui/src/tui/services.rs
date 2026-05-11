//! Default `PlatformServices` impl for the TUI backend.
//!
//! Clipboard access uses `arboard` for real system clipboard
//! integration on all platforms. Other services (file picker,
//! notifications, URL open) remain no-op stubs — apps that need them
//! supply their own `PlatformServices` or call platform APIs directly.

use std::cell::RefCell;
use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification, PlatformServices};

/// System clipboard via `arboard`. The handle is kept alive for the
/// process lifetime so Linux clipboard serving threads persist (dropping
/// the handle immediately would clear clipboard contents on X11/Wayland).
pub struct TuiClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl TuiClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
        }
    }
}

impl Clipboard for TuiClipboard {
    fn read_text(&self) -> Option<String> {
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }
}

/// Default `PlatformServices` impl for the TUI backend.
pub struct TuiPlatformServices {
    clipboard: TuiClipboard,
}

impl TuiPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: TuiClipboard::new(),
        }
    }
}

impl Default for TuiPlatformServices {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for TuiPlatformServices {
    fn clipboard(&self) -> &dyn Clipboard {
        &self.clipboard
    }

    fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        None
    }

    fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        None
    }

    fn send_notification(&self, _n: Notification) {}

    fn open_url(&self, _url: &str) {}

    fn platform_name(&self) -> &'static str {
        "tui"
    }
}
