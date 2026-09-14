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

use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSColor, NSColorSpace,
    NSOpenPanel, NSSavePanel, NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};

use crate::backend::{
    BackendError, Clipboard, FileDialogOptions, MessageDialogButton, MessageDialogChoice,
    MessageDialogOptions, Notification, ServiceResult, SystemTheme,
};
use crate::primitives::dialog::DialogSeverity;
use crate::types::Color;
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

    /// quadraui#935: a two-line variant of `show_file_open_dialog` above —
    /// the same `NSOpenPanel`, with the choose-directories/choose-files
    /// pair flipped.
    fn show_folder_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()
            .expect("show_folder_open_dialog must be called from the main thread");
        // SAFETY: same as `show_file_open_dialog` above.
        unsafe {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            configure_panel(&panel, &opts);
            if panel.runModal() != NS_MODAL_RESPONSE_OK {
                return None;
            }
            url_to_path(panel.URL().as_deref())
        }
    }

    /// `NSAlert` (quadraui#936) — the macOS half of the native-dialog gap
    /// #666 left unfiled after shipping the GTK4 `AlertDialog`
    /// implementation (`gtk::services::GtkPlatformServices::show_message_dialog`,
    /// this method's closest sibling). `native_button_order` picks which
    /// order to add buttons in (macOS convention: most-prominent /
    /// default button added first, since `NSAlert` lays out the
    /// first-added button trailing/rightmost — see `addButtonWithTitle:`'s
    /// docs); `severity_to_alert_style` maps `DialogSeverity` onto
    /// `NSAlertStyle`. Both are free functions below so the mapping is
    /// unit-testable without presenting anything (this issue's
    /// acceptance bar) — the modal `runModal()` call itself still needs a
    /// human on a real Mac.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        // Graceful degrade rather than `expect`'s hard panic (unlike the
        // file dialogs above): quadraui#926's `install_menu_bar` twin.
        // Rust's default test harness spawns a fresh OS thread per
        // `#[test]` fn, so a test that reaches this method off the main
        // thread must see `None` — the same "no native alert facility"
        // shape callers already get from `BackendCaps::native_dialogs ==
        // false` — instead of aborting the whole test process.
        // `NSAlert`'s methods below (unlike `NSOpenPanel`/`NSSavePanel`'s
        // `URL()` and this module's own `url_to_path`) are all safe
        // AppKit wrappers — `MainThreadMarker` still gates *construction*
        // (`NSAlert::new` takes it by value), which is what actually
        // enforces the main-thread requirement here.
        let mtm = MainThreadMarker::new()?;
        let order = native_button_order(&opts.buttons);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&opts.title));
        alert.setInformativeText(&NSString::from_str(&opts.body));
        alert.setAlertStyle(severity_to_alert_style(opts.severity));
        for &i in &order {
            let button = &opts.buttons[i];
            let ns_button = alert.addButtonWithTitle(&NSString::from_str(&button.label));
            if let Some(key) = key_equivalent_for_button(button) {
                ns_button.setKeyEquivalent(&NSString::from_str(key));
            }
        }
        let response = alert.runModal();
        let idx = usize::try_from(response - NSAlertFirstButtonReturn).ok()?;
        let orig = *order.get(idx)?;
        Some(opts.buttons[orig].id.clone())
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

    /// `NSApp.effectiveAppearance` for light/dark,
    /// `NSWorkspace::accessibilityDisplayShouldIncreaseContrast` for high
    /// contrast, `NSColor::controlAccentColor` for the accent colour
    /// (quadraui#952). Reads `effectiveAppearance().name()` and checks it
    /// for `"Dark"` rather than building an `NSArray` and calling
    /// `NSAppearance::bestMatchFromAppearancesWithNames` (the issue's
    /// suggested API, and the one apps *drawing* per-appearance content
    /// should reach for) — for a one-shot query like this, the name
    /// itself already carries the answer, and every system appearance
    /// name (`NSAppearanceNameDarkAqua`, and both
    /// `NSAppearanceNameAccessibilityHighContrast*Dark*` variants)
    /// contains the substring unambiguously.
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
        let appearance_name = NSApplication::sharedApplication(mtm)
            .effectiveAppearance()
            .name()
            .to_string();
        let high_contrast =
            NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
        Ok(system_theme_from_mac_appearance(
            &appearance_name,
            high_contrast,
            mac_accent_color(),
        ))
    }

    fn platform_name(&self) -> &'static str {
        "macos"
    }
}

