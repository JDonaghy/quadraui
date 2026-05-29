//! Default `PlatformServices` impl for the TUI backend.
//!
//! Clipboard access uses **both** `arboard` (local desktop clipboard)
//! and OSC 52 (terminal clipboard via escape sequence). Writing via
//! OSC 52 works over SSH and inside tmux, covering environments where
//! arboard cannot reach the host clipboard.
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

// ── TuiClipboard ──────────────────────────────────────────────────────────────

/// System clipboard that writes via **both** arboard and OSC 52.
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
        emit_osc52_to(text, &mut std::io::stdout());
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
