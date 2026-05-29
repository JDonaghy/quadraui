//! Default `PlatformServices` impl for the TUI backend.
//!
//! Clipboard access uses **both** `arboard` (local desktop clipboard)
//! and OSC 52 (terminal clipboard via escape sequence). Writing via
//! OSC 52 works over SSH and inside tmux (when `set -g set-clipboard
//! on` is set), covering environments where arboard cannot reach the
//! host clipboard.
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

/// Emit an OSC 52 clipboard-write sequence for `text` to `writer`.
///
/// The sequence is: `ESC ] 52 ; c ; <base64(text)> BEL`.
///
/// Terminal requirements:
/// - Most modern terminals (kitty, WezTerm, iTerm2, alacritty, xterm)
///   support OSC 52 by default.
/// - **tmux**: `set -g set-clipboard on` is required for tmux to
///   forward the sequence to the outer terminal.
/// - **screen**: not widely supported; falls back silently.
/// - **SSH**: works when the remote terminal supports OSC 52 passthrough
///   (most do).
pub(crate) fn emit_osc52_to(text: &str, writer: &mut dyn std::io::Write) {
    let encoded = base64_encode(text.as_bytes());
    // `\x1b]52;c;…\x07` — ESC ] = OSC introducer; BEL = ST alternative.
    let _ = write!(writer, "\x1b]52;c;{}\x07", encoded);
    let _ = writer.flush();
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
        emit_osc52_to("hello", &mut out);
        // ESC ] 52 ; c ; aGVsbG8= BEL
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_empty_text() {
        let mut out = Vec::new();
        emit_osc52_to("", &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;\x07");
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
