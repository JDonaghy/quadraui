//! macOS implementation of [`quadraui::PlatformServices`].
//!
//! - **Clipboard** → `arboard` (NSPasteboard under the hood; the sync
//!   API matches the [`Clipboard`] trait exactly, and shares one
//!   implementation with the TUI and GTK backends).
//! - **File dialogs** → `NSOpenPanel` / `NSSavePanel`, sync
//!   `runModal`. Apps must invoke these from event handlers running
//!   on the main thread (the `MacBackend` run loop guarantees this).
//! - **Notifications** → `osascript -e 'display notification ...'`.
//!   `UNUserNotificationCenter` requires a bundled `.app` with
//!   `CFBundleIdentifier` and user authorization, neither of which
//!   suit an unbundled CLI host. The osascript route works for both
//!   bundled and unbundled hosts.
//! - **`open_url`** → `open <url>`. Equivalent to
//!   `NSWorkspace.open(_:)` without needing AppKit initialisation.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;

use objc2_app_kit::{NSOpenPanel, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};

use crate::backend::{Clipboard, FileDialogOptions, Notification};
use crate::PlatformServices;

/// `NSModalResponseOK` — the user clicked Open / Save.
const NS_MODAL_RESPONSE_OK: isize = 1;

/// macOS platform services backed by AppKit + `arboard` + shell-out
/// helpers. Constructed by [`crate::macos::MacBackend::new`] and
/// exposed through [`crate::Backend::services`].
pub struct MacPlatformServices {
    clipboard: MacClipboard,
}

impl MacPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: MacClipboard::new(),
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

    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_file_open_dialog must be called from the main thread");
        // SAFETY: AppKit panels are constructed and driven exclusively
        // on the main thread; MainThreadMarker enforces that.
        unsafe {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            configure_panel(&panel, &opts);
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_file_save_dialog must be called from the main thread");
        // SAFETY: same as `show_file_open_dialog` above.
        unsafe {
            let panel = NSSavePanel::savePanel(mtm);
            configure_panel(&panel, &opts);
            if let Some(ref name) = opts.initial_filename {
                panel.setNameFieldStringValue(&NSString::from_str(name));
            }
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    fn send_notification(&self, n: Notification) {
        let script = format!(
            "display notification \"{body}\" with title \"{title}\"",
            body = applescript_escape(&n.body),
            title = applescript_escape(&n.title),
        );
        let _ = Command::new("osascript").arg("-e").arg(&script).spawn();
    }

    fn open_url(&self, url: &str) {
        let _ = Command::new("open").arg(url).spawn();
    }

    fn platform_name(&self) -> &'static str {
        "macos"
    }
}

/// Apply common options (message, initial directory, file-type filters)
/// to an `NSSavePanel` — also covers `NSOpenPanel`, which inherits all
/// of these setters from `NSSavePanel`.
///
/// # Safety
///
/// Must be called from the main thread; `panel` must be a valid panel
/// retrieved on this same thread via `openPanel:` / `savePanel:`.
unsafe fn configure_panel(panel: &NSSavePanel, opts: &FileDialogOptions) {
    if let Some(ref title) = opts.title {
        panel.setMessage(Some(&NSString::from_str(title)));
    }
    if let Some(ref dir) = opts.initial_dir {
        if let Some(dir_str) = dir.to_str() {
            let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(dir_str), true);
            panel.setDirectoryURL(Some(&url));
        }
    }
    let exts: Vec<_> = opts
        .filters
        .iter()
        .flat_map(|(_, e)| e.iter())
        .map(|ext| NSString::from_str(ext))
        .collect();
    if !exts.is_empty() {
        // `NSArray::from_slice` requires `T: IsRetainable`, which
        // NSString doesn't satisfy (it has an `NSMutableString`
        // subclass). `from_vec` consumes owned retained handles and
        // sidesteps that bound.
        let arr = NSArray::from_vec(exts);
        // `setAllowedFileTypes:` is deprecated in favour of
        // `setAllowedContentTypes:` (UTType), but that requires the
        // UniformTypeIdentifiers framework which objc2 doesn't yet
        // wrap. The legacy API still works on macOS 11–15.
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&arr));
    }
}

/// Convert an `NSURL` back to a Rust `PathBuf`. Returns `None` if the
/// URL is missing or non-file-scheme.
///
/// # Safety
///
/// `url` must be a valid `NSURL` retrieved on the main thread.
unsafe fn url_to_path(url: Option<&NSURL>) -> Option<PathBuf> {
    let path = url?.path()?;
    Some(PathBuf::from(path.to_string()))
}

/// Escape `"` and `\` so a string can be embedded inside an
/// AppleScript double-quoted string literal.
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// System clipboard via `arboard` (NSPasteboard under the hood on
/// macOS). Held for the lifetime of `MacPlatformServices` so the
/// pasteboard handle outlives any cached connection state.
pub struct MacClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl MacClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
        }
    }
}

impl Clipboard for MacClipboard {
    fn read_text(&self) -> Option<String> {
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_macos() {
        let svc = MacPlatformServices::new();
        assert_eq!(svc.platform_name(), "macos");
    }

    #[test]
    fn applescript_escape_handles_quotes_and_backslashes() {
        // Empty + pass-through.
        assert_eq!(applescript_escape(""), "");
        assert_eq!(applescript_escape("plain text"), "plain text");
        // Single-character escapes.
        assert_eq!(applescript_escape("a\"b"), "a\\\"b");
        assert_eq!(applescript_escape("c\\d"), "c\\\\d");
        // Combined. Backslash MUST be escaped first so the subsequent
        // quote-escape's added backslashes aren't re-escaped.
        assert_eq!(applescript_escape("e\"f\\g"), "e\\\"f\\\\g");
        // Order check: a backslash followed by a quote in input
        // should produce `\\\"` (escaped slash + escaped quote),
        // not `\\\\\"` (double-escaped slash + quote).
        assert_eq!(applescript_escape("\\\""), "\\\\\\\"");
    }
}
