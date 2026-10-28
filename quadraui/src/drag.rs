//! Drag-out-to-the-OS seam — the payload type
//! [`crate::PlatformServices::begin_drag_out`] trades in.
//!
//! Drag-*in* already works on every GUI backend
//! ([`crate::UiEvent::FilesDropped`]): the OS hands quadraui a drop. This
//! module is the other direction — Electron's `webContents.startDrag`
//! parity — initiating a drag of an item *out* of a quadraui window to
//! Finder / Explorer / another app. It defines the backend-neutral
//! *contract* only: [`DragOutPayload`] is the data a caller builds, and
//! [`crate::PlatformServices::begin_drag_out`] defaults to
//! [`crate::BackendError::Unsupported`] on every backend. Starting the
//! actual OS drag session — `NSView::beginDraggingSession` on macOS,
//! `IDropSource`/`DoDragDrop` on Win-GUI, `gtk4::DragSource` (held by a
//! widget, not pushed imperatively) on GTK — is a separate,
//! backend-specific follow-on; this module only has to give each of those
//! a contract to implement against.
//!
//! [`crate::compose::tab_group`]'s `DragSource` is unrelated: that one
//! reorders tabs *within* this app's own window and never reaches the OS.
//!
//! TUI has no OS drag-and-drop surface to initiate a session on — the
//! same "physically absent on a terminal" reasoning
//! [`crate::PlatformServices::register_global_shortcut`]'s doc gives for
//! an OS-wide hotkey — so it is expected to leave this at its default
//! rather than fake one.

use std::path::PathBuf;

/// What to hand the OS when starting a drag-out session —
/// [`crate::PlatformServices::begin_drag_out`]'s argument.
///
/// Carries at least one file path, free-text (serving as the
/// `text/uri-list`-style fallback for a target with no native file-drag
/// acceptor), or both — mirroring Electron's `startDrag({file, files})`
/// shape but staying string/path-based rather than requiring a rendered
/// drag icon, which no in-tree backend has a contract for yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragOutPayload {
    files: Vec<PathBuf>,
    text: Option<String>,
}

impl DragOutPayload {
    /// An empty payload — carries neither files nor text. Chain
    /// [`Self::with_files`]/[`Self::with_text`] to populate it; passing
    /// this unmodified to
    /// [`crate::PlatformServices::begin_drag_out`] is a caller bug, not
    /// something this type itself rejects (see [`Self::is_empty`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the file paths to drag out, *replacing* any set by a previous
    /// call — this is not an appender. Chain a single call with every
    /// path the drag should carry.
    #[must_use]
    pub fn with_files(mut self, files: impl IntoIterator<Item = PathBuf>) -> Self {
        self.files = files.into_iter().collect();
        self
    }

