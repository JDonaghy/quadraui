//! GTK implementation of [`quadraui::PlatformServices`].
//!
//! Clipboard uses `arboard` for synchronous system clipboard access
//! (GTK's native read API is async, incompatible with the sync trait).
//! `open_url` uses GIO. File dialogs use `gtk4::FileDialog`, and message
//! dialogs use `gtk4::AlertDialog` (quadraui#666) — `gtk4::MessageDialog`
//! has been deprecated since GTK 4.10, and the fleet is on 4.14.5 — both
//! behind the same nested-mainloop adapter (see [`pump_until_ready`]) so
//! the trait's synchronous signatures can be honored even though GTK4
//! only exposes async dialog APIs (#427). Notifications remain stubbed
//! pending an async-aware trait shape.
//!
//! ## Re-entrancy guard (#427 follow-up)
//!
//! `pump_until_ready` is called from inside `AppLogic::handle`, which
//! `quadraui::gtk::run` invokes while holding the shared
//! `Rc<RefCell<GtkBackend>>` mutably borrowed for the whole call. Pumping
//! `glib::MainContext::iteration(true)` in that state lets *any* pending
//! GLib source run — including the runner's own 33ms idle-drain timer and
//! every input event controller, all of which also do
//! `backend.borrow_mut()`. Left unguarded, that second borrow panics with
//! "already borrowed", and because it happens inside a non-unwindable GLib
//! callback frame, the panic aborts the whole process instead of
//! propagating. `pumping` (a depth counter, not a bool, so nested dialogs
//! stay guarded until the outermost pump finishes) lets those callbacks
//! detect "a dialog pump is in flight further up the stack" and no-op
//! instead of touching the backend. See `GtkBackend::pump_depth` /
//! `quadraui::gtk::run::activate`.
//!
//! The depth counter and its RAII guard are the backend-neutral
//! [`crate::desktop::ModalPumpDepth`] / [`crate::desktop::ModalPumpGuard`]
//! (#498) — extracted here first (#427) and generalised because AppKit's
//! `runModal` and Win32's `IFileOpenDialog::Show` have the identical
//! nested-pump re-entrancy hazard.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gio;
use gtk4::glib;

use crate::backend::{
    Clipboard, FileDialogOptions, MessageDialogButton, MessageDialogChoice, MessageDialogOptions,
    Notification,
};
use crate::desktop::{ModalPumpDepth, ModalPumpGuard};
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
    /// Depth counter, `> 0` while a [`pump_until_ready`] nested-mainloop
    /// wait is in flight (possibly several, if a dialog is opened
    /// re-entrantly from inside another dialog's pump). Shared (via
    /// [`Self::pump_depth`]) with `quadraui::gtk::run`'s event
    /// controllers and idle-drain timer so they can detect the
    /// re-entrant-pump condition and skip touching the backend's
    /// `RefCell` — see the module-level re-entrancy note.
    pumping: ModalPumpDepth,
}

