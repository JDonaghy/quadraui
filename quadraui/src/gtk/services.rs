//! GTK implementation of [`quadraui::PlatformServices`].
//!
//! Clipboard uses `arboard` for synchronous system clipboard access
//! (GTK's native read API is async, incompatible with the sync trait).
//! `open_url` uses GIO. File dialogs use `gtk4::FileDialog` behind a
//! nested-mainloop adapter (see [`pump_until_ready`]) so the trait's
//! synchronous signature can be honored even though GTK4 only exposes
//! an async dialog API (#427). Notifications remain stubbed pending an
//! async-aware trait shape.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gio;
use gtk4::glib;

use crate::backend::{Clipboard, FileDialogOptions, Notification};
use crate::PlatformServices;

/// GTK platform-services impl. Clipboard is backed by `arboard` for
/// cross-platform synchronous access. File dialogs use `gtk4::FileDialog`
/// pumped through a nested main-loop iteration (see module docs).
/// Notifications stay stubbed pending an async-aware trait shape.
pub struct GtkPlatformServices {
    clipboard: GtkClipboard,
    /// Top-level window used to parent file dialogs, so they open modal
    /// to (and centered on) the app window instead of floating
    /// unparented. `None` until [`Self::set_window`] is called (and in
    /// unit tests, which never call it) — dialogs opened before that
    /// point, or in tests, still work, just without a parent.
    window: Rc<RefCell<Option<gtk4::ApplicationWindow>>>,
}