    /// Attach free-text — the `text/uri-list` fallback for a drop target
    /// with no native file-drag acceptor (e.g. a plain text field).
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// The file paths attached via [`Self::with_files`].
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The free-text attached via [`Self::with_text`], if any.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Whether this payload carries neither files nor text — nothing for
    /// a backend to drag out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.text.is_none()
    }

    /// Render [`Self::files`] as a `text/uri-list` (RFC 2483) body — one
    /// percent-encoded `file://` URI per line, CRLF-terminated (including
    /// the last line) per the spec. A backend with only a generic "offer
    /// this text" drag surface and no dedicated file-drag API can hand a
    /// target this instead of the raw paths.
    ///
    /// Each path is rendered in its *host's* native flavor, not a
    /// POSIX-only one: on Windows a drive-letter path becomes
    /// `file:///C:/dir/a.txt` and a UNC path `file://server/share/f`
    /// (RFC 8089) rather than a raw backslash concatenation, while on
    /// Unix the path's raw bytes are encoded, so a name with no valid
    /// UTF-8 form round-trips losslessly. Either way every byte outside
    /// RFC 3986's unreserved set — a space, `#`, `%`, `,` — is
    /// percent-encoded rather than left to break the receiving app's
    /// parse.
    ///
    /// A *relative* path is silently skipped: it has no filesystem-root
    /// anchor, so rendering it would read its first segment back as the
    /// URI authority instead of as part of the path. Call
    /// [`Self::files`] directly for the unfiltered list.
    #[must_use]
    pub fn to_uri_list(&self) -> String {
        let mut out = self
            .files
            .iter()
            .filter_map(|p| crate::desktop::path_to_file_uri(p))
            .collect::<Vec<_>>()
            .join("\r\n");
        if !out.is_empty() {
            out.push_str("\r\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute-path literals and the `file://` URIs `to_uri_list`
    /// renders them as, in the flavor of whichever host the test runs
    /// on: a POSIX literal has no well-defined URI on Windows (no drive
    /// letter, no UNC prefix) and a drive-letter literal is just an
    /// oddly-named relative file on Unix, so a single hardcoded pair
    /// would assert `""` on one of the two hosts.
    #[cfg(unix)]
    const URI_CASES: [(&str, &str); 3] = [
        ("/tmp/a.txt", "file:///tmp/a.txt"),
        ("/tmp/b.txt", "file:///tmp/b.txt"),
        ("/tmp/my dir/a.txt", "file:///tmp/my%20dir/a.txt"),
    ];
    #[cfg(windows)]
    const URI_CASES: [(&str, &str); 3] = [
        (r"C:\dir\a.txt", "file:///C:/dir/a.txt"),
        (r"C:\dir\b.txt", "file:///C:/dir/b.txt"),
        (r"C:\dir\a b.txt", "file:///C:/dir/a%20b.txt"),
    ];

    #[test]
    fn new_payload_is_empty() {
        assert!(DragOutPayload::new().is_empty());
        assert_eq!(DragOutPayload::new().files(), &[] as &[PathBuf]);
        assert_eq!(DragOutPayload::new().text(), None);
    }

    #[test]
    fn with_files_populates_and_is_not_empty() {
        let payload = DragOutPayload::new()
            .with_files([PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]);
        assert!(!payload.is_empty());
        assert_eq!(
            payload.files(),
            &[PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]
        );
        assert_eq!(payload.text(), None);
    }

    #[test]
    fn with_text_populates_and_is_not_empty() {
        let payload = DragOutPayload::new().with_text("hello drag");
        assert!(!payload.is_empty());
        assert_eq!(payload.text(), Some("hello drag"));
        assert!(payload.files().is_empty());
    }

    #[test]
    fn with_files_and_text_combine() {
        let payload = DragOutPayload::new()
            .with_files([PathBuf::from("/tmp/a.txt")])
            .with_text("fallback text");
        assert_eq!(payload.files(), &[PathBuf::from("/tmp/a.txt")]);
        assert_eq!(payload.text(), Some("fallback text"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn to_uri_list_renders_file_uris_crlf_joined() {
        let (first, first_uri) = URI_CASES[0];
        let (second, second_uri) = URI_CASES[1];
        let payload =
            DragOutPayload::new().with_files([PathBuf::from(first), PathBuf::from(second)]);
        assert_eq!(
            payload.to_uri_list(),
            format!("{first_uri}\r\n{second_uri}\r\n")
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn to_uri_list_percent_encodes_a_path_with_a_space() {
        let (spaced, spaced_uri) = URI_CASES[2];
        assert!(spaced.contains(' '), "the case must carry a raw space");
        assert!(spaced_uri.contains("%20"), "the space must be encoded");
        let payload = DragOutPayload::new().with_files([PathBuf::from(spaced)]);
        assert_eq!(payload.to_uri_list(), format!("{spaced_uri}\r\n"));
    }

    #[test]
    fn to_uri_list_is_empty_for_no_files() {
        assert_eq!(DragOutPayload::new().to_uri_list(), "");
        assert_eq!(DragOutPayload::new().with_text("x").to_uri_list(), "");
    }

    #[test]
    fn to_uri_list_skips_a_relative_path() {
        // Relative on every host this crate builds for: no leading `/`,
        // no drive letter, no UNC prefix — so there is no well-defined
        // `file://` URI to render it as.
        let payload = DragOutPayload::new().with_files([PathBuf::from("a.txt")]);
        assert_eq!(payload.to_uri_list(), "");
        assert_eq!(payload.files(), &[PathBuf::from("a.txt")]);
    }
}