impl GtkPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: GtkClipboard::new(),
            window: Rc::new(RefCell::new(None)),
            pumping: ModalPumpDepth::new(),
        }
    }

    /// Store the top-level window handle so file dialogs can be parented
    /// to it. Called once by `GtkBackend::set_window` right after the
    /// window is constructed.
    pub(crate) fn set_window(&self, window: gtk4::ApplicationWindow) {
        *self.window.borrow_mut() = Some(window);
    }

    /// Clone of the pump-depth counter (see the `pumping` field docs).
    /// `quadraui::gtk::run::activate` fetches this once, before installing
    /// any event controllers, and clones it into each closure that would
    /// otherwise call `backend.borrow_mut()` — so they can check
    /// `depth.is_pumping()` and no-op while a dialog's nested pump is live.
    pub(crate) fn pump_depth(&self) -> ModalPumpDepth {
        self.pumping.clone()
    }

    /// Concrete (non-trait-object) handle on the clipboard, so in-crate
    /// tests can call [`GtkClipboard::install_test_contents`] — the
    /// `&dyn Clipboard` returned by [`PlatformServices::clipboard`]
    /// can't reach an inherent method. Test-only (quadraui#415).
    #[cfg(test)]
    pub(crate) fn gtk_clipboard(&self) -> &GtkClipboard {
        &self.clipboard
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
        pump_until_ready(&result, &self.pumping)
            .ok()
            .and_then(|file| gtk4::prelude::FileExt::path(&file))
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
        pump_until_ready(&result, &self.pumping)
            .ok()
            .and_then(|file| gtk4::prelude::FileExt::path(&file))
    }

    /// `gtk4::AlertDialog::choose()` (quadraui#666) — see the module docs
    /// for why `AlertDialog`, not the deprecated `MessageDialog`. Driven
    /// through the same [`pump_until_ready`] + `pump_depth` guard the
    /// file dialogs use above; this does **not** hand-roll a second
    /// nested loop.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        let order = hig_button_order(&opts.buttons);
        let labels: Vec<&str> = order
            .iter()
            .map(|&i| opts.buttons[i].label.as_str())
            .collect();

        let dialog = gtk4::AlertDialog::default();
        dialog.set_message(&opts.title);
        dialog.set_detail(&opts.body);
        dialog.set_modal(true);
        dialog.set_buttons(&labels);
        // `opts.severity` is intentionally unread here: gtk4-rs 0.7.3's
        // `AlertDialog` (`auto/alert_dialog.rs`) exposes no icon-setting
        // API at all, so there is nothing to map it onto. Not a bug —
        // `MessageDialogOptions::severity`'s doc already says "backends
        // *may* use this" — but worth calling out so the next reader
        // doesn't go looking for where it's supposed to go.
        if let Some(pos) = order.iter().position(|&i| opts.buttons[i].is_cancel) {
            dialog.set_cancel_button(pos as i32);
        }
        if let Some(pos) = order.iter().position(|&i| opts.buttons[i].is_default) {
            dialog.set_default_button(pos as i32);
        }

        let window = self.window.borrow().clone();
        let result = Rc::new(RefCell::new(None));
        let result_cb = Rc::clone(&result);
        dialog.choose(window.as_ref(), gio::Cancellable::NONE, move |res| {
            *result_cb.borrow_mut() = Some(res);
        });
        let idx = pump_until_ready(&result, &self.pumping).ok()?;
        let pos = usize::try_from(idx).ok()?;
        let orig = *order.get(pos)?;
        Some(opts.buttons[orig].id.clone())
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

/// Nested-mainloop adapter: `gtk4::FileDialog::open`/`save` and
/// `gtk4::AlertDialog::choose` (quadraui#666) are async-only (GTK4
/// dropped the blocking `FileChooserDialog` API and never had a blocking
/// alert), but [`PlatformServices::show_file_open_dialog`] /
/// `show_file_save_dialog` / `show_message_dialog` are synchronous
/// across every backend (the signature macOS's `NSOpenPanel::runModal` /
/// `NSAlert::runModal` and Win32's `comdlg32` / `MessageBoxEx` map onto
/// directly). This closes the gap the same way GTK itself closes it
/// internally for modal dialogs (e.g. the legacy `gtk_dialog_run`): pump
/// `glib::MainContext::iteration(true)` — which blocks until *some*
/// source is ready and dispatches it — in a loop until the async
/// operation's completion callback has stashed a result in `result`.
/// Generic over the result type so every blocking-dialog call site
/// (file open/save, message/alert) shares this one pump instead of each
/// hand-rolling its own nested loop.
///
/// # Re-entrancy note
///
/// Pumping the main loop here lets *any* pending GLib source run,
/// including redraw/timer/other-widget callbacks that would normally
/// wait their turn — the same way a native nested loop (or GTK's old
/// `gtk_dialog_run`) does. If one of those callbacks itself triggers
/// another dialog, the inner call's `iteration(true)` loop will also
/// service the outer dialog's completion source, which is safe but
/// means dialogs can resolve out of call order. Apps should avoid
/// opening a second dialog from inside a callback that runs while one is
/// already open.
///
/// Critically, this is called while `quadraui::gtk::run`'s caller (an
/// `AppLogic::handle` invocation) still holds the shared
/// `GtkBackend`'s `RefCell` mutably borrowed. `pumping` — bumped for the
/// duration of this call — is how the runner's other backend-touching
/// callbacks (idle-drain timer, input controllers, draw func) detect
/// that and skip their own `backend.borrow_mut()` instead of panicking
/// on a double-borrow (#427).
fn pump_until_ready<T>(result: &Rc<RefCell<Option<T>>>, pumping: &ModalPumpDepth) -> T {
    let _guard = ModalPumpGuard::new(pumping);
    let ctx = glib::MainContext::default();
    while result.borrow().is_none() {
        ctx.iteration(true);
    }
    result
        .borrow_mut()
        .take()
        .expect("loop above only exits once `result` is Some")
}