/// Pure mapping from an `NSAppearance` name plus the two other
/// already-read signals to [`SystemTheme`] — split out from
/// `system_theme` so the name-matching logic is unit-testable without a
/// live `NSApplication` (mirrors this module's `native_button_order`/
/// `severity_to_alert_style`-style helpers).
fn system_theme_from_mac_appearance(
    appearance_name: &str,
    high_contrast: bool,
    accent: Option<Color>,
) -> SystemTheme {
    SystemTheme {
        dark: appearance_name.contains("Dark"),
        accent,
        high_contrast,
    }
}

/// `NSColor::controlAccentColor`'s RGB components, converted to
/// [`Color`]. `getRed:green:blue:alpha:` raises `NSInvalidArgumentException`
/// if the receiver isn't already in an RGB-compatible colour space, and an
/// ObjC exception unwinding across this Rust/FFI boundary is UB (likely an
/// abort) — Apple's docs don't guarantee `controlAccentColor` (a dynamic,
/// catalog-backed system colour) is pre-resolved to RGB in every runtime
/// context, so this defensively runs it through `colorUsingColorSpace:`
/// first rather than assuming it. `colorUsingColorSpace:` returns `None`
/// only when the conversion itself is impossible (no live colour-management
/// pipeline), which — like the rest of this crate's platform-services
/// layer — is treated as "feature unavailable" rather than an error.
fn mac_accent_color() -> Option<Color> {
    let color = NSColor::controlAccentColor();
    let rgb_color = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let (mut r, mut g, mut b, mut a) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    // SAFETY: four valid, non-null `f64` out-pointers, matching
    // `getRed:green:blue:alpha:`'s documented safety requirement. Calling
    // it on `rgb_color` rather than `color` is what makes this safe from
    // the ObjC-exception hazard described above: `colorUsingColorSpace:`
    // above already guarantees an RGB-compatible receiver.
    unsafe {
        rgb_color.getRed_green_blue_alpha(&mut r, &mut g, &mut b, &mut a);
    }
    Some(Color::rgba(
        unit_to_u8(r),
        unit_to_u8(g),
        unit_to_u8(b),
        unit_to_u8(a),
    ))
}

/// Convert a `0.0..=1.0` colour component to a `0..=255` byte, clamping
/// out-of-range input rather than wrapping/panicking (`as u8` on a
/// negative or `NaN` float is technically defined since Rust 1.45's
/// saturating float casts, but `clamp` here makes the intent explicit
/// rather than relying on that cast behaviour).
fn unit_to_u8(component: f64) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0).round() as u8
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
        // subclass). `from_retained_slice` takes already-`Retained`
        // handles and sidesteps that bound — replaces objc2-foundation
        // 0.2's `from_vec` (renamed/reshaped in 0.3, #796; same "already
        // retained" semantics, slice instead of by-value `Vec`).
        let arr = NSArray::from_retained_slice(&exts);
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

/// Order `buttons` for `NSAlert::addButtonWithTitle:` (quadraui#936):
/// `NSAlert` lays out the first-added button trailing/rightmost (its own
/// docs: "arranged from trailing-to-leading edge... Buttons should be
/// added from most-to-least prominent"), so the default/primary button
/// goes first, any cancel button goes second, and everything else keeps
/// its declared relative order after that. Returns indices into
/// `buttons` — `show_message_dialog` maps `runModal`'s chosen index back
/// through this same slice to recover the original
/// [`MessageDialogButton::id`], mirroring
/// `gtk::services::hig_button_order`'s job for the GTK4 `AlertDialog`.
///
/// A button with both `is_default` and `is_cancel` set (a single-button
/// "OK" alert) lands in the default slot here — `show_message_dialog`
/// also binds it to Return, not Escape, when assigning the key
/// equivalent (see that method's doc comment), so which bucket this
/// function sorts it into only affects visual position, not behaviour.
fn native_button_order(buttons: &[MessageDialogButton]) -> Vec<usize> {
    let mut default_idx = None;
    let mut cancel_idx = None;
    let mut rest = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        if b.is_default && default_idx.is_none() {
            default_idx = Some(i);
        } else if b.is_cancel && cancel_idx.is_none() {
            cancel_idx = Some(i);
        } else {
            rest.push(i);
        }
    }
    let mut order = Vec::with_capacity(buttons.len());
    order.extend(default_idx);
    order.extend(cancel_idx);
    order.extend(rest);
    order
}