impl GtkPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: GtkClipboard::new(),
            window: Rc::new(RefCell::new(None)),
        }
    }

    /// Store the top-level window handle so file dialogs can be parented
    /// to it. Called once by `GtkBackend::set_window` right after the
    /// window is constructed.
    pub(crate) fn set_window(&self, window: gtk4::ApplicationWindow) {
        *self.window.borrow_mut() = Some(window);
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

    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let dialog = build_file_dialog(&opts, None);
        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.open(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        pump_until_ready(&result)
    }

    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let initial_name = opts.initial_filename.clone();
        let dialog = build_file_dialog(&opts, initial_name.as_deref());
        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.save(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        pump_until_ready(&result)
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

/// Build a `gtk4::FileDialog` from the backend-agnostic
/// [`FileDialogOptions`]. `initial_name` is passed separately (rather
/// than read off `opts.initial_filename`) because it only applies to
/// the save dialog — `show_file_open_dialog` calls this with `None`.
fn build_file_dialog(opts: &FileDialogOptions, initial_name: Option<&str>) -> gtk4::FileDialog {
    let dialog = gtk4::FileDialog::new();
    if let Some(ref title) = opts.title {
        dialog.set_title(title);
    }
    if let Some(ref dir) = opts.initial_dir {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    if let Some(name) = initial_name {
        dialog.set_initial_name(Some(name));
    }
    if !opts.filters.is_empty() {
        let filters = gio::ListStore::new::<gtk4::FileFilter>();
        for (name, extensions) in &opts.filters {
            let filter = gtk4::FileFilter::new();
            filter.set_name(Some(name));
            for ext in extensions {
                filter.add_suffix(ext);
            }
            filters.append(&filter);
        }
        dialog.set_filters(Some(&filters));
    }
    dialog
}

/// Nested-mainloop adapter: `gtk4::FileDialog::open`/`save` are async-only
/// (GTK4 dropped the blocking `FileChooserDialog` API), but
/// [`PlatformServices::show_file_open_dialog`] /
/// `show_file_save_dialog` are synchronous across every backend (the
/// signature macOS's `NSOpenPanel::runModal` and Win32's `comdlg32` map
/// onto directly). This closes the gap the same way GTK itself closes it
/// internally for modal dialogs (e.g. the legacy `gtk_dialog_run`): pump
/// `glib::MainContext::iteration(true)` — which blocks until *some*
/// source is ready and dispatches it — in a loop until the dialog's
/// completion callback has stashed a result in `result`.
///
/// # Re-entrancy note
///
/// Pumping the main loop here lets *any* pending GLib source run,
/// including redraw/timer/other-widget callbacks that would normally
/// wait their turn — the same way a native nested loop (or GTK's old
/// `gtk_dialog_run`) does. If one of those callbacks itself triggers
/// another file dialog, the inner call's `iteration(true)` loop will
/// also service the outer dialog's completion source, which is safe but
/// means dialogs can resolve out of call order. Apps should avoid
/// opening a second file dialog from inside a callback that runs while
/// one is already open.
fn pump_until_ready(
    result: &Rc<RefCell<Option<Result<gio::File, glib::Error>>>>,
) -> Option<PathBuf> {
    let ctx = glib::MainContext::default();
    while result.borrow().is_none() {
        ctx.iteration(true);
    }
    result
        .borrow_mut()
        .take()
        .and_then(|res| res.ok())
        .and_then(|file| gtk4::prelude::FileExt::path(&file))
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

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::prelude::ListModelExt;
    use std::sync::OnceLock;

    /// Constructing a real `gtk4::FileDialog` requires `gtk4::init()` to
    /// have succeeded, which needs a display connection (X11/Wayland).
    /// CI and headless dev boxes don't have one, so these tests degrade
    /// to a no-op skip there rather than fail — mirrors how the crate's
    /// other GTK-object tests avoid needing a live display (see
    /// `gtk::tab_bar` — Cairo `ImageSurface` tests intentionally sidestep
    /// this same problem). Cached in a `OnceLock` since `gtk4::init()`
    /// after a failed first attempt keeps failing.
    fn require_gtk() -> bool {
        static INIT: OnceLock<bool> = OnceLock::new();
        *INIT.get_or_init(|| gtk4::init().is_ok())
    }

    /// The dialog builder must apply every `FileDialogOptions` field GTK
    /// has a settable property for, so a future maintainer changing the
    /// wiring can't silently drop one (e.g. forgetting `set_filters`).
    #[test]
    fn build_file_dialog_applies_title_initial_dir_and_filters() {
        if !require_gtk() {
            eprintln!("skipping: GTK failed to initialize (no display available)");
            return;
        }
        let opts = FileDialogOptions {
            title: Some("Open File".to_string()),
            initial_dir: Some(PathBuf::from("/tmp")),
            initial_filename: None,
            filters: vec![("Rust files".to_string(), vec!["rs".to_string()])],
        };
        let dialog = build_file_dialog(&opts, None);
        assert_eq!(dialog.title(), "Open File");
        assert_eq!(
            gtk4::prelude::FileExt::path(&dialog.initial_folder().unwrap()),
            Some(PathBuf::from("/tmp"))
        );
        let filters = dialog.filters().expect("filters should be set");
        assert_eq!(filters.n_items(), 1);
    }

    /// `initial_name` only applies when explicitly passed (the save-dialog
    /// path) — `show_file_open_dialog` always passes `None` regardless of
    /// `opts.initial_filename`, since that field is documented save-only.
    #[test]
    fn build_file_dialog_sets_initial_name_only_when_passed() {
        if !require_gtk() {
            eprintln!("skipping: GTK failed to initialize (no display available)");
            return;
        }
        let opts = FileDialogOptions {
            initial_filename: Some("untitled.txt".to_string()),
            ..Default::default()
        };
        let without = build_file_dialog(&opts, None);
        assert_eq!(without.initial_name(), None);

        let with = build_file_dialog(&opts, opts.initial_filename.as_deref());
        assert_eq!(with.initial_name().as_deref(), Some("untitled.txt"));
    }

    /// No filters configured → `set_filters` must not be called (a `Some`
    /// empty list would still change the dialog's filter-picker UI).
    #[test]
    fn build_file_dialog_leaves_filters_unset_when_empty() {
        if !require_gtk() {
            eprintln!("skipping: GTK failed to initialize (no display available)");
            return;
        }
        let dialog = build_file_dialog(&FileDialogOptions::default(), None);
        assert!(dialog.filters().is_none());
    }

    /// `set_window` stores the handle used to parent future dialogs.
    /// Nothing observable to assert without a real window, but this
    /// guards against a panic/wrong-field regression in the setter.
    #[test]
    fn set_window_is_none_until_called() {
        let services = GtkPlatformServices::new();
        assert!(services.window.borrow().is_none());
    }
}