/// GNOME HIG button ordering for `gtk4::AlertDialog::set_buttons`: the
/// cancel button (if any) goes first (leftmost); the default/primary
/// button (if any) goes last (rightmost); every other button keeps its
/// original relative order in between. Returns indices into `buttons`,
/// in the order to hand to `set_buttons` — `show_message_dialog` maps
/// the chosen index back through this same `Vec` to recover the
/// original [`MessageDialogButton::id`].
///
/// A button with both `is_default` and `is_cancel` set (a single-button
/// "OK" dialog) is treated as the cancel slot here — see below, both
/// `set_cancel_button` and `set_default_button` are still pointed at its
/// position afterward regardless of which bucket placed it.
fn hig_button_order(buttons: &[MessageDialogButton]) -> Vec<usize> {
    let mut cancel_idx = None;
    let mut default_idx = None;
    let mut middle = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        if b.is_cancel && cancel_idx.is_none() {
            cancel_idx = Some(i);
        } else if b.is_default && default_idx.is_none() {
            default_idx = Some(i);
        } else {
            middle.push(i);
        }
    }
    let mut order = Vec::with_capacity(buttons.len());
    order.extend(cancel_idx);
    order.extend(middle);
    if let Some(d) = default_idx {
        order.push(d);
    }
    order
}

// The RAII bump/decrement for the shared pump-depth counter used to be
// defined here as `PumpGuard`; extracted to the backend-neutral
// `crate::desktop::ModalPumpGuard` by #498 — see that module's doc.

/// In-memory stand-in for the two OS selections, installed by
/// [`GtkClipboard::install_test_contents`] so unit tests can exercise the
/// clipboard-paste code paths **without** depending on whatever the host
/// running the tests happens to have on its real clipboard (quadraui#415).
///
/// Test-only on purpose: without it, a test asserting "nothing to paste"
/// passes on a headless box (where `arboard::Clipboard::new()` fails) and
/// fails on any developer machine or CI runner with a live display and a
/// non-empty clipboard — a genuinely flaky, environment-dependent
/// assertion rather than a statement about quadraui's behaviour.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct TestClipboardContents {
    /// Contents of the CLIPBOARD selection — what Ctrl-V / Ctrl-Shift-V
    /// reads. `None` models "nothing has been copied".
    pub clipboard: Option<String>,
    /// Contents of the PRIMARY selection — what middle-click reads.
    /// `None` models "nothing is selected anywhere".
    pub primary: Option<String>,
}

/// System clipboard via `arboard`. The handle is kept alive for the
/// process lifetime so Linux clipboard serving threads persist.
pub struct GtkClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
    /// When `Some`, every read/write below is served from this in-memory
    /// fake instead of the OS, making clipboard-dependent unit tests
    /// deterministic on any host. Only ever populated by
    /// [`Self::install_test_contents`]; production builds don't compile
    /// the field at all.
    #[cfg(test)]
    test_contents: RefCell<Option<TestClipboardContents>>,
}

impl GtkClipboard {
    fn new() -> Self {
        Self {
            inner: RefCell::new(arboard::Clipboard::new().ok()),
            #[cfg(test)]
            test_contents: RefCell::new(None),
        }
    }

    /// Swap this clipboard over to an in-memory fake seeded with
    /// `contents`. Every subsequent read and write goes to the fake and
    /// the OS clipboard is left untouched — so a test can assert on the
    /// "there is something to paste" and "there is nothing to paste"
    /// branches independently of the host (quadraui#415).
    #[cfg(test)]
    pub(crate) fn install_test_contents(&self, contents: TestClipboardContents) {
        *self.test_contents.borrow_mut() = Some(contents);
    }

