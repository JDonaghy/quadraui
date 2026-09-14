//! Default `PlatformServices` impl for the TUI backend.
//!
//! Clipboard writes go out on **three** legs: `arboard` (local desktop
//! clipboard), OSC 52 (terminal clipboard via escape sequence), and a
//! native command-line tool (`wl-copy` / `xclip` / `xsel`). OSC 52
//! covers SSH and tmux, where arboard cannot reach the host clipboard.
//! The native-tool leg covers the opposite gap (#398): a local X11
//! session running *inside* an outer tmux, where arboard can fail to
//! own the X `CLIPBOARD` selection and OSC 52 is dropped unless both
//! tmux and the outer terminal are configured to pass it through.
//! Shelling out to the same tool `xclip -o` would use to read the
//! selection sidesteps both failure modes. All three legs are
//! best-effort and run independently — a leg that fails (tool absent,
//! no display, no tty) is silently skipped.
//!
//! ### tmux
//!
//! Inside tmux a bare OSC 52 sequence only reaches the outer terminal
//! when `set -g set-clipboard on` is configured (with `external`/`off`
//! tmux drops or swallows the application's sequence). To cover the
//! other common config, when `$TMUX` is set we *also* emit a copy
//! wrapped in tmux's DCS passthrough (`ESC P tmux ; … ESC \`), which
//! tmux forwards verbatim to the outer terminal when `allow-passthrough
//! on` is set. Emitting both is harmless: each config consumes the form
//! it understands and ignores the other.
//!
//! **User-facing companion: [`docs/CLIPBOARD.md`].** It carries the tmux
//! `~/.tmux.conf` snippets, the `set-clipboard external` trap, the
//! Shift-drag mouse-selection caveat, the OSC 52 payload cap, and a
//! troubleshooting order for "the status bar said `Copied:` but nothing
//! was copied" (#331). Point bug reports there rather than at this
//! module comment — none of it is diagnosable from inside the process.
//!
//! [`docs/CLIPBOARD.md`]: https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/CLIPBOARD.md
//!
//! Other services (notifications, URL open) remain no-op stubs — apps
//! that need them supply their own `PlatformServices` or call platform
//! APIs directly.
//!
//! ## Dialogs (issue #965)
//!
//! `show_file_open_dialog`, `show_file_save_dialog`, and
//! `show_message_dialog` used to return `None` unconditionally — a
//! terminal has no native file picker or alert facility, and nothing in
//! this crate drove an in-canvas replacement's show → block → return-a-
//! choice contract. All three now drive a real in-canvas controller
//! ([`crate::compose::FilePickerController`] /
//! [`crate::compose::MessageDialogController`]) through a **nested
//! draw-and-read loop**: draw one frame, block for the next input event,
//! feed it to the controller, repeat until it resolves. This is the TUI
//! counterpart of `GtkPlatformServices::pump_until_ready`
//! (`crate::gtk::services`) — GTK nests `glib::MainContext::iteration`,
//! TUI nests its own crossterm poll/read/redraw cycle, since a terminal
//! has no separate native event loop to pump; blocking synchronously on
//! `crossterm::event::read()` inside `&self` is the direct equivalent.
//!
//! [`Self::run_nested_dialog_loop`] needs somewhere to paint — a shared
//! handle onto the live [`ratatui::Terminal`], wired once by
//! [`super::run::run_with`] via [`Self::set_dialog_surface`] (mirroring
//! `GtkPlatformServices::set_window`'s "`None` until wired" shape) —
//! and something to poll for events. The latter is **not** wired through
//! the same mechanism: production always reads real crossterm input
//! directly (no plumbing needed — `crossterm::event::poll`/`read` are
//! free functions reachable from anywhere in this crate), while a test
//! seeds [`Self::scripted_dialog_events`] via
//! [`Self::queue_dialog_events`] (also reachable through
//! [`crate::tui::testing::TuiDriver::queue_dialog_events`] for tests
//! outside this crate) so the whole round trip — real
//! `FilePickerController`/`MessageDialogController` state machine, real
//! `TuiBackend` paint — is exercised with **no** live terminal, purely
//! against `ratatui::backend::TestBackend` (see this module's own
//! `dialog_tests` for the PlatformServices-level round trips this makes
//! possible: file-open confirms a path, file-open cancels, file-save
//! confirms a typed name, file-save seeds an initial filename,
//! message-dialog resolves a button, message-dialog Escape resolves the
//! cancel button, and an exhausted scripted queue still resolves rather
//! than hanging).
//!
//! The one thing this nested loop cannot do without help is share the
//! *live* runner's `Terminal` instance — using a second, independently
//! constructed `Terminal` for the dialog would desync ratatui's internal
//! diff cache from the physical screen the moment control returns to the
//! live runner (cells whose post-dialog content happens to match
//! pre-dialog content would never be repainted, leaving stale dialog
//! pixels on screen indefinitely). `set_dialog_surface` exists
//! specifically to avoid that: the live runner and the nested dialog
//! loop draw through the *same* `Rc<RefCell<Terminal<..>>>`, so the
//! diff cache always reflects exactly what was last written, dialog
//! frames included.
//!
//! `show_folder_open_dialog` is deliberately **not** touched here —
//! issue #965 scopes to the three dialogs above; `FolderPickerController`
//! stays an app-driven compose controller with no `PlatformServices`
//! integration of its own (see that controller's module doc for why, and
//! `crate::compose::file_picker`'s module doc for why the analogous file
//! picker doesn't repeat one of its keybinding trade-offs).
//!
//! ## `shell.*` parity (issue #956) — better than the rest of this list
//!
//! Three of #956's four methods are genuinely implemented here, not
//! no-op stubs: `beep` (BEL, a terminal's only notification channel —
//! full support, arguably more honest than any other backend's), and
//! `move_to_trash` (delegates to [`crate::desktop::move_to_trash`] — the
//! cross-platform `trash` crate needs only a filesystem, not a live
//! desktop session, so TUI gets it too). `open_path` shells out to
//! `xdg-open`/`open` directly on Unix (best-effort — see
//! [`TuiPlatformServices::open_path`]'s own doc). Only
//! `reveal_in_file_manager` stays `Err(BackendError::Unsupported)`: a
//! terminal genuinely has no file-manager window to reveal anything in.
//!
//! ## Displays (issue #959)
//!
//! `displays` is also genuinely implemented, not a stub: one
//! [`crate::backend::Display`] whose bounds are `crossterm::terminal::size()`
//! — the honest "one display = the terminal cell grid" degrade, not
//! `Unsupported`. `cursor_screen_point` stays `Unsupported`: a terminal
//! has no synchronous cursor-position query at all. See
//! [`TuiPlatformServices::displays`]/
//! [`TuiPlatformServices::cursor_screen_point`]'s own docs.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::backend::{
    Backend, BackendError, Clipboard, Display, FileDialogOptions, MessageDialogChoice,
    MessageDialogOptions, Notification, PlatformServices, ServiceResult, SystemTheme,
};
use crate::compose::{
    FilePickerController, FilePickerEvent, FilePickerMode, MessageDialogController,
    MessageDialogEvent,
};
use crate::event::{Point, Rect, UiEvent};
use crate::tui::backend::TuiBackend;
use crate::{Key, Modifiers, NamedKey};