/// Pick the `NSButton::setKeyEquivalent` string for `button`, or `None`
/// to leave `NSAlert`'s own default untouched (quadraui#936).
///
/// `setKeyEquivalent` only ever holds one string, so a button that is
/// both `is_default` and `is_cancel` (a single-button "OK" alert — the
/// most common message-dialog shape in this codebase) can only be bound
/// to one of Return/Escape here. `is_default` wins: `NSAlert`'s own
/// native default (its docs: "By default, the first button has a key
/// equivalent of Return...") already gives such a button Return for
/// free when `setKeyEquivalent` is never called, and both sibling
/// backends bind *both* keys to this button —
/// `gtk::services::hig_button_order`'s doc points `set_cancel_button`
/// *and* `set_default_button` at it, and
/// `win::services::assign_button_ids` gives it `TASKDIALOG_IDCANCEL`
/// *and* looks it up as `pszDefaultButton` — so Return is the one to
/// keep when only one of the two can survive on this platform.
fn key_equivalent_for_button(button: &MessageDialogButton) -> Option<&'static str> {
    if button.is_default {
        Some("\r")
    } else if button.is_cancel {
        Some("\u{1b}")
    } else {
        None
    }
}

/// Map [`DialogSeverity`] onto `NSAlertStyle` (quadraui#936) — shares
/// `win::services::win_show_message_dialog`'s `icon` match's "`Question`
/// has no dedicated native icon" gap: `NSAlertStyle` (like
/// `TaskDialogIndirect`'s icon set) has no question-mark style, so
/// `Question` degrades to a neutral style, matching [`DialogSeverity`]'s
/// own "`None` = neutral" doc. Unlike the Windows arm, though,
/// `NSAlertStyle` also has no dedicated "no icon" state the way
/// `PCWSTR::null()` gives Windows one, so `Info` is folded into the same
/// `Informational` bucket as `Question`/`None` here rather than getting
/// its own arm the way `TD_INFORMATION_ICON` does on Windows.
fn severity_to_alert_style(severity: Option<DialogSeverity>) -> NSAlertStyle {
    match severity {
        Some(DialogSeverity::Error) => NSAlertStyle::Critical,
        Some(DialogSeverity::Warning) => NSAlertStyle::Warning,
        Some(DialogSeverity::Info) | Some(DialogSeverity::Question) | None => {
            NSAlertStyle::Informational
        }
    }
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

    // ── system_theme_from_mac_appearance / unit_to_u8 (quadraui#952) ────

    #[test]
    fn dark_aqua_appearance_reports_dark() {
        let theme = system_theme_from_mac_appearance("NSAppearanceNameDarkAqua", false, None);
        assert!(theme.dark);
        assert!(!theme.high_contrast);
        assert_eq!(theme.accent, None);
    }

    #[test]
    fn aqua_appearance_reports_light() {
        let theme = system_theme_from_mac_appearance("NSAppearanceNameAqua", false, None);
        assert!(!theme.dark);
    }

    #[test]
    fn high_contrast_flag_is_passed_through_not_rederived_from_name() {
        // The appearance name alone already implies high contrast, but
        // `high_contrast` is a caller-supplied signal
        // (`NSWorkspace::accessibilityDisplayShouldIncreaseContrast`),
        // not derived from the name — this pins that the function trusts
        // its caller's flag rather than re-deriving it from the name
        // string.
        let theme = system_theme_from_mac_appearance(
            "NSAppearanceNameAccessibilityHighContrastDarkAqua",
            true,
            None,
        );
        assert!(theme.dark);
        assert!(theme.high_contrast);
    }

    #[test]
    fn accent_color_passes_through_unchanged() {
        let accent = Some(Color::rgb(0, 122, 255));
        let theme = system_theme_from_mac_appearance("NSAppearanceNameAqua", false, accent);
        assert_eq!(theme.accent, accent);
    }

    #[test]
    fn unit_to_u8_maps_the_full_range() {
        assert_eq!(unit_to_u8(0.0), 0);
        assert_eq!(unit_to_u8(1.0), 255);
        assert_eq!(unit_to_u8(0.5), 128);
    }

    #[test]
    fn unit_to_u8_clamps_out_of_range_input() {
        assert_eq!(unit_to_u8(-1.0), 0);
        assert_eq!(unit_to_u8(2.0), 255);
    }

    fn msg_btn(id: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
        MessageDialogButton {
            id: crate::types::WidgetId::new(id),
            label: id.to_string(),
            is_default,
            is_cancel,
        }
    }

    #[test]
    fn native_button_order_puts_default_first_and_cancel_second() {
        let buttons = [
            msg_btn("middle", false, false),
            msg_btn("cancel", false, true),
            msg_btn("default", true, false),
        ];
        assert_eq!(native_button_order(&buttons), vec![2, 1, 0]);
    }

    #[test]
    fn native_button_order_preserves_relative_order_of_the_rest() {
        let buttons = [
            msg_btn("a", false, false),
            msg_btn("b", false, false),
            msg_btn("c", false, false),
        ];
        assert_eq!(native_button_order(&buttons), vec![0, 1, 2]);
    }

    #[test]
    fn native_button_order_single_button_both_default_and_cancel() {
        // A single "OK" button often carries both flags. It should still
        // appear exactly once (in the default slot), not twice.
        let buttons = [msg_btn("ok", true, true)];
        assert_eq!(native_button_order(&buttons), vec![0]);
    }

    #[test]
    fn native_button_order_no_default_or_cancel_keeps_declared_order() {
        let buttons = [msg_btn("a", false, false), msg_btn("b", false, false)];
        assert_eq!(native_button_order(&buttons), vec![0, 1]);
    }

    #[test]
    fn key_equivalent_for_button_default_only_binds_return() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("ok", true, false)),
            Some("\r")
        );
    }

    #[test]
    fn key_equivalent_for_button_cancel_only_binds_escape() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("cancel", false, true)),
            Some("\u{1b}")
        );
    }

    #[test]
    fn key_equivalent_for_button_neither_binds_nothing() {
        assert_eq!(
            key_equivalent_for_button(&msg_btn("middle", false, false)),
            None
        );
    }

    #[test]
    fn key_equivalent_for_button_both_default_and_cancel_binds_return_not_escape() {
        // Regression test (quadraui#936 review): a single "OK" button
        // that carries both flags — the most common message-dialog
        // shape — MUST get Return, not just Escape, or pressing Enter
        // does nothing and the dialog is keyboard-unresponsive on its
        // most common shape.
        assert_eq!(
            key_equivalent_for_button(&msg_btn("ok", true, true)),
            Some("\r")
        );
    }

    #[test]
    fn severity_to_alert_style_maps_error_and_warning_distinctly() {
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Error)),
            NSAlertStyle::Critical
        );
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Warning)),
            NSAlertStyle::Warning
        );
    }

    #[test]
    fn severity_to_alert_style_defaults_question_and_none_to_informational() {
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Info)),
            NSAlertStyle::Informational
        );
        assert_eq!(
            severity_to_alert_style(Some(DialogSeverity::Question)),
            NSAlertStyle::Informational
        );
        assert_eq!(severity_to_alert_style(None), NSAlertStyle::Informational);
    }

    /// RED-verify companion (quadraui#936's acceptance bar): off the main
    /// thread — which is exactly where Rust's default per-test-fn thread
    /// puts this test — `show_message_dialog` must degrade to `None`
    /// rather than panic, the same contract the doc comment on
    /// `show_message_dialog` describes. Before this issue's fix the
    /// method was an unconditional `None` stub, so this assertion also
    /// held then; what changed is that it now holds for the *documented
    /// reason* (no `MainThreadMarker` here) rather than "not implemented
    /// yet".
    #[test]
    fn show_message_dialog_off_main_thread_returns_none_not_panic() {
        let svc = MacPlatformServices::new();
        let opts = MessageDialogOptions {
            title: "Title".to_string(),
            body: "Body".to_string(),
            buttons: vec![msg_btn("ok", true, true)],
            severity: None,
        };
        assert!(svc.show_message_dialog(opts).is_none());
    }
}
