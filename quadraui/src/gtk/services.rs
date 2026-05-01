//! GTK implementation of [`quadraui::PlatformServices`].
//!
//! Stage 7 wires the synchronous-API methods through real GTK calls:
//! `Clipboard::write_text` and `PlatformServices::open_url`. The
//! async-API methods (`Clipboard::read_text`, `show_file_*_dialog`)
//! stay stubbed because the trait shape is synchronous and GTK's
//! native versions are async — vimcode's existing GTK code calls
//! those native APIs directly with `await`-style callbacks; the trait
//! surface is the forward-compat escape hatch a future shape change
//! would route through.

use std::path::PathBuf;

use gtk4::gdk;
use gtk4::prelude::*;

use crate::backend::{Clipboard, FileDialogOptions, Notification};
use crate::PlatformServices;

/// GTK platform-services impl. Holds a `GtkClipboard` proxy that
/// reaches the GDK display's clipboard each call. Other surfaces
/// (file dialogs, notifications) stay stubbed pending an async-aware
/// trait shape.
pub struct GtkPlatformServices {
    clipboard: GtkClipboard,
}

impl GtkPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: GtkClipboard,
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
        // GTK's `gtk::FileDialog::open` is async (callback-style). The
        // trait's synchronous return doesn't fit. Today vimcode
        // invokes FileDialog directly from `mod.rs` and feeds the
        // result back through Relm4 messages.
        None
    }

    fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        None
    }

    fn send_notification(&self, _n: Notification) {
        // Same async-vs-sync mismatch. `gio::Application::send_notification`
        // requires a registered Application id and an active GMainContext.
    }

    fn open_url(&self, url: &str) {
        // `gio::AppInfo::launch_default_for_uri` is synchronous and
        // doesn't need a parent window or active GMainContext.
        // Failures (no handler, malformed URI) are swallowed — the
        // worst case is a no-op, same as TUI's stub.
        let _ =
            gtk4::gio::AppInfo::launch_default_for_uri(url, None::<&gtk4::gio::AppLaunchContext>);
    }

    fn platform_name(&self) -> &'static str {
        "gtk"
    }
}

/// GTK clipboard proxy. `write_text` writes to the default display's
/// clipboard via `gdk::Clipboard::set_text` — synchronous. `read_text`
/// stays a stub because GTK's clipboard read API is async (callback-
/// style); the trait's `-> Option<String>` shape can't await without
/// a runtime swap, and vimcode's existing engine clipboard callbacks
/// (set in `mod.rs::setup_clipboard`) cover the read path today.
pub struct GtkClipboard;

impl Clipboard for GtkClipboard {
    fn read_text(&self) -> Option<String> {
        None
    }

    fn write_text(&self, text: &str) {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(text);
        }
    }
}