    /// OS-level PRIMARY-selection read, split out from the trait method
    /// so the trait method itself stays uncfg'd (and therefore honours
    /// [`Self::install_test_contents`] on every target).
    ///
    /// Gated to the same `cfg` `arboard` itself uses for `GetExtLinux`
    /// (see `arboard::lib`). Windows/macOS have no PRIMARY-selection
    /// concept, and quadraui's `gtk` feature only ships a Linux backend
    /// in practice, so there the OS read is simply `None`.
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    ))]
    fn read_os_primary_selection(&self) -> Option<String> {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        self.inner
            .borrow_mut()
            .as_mut()?
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok()
    }

    /// Non-Linux counterpart of [`Self::read_os_primary_selection`] —
    /// there is no PRIMARY selection to read.
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    )))]
    fn read_os_primary_selection(&self) -> Option<String> {
        None
    }
}

impl Clipboard for GtkClipboard {
    fn read_text(&self) -> Option<String> {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow().as_ref() {
                return fake.clipboard.clone();
            }
        }
        self.inner.borrow_mut().as_mut()?.get_text().ok()
    }

    fn write_text(&self, text: &str) {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow_mut().as_mut() {
                fake.clipboard = Some(text.to_string());
                return;
            }
        }
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
    }

    /// PRIMARY selection (middle-click paste source) — only meaningful on
    /// X11/Wayland, where `arboard`'s Linux extension trait exposes it as
    /// a distinct selection from CLIPBOARD (quadraui#415). See
    /// [`GtkClipboard::read_os_primary_selection`] for the per-target
    /// split.
    fn read_primary_selection(&self) -> Option<String> {
        #[cfg(test)]
        {
            if let Some(fake) = self.test_contents.borrow().as_ref() {
                return fake.primary.clone();
            }
        }
        self.read_os_primary_selection()
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
    ///
    /// # Thread affinity (#460)
    ///
    /// gtk4-rs's thread-affinity guard is a per-thread flag
    /// (`IS_MAIN_THREAD`, a `thread_local!` in gtk4-rs's `rt.rs`) set only
    /// on whichever thread's call actually ran `gtk_init_check()`. Rust's
    /// default test harness runs every `#[test]` fn on its own spawned OS
    /// thread, so if more than one `#[test]` fn calls `require_gtk()`,
    /// only the winner of this `OnceLock` race — one specific thread —
    /// is ever GTK-safe; every other test's thread reads the cached
    /// `true` without ever calling `gtk4::init()` itself, then panics
    /// with "GTK may only be used from the main thread" the moment it
    /// touches a real GTK object. gtk4-rs ships `#[gtk4::test]` /
    /// `test_synced` for exactly this (marshal every GTK-touching test
    /// onto one dedicated worker thread) — not used here because its
    /// `Lazy` worker-thread init calls `gtk4::init().expect(...)`
    /// unconditionally, with no graceful-skip path, which would turn a
    /// headless dev box's "no display" case into a hard panic instead of
    /// the skip this helper is designed to give it. So instead, every
    /// `#[test]` fn that needs a real `gtk4::FileDialog` lives in
    /// [`build_file_dialog_behaviors`] below — one function body, hence
    /// one OS thread, guaranteeing it's the same thread that (first) runs
    /// `gtk4::init()`. Do **not** add another separate `#[test]` fn that
    /// calls `require_gtk()` and then constructs a real GTK object —
    /// fold its assertions into `build_file_dialog_behaviors` instead, or
    /// this panic comes back.
    fn require_gtk() -> bool {
        static INIT: OnceLock<bool> = OnceLock::new();
        *INIT.get_or_init(|| gtk4::init().is_ok())
    }

    /// Covers every `build_file_dialog` behavior that requires
    /// constructing a real `gtk4::FileDialog`. Deliberately one `#[test]`
    /// fn (not three) — see the thread-affinity note on [`require_gtk`]
    /// (#460): splitting these across separate `#[test]` fns lets the
    /// Rust test harness run them on different OS threads, which
    /// deterministically panics every thread except whichever one wins
    /// the `require_gtk()` / `gtk4::init()` race.
    // #619: exempt from the crate-wide `print_stderr` deny. `cargo test`
    // output, not code that runs inside a host's live UI session — this
    // line only ever executes on a headless dev box with no display,
    // where it's the reason the test reports "ok" without covering
    // anything.
    #[allow(clippy::print_stderr)]
    #[test]
    fn build_file_dialog_behaviors() {
        if !require_gtk() {
            eprintln!("skipping: GTK failed to initialize (no display available)");
            return;
        }

        // The dialog builder must apply every `FileDialogOptions` field
        // GTK has a settable property for, so a future maintainer
        // changing the wiring can't silently drop one (e.g. forgetting
        // `set_filters`).
        {
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

        // `initial_name` only applies when explicitly passed (the
        // save-dialog path) — `show_file_open_dialog` always passes
        // `None` regardless of `opts.initial_filename`, since that field
        // is documented save-only.
        {
            let opts = FileDialogOptions {
                initial_filename: Some("untitled.txt".to_string()),
                ..Default::default()
            };
            let without = build_file_dialog(&opts, None);
            assert_eq!(without.initial_name(), None);

            let with = build_file_dialog(&opts, opts.initial_filename.as_deref());
            assert_eq!(with.initial_name().as_deref(), Some("untitled.txt"));
        }

        // No filters configured → `set_filters` must not be called (a
        // `Some` empty list would still change the dialog's
        // filter-picker UI).
        {
            let dialog = build_file_dialog(&FileDialogOptions::default(), None);
            assert!(dialog.filters().is_none());
        }
    }

    /// `set_window` stores the handle used to parent future dialogs.
    /// Nothing observable to assert without a real window, but this
    /// guards against a panic/wrong-field regression in the setter.
    #[test]
    fn set_window_is_none_until_called() {
        let services = GtkPlatformServices::new();
        assert!(services.window.borrow().is_none());
    }

    // Regression test for #427 ("depth counter stays positive until the
    // outermost re-entrant pump unwinds") used to live here directly
    // against a local `PumpGuard`; that behavior is now covered once,
    // backend-neutrally, by `crate::desktop`'s own `modal_pump_tests`
    // (#498). The test below stays GTK-specific: it pins that
    // `GtkPlatformServices::pump_depth()`'s returned handle really does
    // share state with the services' own counter (same `Rc`, not an
    // independent clone).

    /// `GtkPlatformServices::pump_depth()` — the handle
    /// `quadraui::gtk::run::activate` clones into its event
    /// controllers — must observe mutations `pump_until_ready` makes
    /// through the services' own copy (same underlying counter via
    /// `ModalPumpDepth`'s `Rc`, not an independent one).
    #[test]
    fn pump_depth_handle_observes_guard_mutations_on_the_services_copy() {
        let services = GtkPlatformServices::new();
        let handle = services.pump_depth();
        assert_eq!(handle.get(), 0);
        let _guard = ModalPumpGuard::new(&services.pumping);
        assert_eq!(
            handle.get(),
            1,
            "handle must observe the mutation made through services.pumping"
        );
    }

    // ── hig_button_order (quadraui#666) ────────────────────────────────

    fn msg_btn(id: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
        MessageDialogButton {
            id: crate::types::WidgetId::new(id),
            label: id.to_string(),
            is_default,
            is_cancel,
        }
    }

    #[test]
    fn hig_button_order_puts_cancel_first_and_default_last() {
        let buttons = vec![
            msg_btn("save", true, false),
            msg_btn("dont-save", false, false),
            msg_btn("cancel", false, true),
        ];
        // Declared order is default, other, cancel — HIG order must
        // reshuffle to cancel, other, default.
        assert_eq!(hig_button_order(&buttons), vec![2, 1, 0]);
    }

    #[test]
    fn hig_button_order_preserves_middle_button_relative_order() {
        let buttons = vec![
            msg_btn("cancel", false, true),
            msg_btn("a", false, false),
            msg_btn("b", false, false),
            msg_btn("save", true, false),
        ];
        assert_eq!(hig_button_order(&buttons), vec![0, 1, 2, 3]);
    }

    #[test]
    fn hig_button_order_single_button_both_default_and_cancel() {
        // A single "OK" button dialog: is_cancel is checked first, so it
        // lands in the cancel slot — but `show_message_dialog` still
        // points both `set_cancel_button` and `set_default_button` at
        // its position since `is_default` also reads true on it.
        let buttons = vec![msg_btn("ok", true, true)];
        assert_eq!(hig_button_order(&buttons), vec![0]);
    }

    #[test]
    fn hig_button_order_no_default_or_cancel_keeps_declared_order() {
        let buttons = vec![msg_btn("a", false, false), msg_btn("b", false, false)];
        assert_eq!(hig_button_order(&buttons), vec![0, 1]);
    }
}
