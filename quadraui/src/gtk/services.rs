//! GTK implementation of [`quadraui::PlatformServices`].
//!
//! Clipboard uses `arboard` for synchronous system clipboard access
//! (GTK's native read API is async, incompatible with the sync trait).
//! `open_url` uses GIO. File dialogs and notifications remain stubbed
//! pending an async-aware trait shape.

use std::cell::RefCell;
use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification};
use crate::PlatformServices;

/// GTK platform-services impl. Clipboard is backed by `arboard` for
/// cross-platform synchronous access. Other surfaces (file dialogs,
/// notifications) stay stubbed pending an async-aware trait shape.
pub struct GtkPlatformServices {
    clipboard: GtkClipboard,
}

impl GtkPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: GtkClipboard::new(),
        }
    }
}

impl Default for GtkPlatformServices {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for GtkPlatformServices {
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

    fn open_url(&self, url: &str) {
        let _ =
            gtk4::gio::AppInfo::launch_default_for_uri(url, None::<&gtk4::gio::AppLaunchContext>);
    }

    fn platform_name(&self) -> &'static str {
        "gtk"
    }
}

/// System clipboard via `arboard`. The handle is kept alive for the
/// process lifetime so Linux clipboard serving threads persist.
pub struct GtkClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl GtkClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
        }
    }
}

impl Clipboard for GtkClipboard {
    fn read_text(&self) -> Option<String> {
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }
}