// ── Nested dialog loop (issue #965) ─────────────────────────────────────────────

/// A paintable, sizeable surface a nested dialog loop can draw into —
/// implemented for any `ratatui::Terminal<B>`. Exists so
/// [`TuiPlatformServices`]'s dialog methods work identically against the
/// live runner's real `Terminal<CrosstermBackend<Stdout>>`-wrapping
/// backend and a test's in-memory `Terminal<TestBackend>`, without this
/// module naming either concrete ratatui backend type. See the module
/// doc's "Dialogs (issue #965)" section.
pub(crate) trait DialogSurface {
    /// Paint one frame, handing the raw `ratatui::Frame` to `paint`.
    /// Swallows a paint error (matching `TuiClipboard`'s established
    /// "best-effort, no propagation path" posture for `&self` methods)
    /// — [`TuiPlatformServices::run_nested_dialog_loop`] instead treats a
    /// `None` from [`Self::dialog_size`] as the signal to bail, since
    /// that failure mode (no controlling terminal) is the one this crate
    /// already has an honest-degrade story for (see
    /// `TuiPlatformServices::displays`'s doc).
    fn draw_dialog_frame(&mut self, paint: &mut dyn FnMut(&mut ratatui::Frame<'_>));
    /// The surface's current cell size, or `None` if it can't be
    /// determined.
    fn dialog_size(&self) -> Option<ratatui::layout::Size>;
}

impl<B: ratatui::backend::Backend> DialogSurface for ratatui::Terminal<B> {
    fn draw_dialog_frame(&mut self, paint: &mut dyn FnMut(&mut ratatui::Frame<'_>)) {
        let _ = self.draw(|frame| paint(frame));
    }

    fn dialog_size(&self) -> Option<ratatui::layout::Size> {
        ratatui::Terminal::size(self).ok()
    }
}

/// Outcome of one step through [`TuiPlatformServices::run_nested_dialog_loop`].
enum NestedDialogStep<T> {
    /// The dialog resolved to a real value — stop looping and return it.
    Resolved(T),
    /// The dialog was dismissed with nothing to return — stop looping,
    /// report `None`.
    Cancelled,
    /// Internal state changed (or the event was irrelevant); keep going.
    Continue,
}

// ── OSC 52 support ────────────────────────────────────────────────────────────

/// Base64-encode `data` using the standard alphabet (no line wrapping).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as u32
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        if i + 1 < data.len() {
            out.push(CHARS[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(CHARS[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

/// Build the raw OSC 52 clipboard-write sequence for `text`:
/// `ESC ] 52 ; c ; <base64(text)> BEL` (ESC ] = OSC introducer; BEL
/// terminates).
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Wrap a terminal escape sequence in tmux's DCS passthrough so tmux
/// forwards it verbatim to the outer terminal: `ESC P tmux ; <seq> ESC \`,
/// with every inner `ESC` doubled (tmux's escaping rule). Requires
/// `allow-passthrough on` in the tmux config.
fn tmux_passthrough_wrap(seq: &str) -> String {
    let escaped = seq.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{}\x1b\\", escaped)
}

/// Emit an OSC 52 clipboard-write sequence for `text` to `writer`,
/// additionally emitting a tmux DCS-passthrough copy when `in_tmux`.
///
/// Terminal requirements:
/// - Most modern terminals (kitty, WezTerm, iTerm2, alacritty, xterm)
///   support OSC 52 by default.
/// - **tmux**: the bare sequence needs `set -g set-clipboard on`; the
///   passthrough copy (emitted when `in_tmux`) needs `allow-passthrough
///   on`. Emitting both covers either config.
/// - **screen**: not widely supported; falls back silently.
/// - **SSH**: works when the remote terminal supports OSC 52 passthrough
///   (most do).
///
/// **Payload size limits**: Many terminals cap the OSC 52 base64 payload
/// at roughly 74–100 KB of encoded data (≈ 55–75 KB of raw text) and
/// silently drop or truncate sequences that exceed it. Very large
/// selections may not reach the clipboard; no feedback is given when
/// this occurs.
pub(crate) fn emit_osc52_with(text: &str, in_tmux: bool, writer: &mut dyn std::io::Write) {
    let seq = osc52_sequence(text);
    let _ = writer.write_all(seq.as_bytes());
    if in_tmux {
        let _ = writer.write_all(tmux_passthrough_wrap(&seq).as_bytes());
    }
    let _ = writer.flush();
}

/// Emit OSC 52 for `text`, auto-detecting tmux from `$TMUX`. Thin
/// wrapper over [`emit_osc52_with`] used by production code; tests call
/// `emit_osc52_with` with an explicit `in_tmux` to stay independent of
/// the ambient environment.
pub(crate) fn emit_osc52_to(text: &str, writer: &mut dyn std::io::Write) {
    emit_osc52_with(text, std::env::var_os("TMUX").is_some(), writer);
}

// ── Native clipboard tool fallback (#398) ───────────────────────────────────────

/// Ordered list of native clipboard-tool invocations to try, as
/// `(program, args)` pairs.
///
/// Wayland-first when `wayland` is true (typically driven by
/// `$WAYLAND_DISPLAY`), else X11-first — but **all three** candidates
/// are always present regardless of order, so a mislabelled session
/// (e.g. a Wayland compositor that doesn't set `$WAYLAND_DISPLAY`)
/// still finds a working tool.
#[cfg_attr(not(unix), allow(dead_code))]
fn native_clipboard_candidates(wayland: bool) -> [(&'static str, &'static [&'static str]); 3] {
    let wl_copy: (&'static str, &'static [&'static str]) = ("wl-copy", &[]);
    let xclip: (&'static str, &'static [&'static str]) = ("xclip", &["-selection", "clipboard"]);
    let xsel: (&'static str, &'static [&'static str]) = ("xsel", &["--clipboard", "--input"]);
    if wayland {
        [wl_copy, xclip, xsel]
    } else {
        [xclip, xsel, wl_copy]
    }
}

/// Best-effort clipboard write via a native command-line tool
/// (`wl-copy` / `xclip` / `xsel`) — the third leg of [`TuiClipboard::write_text`].
///
/// This covers the setup where neither arboard nor OSC 52 reliably
/// reach the real system clipboard: a local X11 (or Wayland) session
/// running inside an outer tmux (#398). The native tool owns the
/// selection exactly the way `xclip -o` (or a paste elsewhere on the
/// desktop) reads it back.
///
/// Tries each candidate in [`native_clipboard_candidates`] order,
/// stopping at the first one that spawns successfully. These tools
/// daemonize after reading stdin to EOF and keep serving the
/// selection in the background; closing our end of the pipe (by
/// dropping `stdin`) and then `wait()`-ing only reaps the short-lived
/// foreground process, it does not wait for the daemonized copy to
/// exit.
///
/// Silent on failure: over SSH these tools are typically absent (or
/// target the wrong display) and every candidate's `spawn()` simply
/// errors — OSC 52 already carries the copy in that case, so this
/// leg is a no-op regression-free fallback, not the primary path.
#[cfg_attr(not(unix), allow(dead_code))]
fn write_clipboard_via_native_tool(text: &str) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    for (program, args) in native_clipboard_candidates(wayland) {
        let mut child = match std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue, // tool not installed — try the next candidate
        };
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
            // `stdin` drops here, closing our end (EOF) — the signal
            // these tools wait for before forking to serve the
            // selection in the background.
        }
        match child.wait() {
            Ok(status) if status.success() => return,
            // Installed but failed at runtime (e.g. `$DISPLAY`/
            // `$WAYLAND_DISPLAY` unset or stale) — try the next
            // candidate instead of silently giving up.
            _ => continue,
        }
    }
}

// ── TuiClipboard ──────────────────────────────────────────────────────────────

/// System clipboard that writes via **three** independent legs: arboard,
/// OSC 52, and (Unix only) a native command-line tool fallback. See the
/// module doc comment for why all three exist.
///
/// The arboard handle is kept alive for the process lifetime so Linux
/// clipboard serving threads persist (dropping the handle immediately
/// would clear clipboard contents on X11/Wayland).
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
        // 1. arboard — local desktop clipboard (works when not over SSH).
        if let Some(cb) = self.inner.borrow_mut().as_mut() {
            let _ = cb.set_text(text);
        }
        // 2. OSC 52 — terminal clipboard escape (works over SSH / tmux).
        //
        // Write to stdout AND to /dev/tty (Unix-only) for reliability:
        //  - stdout: the normal TUI output stream; works in most setups.
        //  - /dev/tty: the controlling terminal device, always reachable
        //    even when stdout is redirected (e.g. run via a wrapper script
        //    that pipes stdout). The two writes are harmless duplicates for
        //    normal use where stdout already is the tty.
        emit_osc52_to(text, &mut std::io::stdout());
        #[cfg(unix)]
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            emit_osc52_to(text, &mut tty);
        }
        // 3. Native clipboard tool (wl-copy / xclip / xsel) — best-effort
        //    fallback for local X11-inside-tmux setups where neither leg
        //    above reaches the real system clipboard (#398). Offloaded to
        //    a detached thread: these tools daemonize after reading
        //    stdin, so spawning them synchronously here would risk
        //    stalling the UI loop on a slow fork/exec.
        #[cfg(unix)]
        {
            let owned = text.to_string();
            let _ = std::thread::spawn(move || write_clipboard_via_native_tool(&owned));
        }
    }

    /// Clear the **local** desktop clipboard via the same `arboard`
    /// handle [`Self::read_text`] already reads through (issue #954).
    ///
    /// Deliberately not a three-leg operation like [`Self::write_text`]:
    /// OSC 52 has no "clear" form (only "set to this base64 payload") and
    /// the native-tool fallback (`wl-copy`/`xclip`/`xsel`) only knows how
    /// to *serve* new content, not command a remote/outer clipboard to go
    /// empty. So this is honest about its reach — same "local only,
    /// nothing over SSH" ceiling [`Self::read_text`] already has — rather
    /// than faking a clear by writing an empty string through the other
    /// two legs, which would leave `ClipboardFormat::Text` reporting
    /// present-but-empty instead of genuinely absent.
    ///
    /// `read_image`/`write_image`/`write_html`/`read_file_list` stay at
    /// the trait's `Unsupported` default on `TuiClipboard` — OSC 52 (the
    /// one channel that reaches a remote/SSH session) has no image or
    /// HTML form, so this backend's clipboard is text-only by design, not
    /// by omission (see [`crate::backend::Clipboard::read_image`]'s doc).
    fn clear(&self) -> ServiceResult<()> {
        let mut inner = self.inner.borrow_mut();
        let cb = inner.as_mut().ok_or(BackendError::PlatformFailure {
            context: "arboard::Clipboard::new (no local clipboard available)".to_string(),
        })?;
        cb.clear().map_err(|e| BackendError::PlatformFailure {
            context: format!("arboard::clear: {e}"),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_three_bytes_no_padding() {
        // "Man" → "TWFu"
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn base64_two_bytes_one_pad() {
        // "Ma" → "TWE="
        assert_eq!(base64_encode(b"Ma"), "TWE=");
    }

    #[test]
    fn base64_one_byte_two_pads() {
        // "M" → "TQ=="
        assert_eq!(base64_encode(b"M"), "TQ==");
    }

    #[test]
    fn base64_hello() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn osc52_sequence_correct() {
        let mut out = Vec::new();
        emit_osc52_with("hello", false, &mut out);
        // ESC ] 52 ; c ; aGVsbG8= BEL
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_empty_text() {
        let mut out = Vec::new();
        emit_osc52_with("", false, &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;\x07");
    }

    #[test]
    fn osc52_tmux_emits_raw_then_passthrough() {
        let mut out = Vec::new();
        emit_osc52_with("hello", true, &mut out);
        // Raw sequence first, then the DCS-passthrough copy with the
        // inner ESC doubled and an `ESC \` terminator.
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b]52;c;aGVsbG8=\x07\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\"
        );
    }

    #[test]
    fn tmux_passthrough_doubles_every_esc() {
        // A two-ESC payload must come back with four ESCs, wrapped.
        let wrapped = tmux_passthrough_wrap("\x1bA\x1bB");
        assert_eq!(wrapped, "\x1bPtmux;\x1b\x1bA\x1b\x1bB\x1b\\");
    }

    #[test]
    fn native_candidates_wayland_first_when_wayland() {
        let candidates = native_clipboard_candidates(true);
        assert_eq!(candidates[0].0, "wl-copy");
        assert_eq!(candidates[1].0, "xclip");
        assert_eq!(candidates[2].0, "xsel");
    }

    #[test]
    fn native_candidates_x11_first_when_not_wayland() {
        let candidates = native_clipboard_candidates(false);
        assert_eq!(candidates[0].0, "xclip");
        assert_eq!(candidates[1].0, "xsel");
        assert_eq!(candidates[2].0, "wl-copy");
    }

    #[test]
    fn native_candidates_always_list_all_three_tools_regardless_of_order() {
        // A mislabelled session (e.g. Wayland compositor that doesn't set
        // $WAYLAND_DISPLAY) should still find a working tool — so both
        // orderings must contain the same three programs.
        for wayland in [true, false] {
            let names: Vec<&str> = native_clipboard_candidates(wayland)
                .iter()
                .map(|(program, _)| *program)
                .collect();
            assert!(names.contains(&"wl-copy"), "{names:?}");
            assert!(names.contains(&"xclip"), "{names:?}");
            assert!(names.contains(&"xsel"), "{names:?}");
        }
    }

    #[test]
    fn native_candidate_args_target_the_system_clipboard_selection() {
        // xclip/xsel default to the PRIMARY selection unless told
        // otherwise — verify each candidate explicitly requests the
        // CLIPBOARD selection that `xclip -o -selection clipboard` reads.
        let candidates = native_clipboard_candidates(false);
        let xclip = candidates.iter().find(|(p, _)| *p == "xclip").unwrap();
        assert_eq!(xclip.1, &["-selection", "clipboard"]);
        let xsel = candidates.iter().find(|(p, _)| *p == "xsel").unwrap();
        assert_eq!(xsel.1, &["--clipboard", "--input"]);
    }

    // ── Clipboard image/html/file-list/clear degrade story (#954) ──────

    /// TUI is text-only by design (OSC 52 has no image/HTML form), so
    /// `read_image`/`write_image`/`write_html`/`read_file_list` must stay
    /// on the trait's `Unsupported` default — this pins that degrade
    /// story so a future change to `TuiClipboard` can't silently start
    /// (or stop) overriding one of them.
    #[test]
    fn image_html_and_file_list_are_unsupported_by_design() {
        let cb = TuiClipboard::new();
        assert_eq!(cb.read_image(), Err(BackendError::Unsupported));
        assert_eq!(
            cb.write_image(&crate::backend::RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![0, 0, 0, 255],
            }),
            Err(BackendError::Unsupported)
        );
        assert_eq!(
            cb.write_html("<b>hi</b>", "hi"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(cb.read_file_list(), Err(BackendError::Unsupported));
    }

    /// `clear()` real round trip through the local `arboard` leg — skips
    /// gracefully (rather than failing) when this environment has no
    /// local desktop clipboard at all (headless CI, pure SSH session),
    /// since that's a real gap this test isn't trying to paper over.
    #[allow(clippy::print_stderr)]
    #[test]
    fn clear_empties_the_local_clipboard() {
        let cb = TuiClipboard::new();
        cb.write_text("some text #954 (tui clear test)");
        if cb.read_text().is_none() {
            eprintln!("skipping: no local desktop clipboard in this environment");
            return;
        }
        cb.clear()
            .expect("clear should succeed once write_text/read_text already proved a local clipboard is live");
        assert_eq!(cb.read_text(), None);
    }
}

/// Default `PlatformServices` impl for the TUI backend.
pub struct TuiPlatformServices {
    clipboard: TuiClipboard,
    /// Shared handle onto the live paint target for the nested dialog
    /// loop (issue #965) — see the module doc's "Dialogs" section.
    /// `None` until [`Self::set_dialog_surface`] is called (and in unit
    /// tests that never call it), mirroring
    /// `GtkPlatformServices::window`'s "`None` until wired" shape:
    /// every dialog method degrades to `None` rather than panicking.
    dialog_surface: RefCell<Option<Rc<RefCell<dyn DialogSurface>>>>,
    /// Scripted input for the nested dialog loop. `None` (the production
    /// default) means "read real crossterm input"; `Some` — even an
    /// empty queue — means "test mode, never touch the real terminal".
    /// Set via [`Self::queue_dialog_events`].
    scripted_dialog_events: RefCell<Option<VecDeque<UiEvent>>>,
}

impl TuiPlatformServices {
    pub fn new() -> Self {
        Self {
            clipboard: TuiClipboard::new(),
            dialog_surface: RefCell::new(None),
            scripted_dialog_events: RefCell::new(None),
        }
    }

    /// Wire the shared paint target the nested dialog loop draws
    /// through. Called once by [`super::run::run_with`] right after the
    /// live `Terminal` is constructed, mirroring
    /// `GtkPlatformServices::set_window`. See the module doc for why
    /// this must be the *same* `Terminal` instance the live runner
    /// itself draws through, not an independently constructed one.
    pub(crate) fn set_dialog_surface(&self, surface: Rc<RefCell<dyn DialogSurface>>) {
        *self.dialog_surface.borrow_mut() = Some(surface);
    }

    /// Seed the nested dialog loop's event source. Must be called
    /// *before* the event that triggers the dialog-opening call is
    /// dispatched — the loop drains this queue synchronously, with no
    /// way to be fed more input mid-call. See the module doc's
    /// "Dialogs" section and this module's `dialog_tests`.
    ///
    /// Not `#[cfg(test)]`: [`crate::tui::testing::TuiDriver::queue_dialog_events`]
    /// forwards to this from an ordinary (non-`cfg(test)`) build of this
    /// crate — `TuiDriver` is `pub`, used by *downstream* crates' own
    /// `cargo test` runs, which never set `cfg(test)` on quadraui itself
    /// (only on their own crate). A `#[cfg(test)]` gate here would make
    /// `TuiDriver::queue_dialog_events` silently uncompilable the moment
    /// it tried to call this.
    pub(crate) fn queue_dialog_events(&self, events: impl IntoIterator<Item = UiEvent>) {
        self.scripted_dialog_events
            .borrow_mut()
            .get_or_insert_with(VecDeque::new)
            .extend(events);
    }

    /// Get the next batch of `UiEvent`s for a nested dialog loop —
    /// draining [`Self::scripted_dialog_events`] in test mode, or
    /// blocking (in short, re-checked slices — never truly forever) on
    /// real crossterm input in production, the same poll/read shape
    /// [`crate::tui::backend::TuiBackend::wait_events`] uses.
    fn next_dialog_events(&self) -> Vec<UiEvent> {
        {
            let mut scripted = self.scripted_dialog_events.borrow_mut();
            if let Some(queue) = scripted.as_mut() {
                return match queue.pop_front() {
                    Some(ev) => vec![ev],
                    // Exhausted without the controller resolving — this
                    // only happens on a malformed test script (missing
                    // its own terminating key). Synthesize Escape so the
                    // loop can never hang a test suite instead of
                    // spinning on an empty queue forever.
                    None => vec![UiEvent::KeyPressed {
                        key: Key::Named(NamedKey::Escape),
                        modifiers: Modifiers::default(),
                        repeat: false,
                    }],
                };
            }
        }
        loop {
            match ratatui::crossterm::event::poll(std::time::Duration::from_millis(250)) {
                Ok(true) => {
                    return match ratatui::crossterm::event::read() {
                        Ok(ev) => super::events::crossterm_to_uievents(ev),
                        Err(_) => Vec::new(),
                    };
                }
                // No input within this slice — loop back and poll again
                // rather than blocking indefinitely in one call.
                Ok(false) => continue,
                Err(_) => return Vec::new(),
            }
        }
    }

    /// Draw-and-read nested loop shared by [`Self::show_file_open_dialog`],
    /// [`Self::show_file_save_dialog`], and [`Self::show_message_dialog`]
    /// — the TUI counterpart of `GtkPlatformServices::pump_until_ready`.
    /// See the module doc's "Dialogs (issue #965)" section for the full
    /// design.
    ///
    /// Returns `None` immediately if no [`Self::dialog_surface`] is
    /// wired, or once it can no longer be sized (no controlling
    /// terminal). `draw` paints one frame given the current viewport;
    /// `handle_event` processes one input event against that same
    /// viewport and reports whether the loop should keep going.
    fn run_nested_dialog_loop<T>(
        &self,
        mut draw: impl FnMut(&mut dyn Backend, Rect),
        mut handle_event: impl FnMut(&UiEvent, &dyn Backend, Rect) -> NestedDialogStep<T>,
    ) -> Option<T> {
        let surface = self.dialog_surface.borrow().clone()?;
        let mut scratch = TuiBackend::new();
        loop {
            let size = surface.borrow().dialog_size()?;
            let viewport = Rect::new(0.0, 0.0, size.width as f32, size.height as f32);
            scratch.begin_frame(crate::Viewport::new(
                size.width as f32,
                size.height as f32,
                1.0,
            ));
            surface.borrow_mut().draw_dialog_frame(&mut |frame| {
                scratch.enter_frame_scope(frame, |b| draw(b, viewport));
            });
            scratch.end_frame();

            for event in self.next_dialog_events() {
                match handle_event(&event, &scratch, viewport) {
                    NestedDialogStep::Resolved(v) => return Some(v),
                    NestedDialogStep::Cancelled => return None,
                    NestedDialogStep::Continue => {}
                }
            }
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

    /// Nested draw-and-read loop over [`crate::compose::FilePickerController`]
    /// in [`FilePickerMode::Open`] (issue #965) — see the module doc's
    /// "Dialogs" section. `opts.initial_dir` seeds the browsing root
    /// (falling back to the process's current directory);
    /// `opts.filters` restricts which files are listed.
    /// `opts.title`/`opts.initial_filename` don't apply to an *open*
    /// dialog and are ignored, matching
    /// [`crate::backend::FileDialogOptions`]'s own doc ("ignored by
    /// `show_file_open_dialog`").
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let root = opts
            .initial_dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let picker = RefCell::new(FilePickerController::new(
            FilePickerMode::Open,
            root,
            opts.filters,
        ));
        self.run_nested_dialog_loop(
            |backend, viewport| picker.borrow().render(viewport, backend),
            |event, _backend, viewport| {
                let visible_rows = (viewport.height as usize)
                    .saturating_sub(crate::compose::folder_picker::PALETTE_CHROME_ROWS);
                match picker.borrow_mut().handle(event, visible_rows) {
                    FilePickerEvent::Confirmed { path } => NestedDialogStep::Resolved(path),
                    FilePickerEvent::Cancelled => NestedDialogStep::Cancelled,
                    FilePickerEvent::Consumed | FilePickerEvent::Ignored => {
                        NestedDialogStep::Continue
                    }
                }
            },
        )
    }

    /// Nested draw-and-read loop over [`crate::compose::FilePickerController`]
    /// in [`FilePickerMode::Save`] (issue #965) — see
    /// [`Self::show_file_open_dialog`]'s doc for the shared shape.
    /// `opts.initial_filename` seeds the destination filename (the
    /// picker's query field — see `FilePickerController`'s module doc
    /// for why the two are the same field in Save mode).
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf> {
        let root = opts
            .initial_dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let mut picker = FilePickerController::new(FilePickerMode::Save, root, opts.filters);
        if let Some(name) = opts.initial_filename {
            picker = picker.with_initial_filename(name);
        }
        let picker = RefCell::new(picker);
        self.run_nested_dialog_loop(
            |backend, viewport| picker.borrow().render(viewport, backend),
            |event, _backend, viewport| {
                let visible_rows = (viewport.height as usize)
                    .saturating_sub(crate::compose::folder_picker::PALETTE_CHROME_ROWS);
                match picker.borrow_mut().handle(event, visible_rows) {
                    FilePickerEvent::Confirmed { path } => NestedDialogStep::Resolved(path),
                    FilePickerEvent::Cancelled => NestedDialogStep::Cancelled,
                    FilePickerEvent::Consumed | FilePickerEvent::Ignored => {
                        NestedDialogStep::Continue
                    }
                }
            },
        )
    }

    /// No native directory chooser on TUI — unconditionally `None`, same
    /// as the file-dialog methods above (quadraui#935). Apps should
    /// provide an in-canvas picker instead (see
    /// `BackendCaps::folder_dialogs`'s doc for why this is a distinct
    /// flag from `file_dialogs`).
    fn show_folder_open_dialog(&self, _opts: FileDialogOptions) -> Option<PathBuf> {
        None
    }

    /// Nested draw-and-read loop over
    /// [`crate::compose::MessageDialogController`] (issues #666, #965) —
    /// see the module doc's "Dialogs" section and
    /// [`Self::show_file_open_dialog`]'s doc for the shared shape.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
        let controller = RefCell::new(MessageDialogController::new(opts));
        self.run_nested_dialog_loop(
            |backend, _viewport| controller.borrow().render(backend),
            |event, backend, _viewport| match controller.borrow_mut().handle(event, backend) {
                MessageDialogEvent::Resolved(id) => NestedDialogStep::Resolved(id),
                MessageDialogEvent::Consumed | MessageDialogEvent::Ignored => {
                    NestedDialogStep::Continue
                }
            },
        )
    }

    fn send_notification(&self, _n: Notification) {}

    fn open_url(&self, _url: &str) {}

    /// quadraui#949: TUI has no browser to hand a URL to, and `open_url`
    /// above is an empty no-op body a caller has no way to distinguish
    /// from "it worked" — this is the fix. Skips calling `open_url`
    /// (there is nothing for it to do) and reports the gap directly
    /// instead of falling through to the trait's default `Ok(())`.
    fn open_url_result(&self, _url: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// No file manager window a terminal could reveal anything in —
    /// unconditionally `Err(BackendError::Unsupported)` (issue #956), same
    /// as [`Self::open_url_result`]'s TUI degrade. Explicitly overridden
    /// (rather than left to inherit
    /// [`PlatformServices::reveal_in_file_manager`]'s identical default
    /// body) purely so a reader scanning `TuiPlatformServices` for #956
    /// coverage finds this note instead of wondering why the method is
    /// missing — [`Self::open_path`] and [`Self::move_to_trash`] below are
    /// the two members of this issue's four that TUI implements for real,
    /// [`Self::beep`] is fully native to a terminal, and this one is the
    /// genuine gap: a terminal has no windowed file manager to hand a
    /// selection to.
    fn reveal_in_file_manager(&self, _path: &Path) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// `xdg-open`/`open`, shelled out directly (issue #956) — unlike
    /// [`Self::open_url`]/[`Self::open_url_result`] above, which have no
    /// browser to hand a URL to and stay genuine no-ops, a terminal
    /// running inside a desktop session (the common case — most TUI apps
    /// run in a graphical terminal emulator, not a bare VT) can still
    /// launch the OS's default handler for a *file*. Best-effort: a
    /// successful `spawn()` reports `Ok(())` even though the spawned
    /// `xdg-open`/`open` may itself fail asynchronously with no way for
    /// this call to observe it (same "launch succeeded, outcome unknown"
    /// contract [`crate::tui::services::write_clipboard_via_native_tool`]'s
    /// tool-spawn leg already has). `Err(BackendError::Unsupported)` on a
    /// non-Unix host — Windows Terminal has no equivalent this module
    /// implements yet.
    fn open_path(&self, path: &Path) -> ServiceResult<()> {
        #[cfg(target_os = "macos")]
        const OPENER: &str = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        const OPENER: &str = "xdg-open";

        #[cfg(unix)]
        {
            std::process::Command::new(OPENER)
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| BackendError::PlatformFailure {
                    context: format!("{OPENER}: {e}"),
                })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(BackendError::Unsupported)
        }
    }

    /// [`crate::desktop::move_to_trash`] (issue #956) — genuinely **full**
    /// support on TUI, not a degrade: the `trash` crate needs only a
    /// filesystem, no live desktop/window-server session, so this is the
    /// exact same implementation GTK/macOS/Win-GUI use. See that
    /// function's doc and the module doc's "TUI story" note.
    fn move_to_trash(&self, path: &Path) -> ServiceResult<()> {
        crate::desktop::move_to_trash(path)
    }

    /// BEL (`\x07`), written to both stdout and (Unix) `/dev/tty` for the
    /// same reliability reason [`TuiClipboard::write_text`]'s OSC 52 leg
    /// writes to both (issue #956): stdout may be redirected away from
    /// the terminal by a wrapper script, but `/dev/tty` always reaches
    /// the controlling terminal directly. Arguably the most *honest* of
    /// this issue's four TUI overrides — BEL is a terminal's only
    /// notification channel, so this is full support, not a fallback.
    fn beep(&self) -> ServiceResult<()> {
        use std::io::Write;
        let _ = std::io::stdout().write_all(b"\x07");
        let _ = std::io::stdout().flush();
        #[cfg(unix)]
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            let _ = tty.write_all(b"\x07");
            let _ = tty.flush();
        }
        Ok(())
    }

    /// quadraui#952: the honest TUI degrade — see
    /// `crate::tui::caps::detect_system_theme`'s doc for exactly what
    /// signal this reads (`COLORFGBG`) and why there is no OSC 11 live
    /// probe here yet. `Err(BackendError::Unsupported)` on any terminal
    /// that doesn't set `COLORFGBG` (the majority — xterm, GNOME
    /// Terminal, kitty, WezTerm, iTerm2, Alacritty, Windows Terminal all
    /// leave it unset).
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        super::caps::detect_system_theme().ok_or(BackendError::Unsupported)
    }

    /// A truthful degrade, not `Unsupported` (issue #959, see the crate's
    /// `CLAUDE.md` "TUI story" note for this issue): one [`Display`]
    /// whose `bounds`/`work_area` are both the terminal's cell grid —
    /// `crossterm::terminal::size()`, the same query
    /// [`crate::tui::run::paint_frame`]'s own doc names as reaching the
    /// process's real controlling terminal (`/dev/tty` on Unix) rather
    /// than whatever `Write` sink a backend happens to be constructed
    /// with. `scale` is always `1.0` (a cell has no DPI concept) and
    /// `primary` is always `true` (one grid, trivially the only one).
    ///
    /// `Err(BackendError::PlatformFailure)` when that query itself fails
    /// — no controlling terminal at all (piped stdout, `cargo test`'s
    /// captured output with no pty attached). An honest outcome for a
    /// headless environment that genuinely has no terminal size to
    /// report, not a bug to paper over with a guessed value.
    fn displays(&self) -> ServiceResult<Vec<Display>> {
        let (width, height) =
            ratatui::crossterm::terminal::size().map_err(|e| BackendError::PlatformFailure {
                context: format!("crossterm::terminal::size: {e}"),
            })?;
        let bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
        Ok(vec![Display {
            bounds,
            work_area: bounds,
            scale: 1.0,
            primary: true,
        }])
    }

    /// **Deliberately not overridden** — kept explicit purely so a reader
    /// scanning this file for #959 coverage finds this note instead of
    /// wondering why the method is missing (same posture
    /// [`Self::reveal_in_file_manager`]'s doc explains for an identical
    /// case). A terminal has no synchronous "where is the mouse right
    /// now" query at all — only `UiEvent::MouseMoved`, delivered when
    /// the terminal's mouse-tracking mode is on, and only ever in cell
    /// coordinates relative to this terminal's own grid, not a
    /// cross-display screen position. `Err(BackendError::Unsupported)`,
    /// the trait's own default, is the honest final answer here.
    fn cursor_screen_point(&self) -> ServiceResult<Point> {
        Err(BackendError::Unsupported)
    }

    fn platform_name(&self) -> &'static str {
        "tui"
    }
}

#[cfg(test)]
mod message_dialog_tests {
    use super::*;
    use crate::backend::MessageDialogOptions;

    /// quadraui#965: `show_message_dialog` now drives a real nested
    /// draw-and-read loop over `MessageDialogController` — but that loop
    /// needs somewhere to paint (`set_dialog_surface`), which a bare
    /// `TuiPlatformServices::new()` never wires. This pins the resulting
    /// degrade: `None`, not a panic, mirroring
    /// `GtkPlatformServices::show_message_dialog`'s identical "no window
    /// wired yet" shape. See `dialog_tests` below for the real, wired
    /// round trip this issue's acceptance bar actually asks for.
    #[test]
    fn show_message_dialog_returns_none_when_no_surface_wired() {
        let services = TuiPlatformServices::new();
        let opts = MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "Do you want to save?".to_string(),
            buttons: Vec::new(),
            severity: None,
        };
        assert!(services.show_message_dialog(opts).is_none());
    }

    /// quadraui#935: TUI has no native directory chooser, so
    /// `show_folder_open_dialog` unconditionally returns `None` — same
    /// shape as `show_message_dialog` above, and matching
    /// `BackendCaps::folder_dialogs` being `false` on `TuiBackend`.
    #[test]
    fn show_folder_open_dialog_always_returns_none() {
        let services = TuiPlatformServices::new();
        assert!(services
            .show_folder_open_dialog(FileDialogOptions::default())
            .is_none());
    }

    /// quadraui#949: before `open_url_result` existed, `open_url`'s empty
    /// no-op body was a genuinely undetectable silent failure — a caller
    /// had no way to tell "the browser opened" from "TUI silently
    /// discarded this". `open_url_result` closes that gap by reporting
    /// `Unsupported` explicitly rather than falling through to the
    /// trait's default `Ok(())`.
    #[test]
    fn open_url_result_reports_unsupported_on_tui() {
        let services = TuiPlatformServices::new();
        assert_eq!(
            services.open_url_result("https://example.com"),
            Err(BackendError::Unsupported)
        );
    }

    /// quadraui#956: no file manager window a terminal could reveal
    /// anything in — the one member of this issue's four TUI does not
    /// implement for real (see `TuiPlatformServices::reveal_in_file_manager`'s
    /// doc).
    #[test]
    fn reveal_in_file_manager_reports_unsupported_on_tui() {
        let services = TuiPlatformServices::new();
        assert_eq!(
            services.reveal_in_file_manager(std::path::Path::new("/tmp")),
            Err(BackendError::Unsupported)
        );
    }

    /// quadraui#956: BEL is a terminal's only notification channel — this
    /// pins that `beep` reports success (rather than the trait's
    /// `Unsupported` default) on every host this runs on, since writing
    /// to stdout/`/dev/tty` never depends on a live desktop session the
    /// way `open_path`/`move_to_trash` might. Doesn't assert on the
    /// actual bytes written (stdout isn't captured here) — just that this
    /// backend claims real support instead of silently degrading.
    #[test]
    fn beep_reports_success() {
        let services = TuiPlatformServices::new();
        assert_eq!(services.beep(), Ok(()));
    }

    /// quadraui#959: a terminal has no synchronous cursor-position query
    /// — `cursor_screen_point` always reports `Unsupported`, on every
    /// host this runs on (no environment dependency, unlike `displays`
    /// below).
    #[test]
    fn cursor_screen_point_always_reports_unsupported_on_tui() {
        let services = TuiPlatformServices::new();
        assert_eq!(
            services.cursor_screen_point(),
            Err(BackendError::Unsupported)
        );
    }

    /// quadraui#959: `displays` reads the real controlling terminal via
    /// `crossterm::terminal::size()` — genuinely environment-dependent
    /// (no pty under `cargo test`'s captured output is a real,
    /// non-bug `Err`, same "skip rather than fail" posture
    /// `backend::secret_store_tests`' `answered` helper documents for an
    /// unreachable OS credential store). When it *does* answer, pin the
    /// honest-degrade shape this backend promises: exactly one display,
    /// `work_area == bounds`, `scale == 1.0`, `primary == true`.
    #[test]
    #[allow(clippy::print_stderr)]
    fn displays_reports_one_cell_grid_display_or_skips_headless() {
        let services = TuiPlatformServices::new();
        match services.displays() {
            Ok(displays) => {
                assert_eq!(displays.len(), 1);
                let d = displays[0];
                assert_eq!(d.work_area, d.bounds);
                assert_eq!(d.scale, 1.0);
                assert!(d.primary);
            }
            Err(e) => {
                eprintln!(
                    "skipping: displays() failed in this environment ({e:?}) — no controlling \
                     terminal attached (piped/captured output, no pty)"
                );
            }
        }
    }
}

/// Issue #965 acceptance bar: prove the nested dialog loop returns a real
/// user choice — a selected path, a saved path, a pressed button — not
/// merely that it doesn't panic. Every test here wires a real
/// `ratatui::Terminal<TestBackend>` via `set_dialog_surface` and a
/// scripted `UiEvent` sequence via `queue_dialog_events`, then calls the
/// `PlatformServices` method exactly as an app would — the *same*
/// `FilePickerController`/`MessageDialogController` state machine and
/// `TuiBackend` paint path production uses, with no live terminal
/// involved. See the module doc's "Dialogs (issue #965)" section.
#[cfg(test)]
mod dialog_tests {
    use super::*;
    use crate::backend::{MessageDialogButton, MessageDialogOptions};
    use crate::types::WidgetId;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Wire a fresh `TuiPlatformServices` to a headless `TestBackend`
    /// surface, so its nested dialog loop has somewhere to paint without
    /// a live terminal.
    fn wired_services(width: u16, height: u16) -> TuiPlatformServices {
        let services = TuiPlatformServices::new();
        let terminal = Terminal::new(TestBackend::new(width, height)).expect("TestBackend");
        let surface: Rc<RefCell<dyn DialogSurface>> = Rc::new(RefCell::new(terminal));
        services.set_dialog_surface(surface);
        services
    }

    fn key_char(c: char) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn key_named(k: NamedKey) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(k),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    #[test]
    fn show_file_open_dialog_confirms_a_selected_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("target.txt"), b"").expect("write file");
        let services = wired_services(60, 20);
        // Filter down to the one file whose name contains "targ", then
        // confirm it — the fuzzy filter drops the unrelated ".." entry
        // along the way (it has no "t"/"a"/"r"/"g" subsequence).
        services.queue_dialog_events(vec![
            key_char('t'),
            key_char('a'),
            key_char('r'),
            key_char('g'),
            key_named(NamedKey::Enter),
        ]);
        let opts = FileDialogOptions {
            initial_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        };
        let result = services.show_file_open_dialog(opts);
        assert_eq!(result, Some(tmp.path().join("target.txt")));
    }

    #[test]
    fn show_file_open_dialog_cancelled_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("target.txt"), b"").expect("write file");
        let services = wired_services(60, 20);
        services.queue_dialog_events(vec![key_named(NamedKey::Escape)]);
        let opts = FileDialogOptions {
            initial_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        };
        assert_eq!(services.show_file_open_dialog(opts), None);
    }

    #[test]
    fn show_file_save_dialog_confirms_a_typed_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let services = wired_services(60, 20);
        services.queue_dialog_events(vec![
            key_char('n'),
            key_char('e'),
            key_char('w'),
            key_char('.'),
            key_char('t'),
            key_char('x'),
            key_char('t'),
            key_named(NamedKey::Enter),
        ]);
        let opts = FileDialogOptions {
            initial_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        };
        let result = services.show_file_save_dialog(opts);
        assert_eq!(result, Some(tmp.path().join("new.txt")));
    }

    #[test]
    fn show_file_save_dialog_seeds_initial_filename() {
        // Confirming immediately (no typing) should save under
        // `initial_filename` verbatim — proves `opts.initial_filename`
        // actually reaches the picker's query field.
        let tmp = tempfile::tempdir().expect("tempdir");
        let services = wired_services(60, 20);
        services.queue_dialog_events(vec![key_named(NamedKey::Enter)]);
        let opts = FileDialogOptions {
            initial_dir: Some(tmp.path().to_path_buf()),
            initial_filename: Some("untitled.txt".to_string()),
            ..Default::default()
        };
        let result = services.show_file_save_dialog(opts);
        assert_eq!(result, Some(tmp.path().join("untitled.txt")));
    }

    #[test]
    fn show_message_dialog_resolves_a_pressed_button() {
        let services = wired_services(60, 20);
        // Move focus to the second button, then activate it.
        services.queue_dialog_events(vec![key_named(NamedKey::Right), key_named(NamedKey::Enter)]);
        let opts = MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "Do you want to save?".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                },
            ],
            severity: None,
        };
        let result = services.show_message_dialog(opts);
        assert_eq!(result, Some(WidgetId::new("cancel")));
    }

    #[test]
    fn show_message_dialog_escape_resolves_cancel_button() {
        let services = wired_services(60, 20);
        services.queue_dialog_events(vec![key_named(NamedKey::Escape)]);
        let opts = MessageDialogOptions {
            title: "Heads up".to_string(),
            body: "Something happened.".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("ok"),
                    label: "OK".into(),
                    is_default: true,
                    is_cancel: false,
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                },
            ],
            severity: None,
        };
        assert_eq!(
            services.show_message_dialog(opts),
            Some(WidgetId::new("cancel"))
        );
    }

    #[test]
    fn scripted_queue_exhaustion_synthesizes_escape_instead_of_hanging() {
        // A malformed script that never resolves the dialog must not spin
        // forever — `next_dialog_events` synthesizes Escape once the
        // queue drains, which resolves to the cancel button here.
        let services = wired_services(60, 20);
        services.queue_dialog_events(Vec::<UiEvent>::new());
        let opts = MessageDialogOptions {
            title: "T".to_string(),
            body: "B".to_string(),
            buttons: vec![MessageDialogButton {
                id: WidgetId::new("cancel"),
                label: "Cancel".into(),
                is_default: false,
                is_cancel: true,
            }],
            severity: None,
        };
        assert_eq!(
            services.show_message_dialog(opts),
            Some(WidgetId::new("cancel"))
        );
    }
}
