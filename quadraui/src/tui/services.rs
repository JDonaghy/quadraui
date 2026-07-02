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
//! Other services (file picker, notifications, URL open) remain no-op
//! stubs — apps that need them supply their own `PlatformServices` or
//! call platform APIs directly.

use std::cell::RefCell;
use std::path::PathBuf;

use crate::backend::{Clipboard, FileDialogOptions, Notification, PlatformServices};

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
        let _ = child.wait();
        return;
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
