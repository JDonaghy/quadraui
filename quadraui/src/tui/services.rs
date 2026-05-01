//! Default `PlatformServices` impl for the TUI backend.
//!
//! TUI is the lowest-common-denominator backend: there's no native
//! file picker, no native notification system, no integrated browser
//! launcher. Apps that want richer services (real clipboard, OSC 52
//! integration, etc.) supply their own `PlatformServices` — the
//! [`TuiPlatformServices`] here is a no-op default the runner can
//! plug in for apps that don't customise.

use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification, PlatformServices};

/// TUI clipboard stub. Returns `None` on read; ignores writes.
pub struct TuiClipboard;

impl Clipboard for TuiClipboard {
    fn read_text(&self) -> Option<String> {
        None
    }

    fn write_text(&self, _text: &str) {}
}

/// Default `PlatformServices` impl for the TUI backend.
pub struct TuiPlatformServices {
    clipboard: TuiClipboard,
}

impl TuiPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: TuiClipboard,
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
