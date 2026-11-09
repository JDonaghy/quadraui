/// Escape `"` and `\` so a string can be embedded inside an AppleScript
/// double-quoted string literal. Shared by [`display_notification_script`]
/// below — `macos::services::MacPlatformServices::send_notification`'s
/// own private helper until issue #1092 gave `tui::services` a second
/// `osascript` call site (the TUI degrade's macOS notification fallback
/// leg) that needed byte-for-byte the same escaping.
#[cfg(all(target_os = "macos", any(feature = "macos", feature = "tui")))]
pub(crate) fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the `osascript -e` argument an AppleScript `display
/// notification` call spawns. `display notification` needs an explicit
/// `sound name` clause to make *any* sound — omitting it entirely is
/// silent, not the system default. So `silent: false` (the default — a
/// caller that never calls `Notification::with_silent` gets a
/// notification that actually makes a sound, matching ordinary
/// desktop-notification expectations) adds `sound name ""`, which plays
/// the system default notification sound.
#[cfg(all(target_os = "macos", any(feature = "macos", feature = "tui")))]
pub(crate) fn display_notification_script(title: &str, body: &str, silent: bool) -> String {
    let mut script = format!(
        "display notification \"{body}\" with title \"{title}\"",
        body = applescript_escape(body),
        title = applescript_escape(title),
    );
    if !silent {
        script.push_str(" sound name \"\"");
    }
    script
}

/// Sniff the file extension that matches an encoded image's magic
/// bytes, for naming a temp copy a native API will re-decode from disk.
///
/// `UNNotificationAttachment` (macOS) picks its decoder from the
/// attached file's *extension*, not from its content, so an
/// [`crate::ImageSource::Bytes`] icon needs a plausible extension before it
/// will render. `None` means "not a format recognised here" — callers
/// skip the icon rather than guess an extension that would make the
/// platform decoder fail on a correctly-encoded image.
#[cfg(any(all(target_os = "macos", feature = "macos"), test))]
pub(crate) fn encoded_image_extension(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) {
        return Some("png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some("tiff");
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    // SVG has no magic number; the two openings a serialiser emits are
    // an XML prolog and a bare root element, either possibly preceded
    // by whitespace or a UTF-8 BOM. A prolog alone is any XML document,
    // so the `<svg` root element has to turn up inside the sniffed
    // window too — a plist or an SVG whose prolog and doctype run past
    // it both sniff as "unrecognised", which costs an icon rather than
    // writing an `.svg` the platform decoder would choke on.
    let text = bytes
        .strip_prefix(b"\xef\xbb\xbf".as_slice())
        .unwrap_or(bytes);
    let window = text.get(..text.len().min(512))?;
    let text = match std::str::from_utf8(window) {
        Ok(text) => text,
        // The window can land mid-character; the valid prefix is still
        // enough to spot a root element.
        Err(e) => std::str::from_utf8(&window[..e.valid_up_to()]).ok()?,
    };
    let text = text.trim_start();
    if text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg")) {
        return Some("svg");
    }
    None
}

/// Copy a notification icon into `dir` as `<stem>.<ext>` and return the
/// path written, so a native notification API that can only attach a
/// *file* has one to point at.
///
/// Always copies, never hands over the caller's own file: macOS's
/// `UNNotificationAttachment` **moves** the attached file into the
/// system attachment data store, so attaching a consumer-owned path
/// directly would delete that file out from under them.
///
/// The extension comes from the source path when it has one (a
/// consumer naming `logo.png` knows the format better than any sniffer)
/// and otherwise from [`encoded_image_extension`]; an unrecognised
/// encoding is an [`std::io::ErrorKind::InvalidData`] error so the
/// caller can skip the icon instead of writing a file the platform
/// cannot decode. `dir` is created if missing.
#[cfg(any(all(target_os = "macos", feature = "macos"), test))]
pub(crate) fn materialize_notification_icon(
    source: &crate::primitives::image::ImageSource,
    dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<std::path::PathBuf> {
    use crate::primitives::image::ImageSource;

    let (bytes, source_extension) = match source {
        ImageSource::Bytes(bytes) => (bytes.clone(), None),
        ImageSource::Path(path) => {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| !e.is_empty())
                .map(str::to_ascii_lowercase);
            (std::fs::read(path)?, extension)
        }
    };
    let extension = match source_extension {
        Some(extension) => extension,
        None => encoded_image_extension(&bytes)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "notification icon is not a recognised image encoding",
                )
            })?
            .to_string(),
    };
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{stem}.{extension}"));
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
mod notification_icon_tests {
    use super::{encoded_image_extension, materialize_notification_icon};
    use crate::primitives::image::ImageSource;

    const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

    /// A scratch directory that cleans itself up when the returned
    /// guard drops. `icons` is deliberately *not* created, so a test can
    /// assert `materialize_notification_icon` created (or didn't create)
    /// the directory it was handed.
    fn scratch_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("scratch dir");
        let icons = root.path().join("icons");
        (root, icons)
    }

    #[test]
    fn encoded_image_extension_recognises_common_encodings() {
        assert_eq!(encoded_image_extension(PNG_HEADER), Some("png"));
        assert_eq!(
            encoded_image_extension(b"\xff\xd8\xff\xe0JFIF"),
            Some("jpg")
        );
        assert_eq!(encoded_image_extension(b"GIF89a....."), Some("gif"));
        assert_eq!(encoded_image_extension(b"II*\0\x08\0\0\0"), Some("tiff"));
        assert_eq!(encoded_image_extension(b"MM\0*\0\0\0\x08"), Some("tiff"));
        assert_eq!(encoded_image_extension(b"BM\x8a\x02\0\0"), Some("bmp"));
        assert_eq!(
            encoded_image_extension(b"RIFF\x1a\0\0\0WEBPVP8 "),
            Some("webp")
        );
    }

    #[test]
    fn encoded_image_extension_recognises_svg_with_prolog_bom_or_bare_root() {
        assert_eq!(
            encoded_image_extension(b"<?xml version=\"1.0\"?><svg/>"),
            Some("svg")
        );
        assert_eq!(
            encoded_image_extension(b"  \n<svg width=\"8\"/>"),
            Some("svg")
        );
        assert_eq!(encoded_image_extension(b"\xef\xbb\xbf<svg/>"), Some("svg"));
    }

    #[test]
    fn encoded_image_extension_rejects_unrecognised_bytes() {
        assert_eq!(encoded_image_extension(b""), None);
        assert_eq!(encoded_image_extension(b"not an image at all"), None);
        // A truncated PNG signature is not a PNG.
        assert_eq!(encoded_image_extension(b"\x89PNG"), None);
    }

    #[test]
    fn encoded_image_extension_rejects_xml_that_is_not_svg() {
        assert_eq!(
            encoded_image_extension(b"<?xml version=\"1.0\"?><plist version=\"1.0\"></plist>"),
            None
        );
    }

    #[test]
    fn materialize_bytes_icon_sniffs_extension_and_creates_the_directory() {
        let (_root, dir) = scratch_dir();
        let written =
            materialize_notification_icon(&ImageSource::Bytes(PNG_HEADER.to_vec()), &dir, "icon-7")
                .expect("png bytes materialize");

        assert_eq!(written, dir.join("icon-7.png"));
        assert_eq!(std::fs::read(&written).unwrap(), PNG_HEADER);
    }

    #[test]
    fn materialize_path_icon_copies_and_keeps_the_source_extension() {
        let (_root, dir) = scratch_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("logo.PNG");
        std::fs::write(&original, PNG_HEADER).unwrap();

        let written =
            materialize_notification_icon(&ImageSource::Path(original.clone()), &dir, "icon-1")
                .expect("path icon materializes");

        assert_eq!(written, dir.join("icon-1.png"));
        // The consumer's own file is copied, never moved away.
        assert!(original.exists());
    }

    #[test]
    fn materialize_path_icon_without_extension_falls_back_to_sniffing() {
        let (_root, dir) = scratch_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("logo");
        std::fs::write(&original, PNG_HEADER).unwrap();

        let written = materialize_notification_icon(&ImageSource::Path(original), &dir, "icon-2")
            .expect("extensionless icon materializes");

        assert_eq!(written, dir.join("icon-2.png"));
    }

    #[test]
    fn materialize_unrecognised_bytes_reports_invalid_data_and_writes_nothing() {
        let (_root, dir) = scratch_dir();
        let err = materialize_notification_icon(
            &ImageSource::Bytes(b"definitely not an image".to_vec()),
            &dir,
            "icon-3",
        )
        .expect_err("unrecognised encoding is rejected");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!dir.exists());
    }

    #[test]
    fn materialize_missing_path_reports_not_found() {
        let (_root, dir) = scratch_dir();
        let err =
            materialize_notification_icon(&ImageSource::Path(dir.join("nope.png")), &dir, "icon-4")
                .expect_err("a missing icon file is an error");

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}

/// Build the `osascript -e <script>` command
/// `tui::services::TuiPlatformServices::send_notification`'s macOS
/// fallback leg spawns — not yet spawned, so a test can
/// assert on the constructed argv (in particular, that `title`/`body`
/// reach `osascript` via [`display_notification_script`] unmodified)
/// without a live display session. Split out for the same reason
/// [`macos_reveal_in_file_manager_command`] and
/// [`linux_show_items_via_dbus_command`] are: every other command this
/// issue builds is unit-testable this way, and the notification fallback
/// legs shouldn't be the one exception.
#[cfg(all(target_os = "macos", feature = "tui"))]
pub(crate) fn tui_macos_notify_command(
    title: &str,
    body: &str,
    silent: bool,
) -> std::process::Command {
    let script = display_notification_script(title, body, silent);
    let mut cmd = std::process::Command::new("osascript");
    cmd.arg("-e").arg(script);
    cmd
}

/// Build the `notify-send <title> <body>` command
/// `tui::services::TuiPlatformServices::send_notification`'s Linux/BSD
/// fallback leg spawns — not yet spawned, so a test can
/// assert `title` and `body` arrive as two separate argv entries (not
/// shell-concatenated, so a body containing spaces can't be mistaken for
/// extra `notify-send` flags) without a live notification-daemon
/// session.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
pub(crate) fn tui_notify_send_command(title: &str, body: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.arg(title).arg(body);
    cmd
}

/// `unix`-gated as a whole, not per-test: both command builders this
/// module covers ([`tui_macos_notify_command`],
/// [`tui_notify_send_command`]) are themselves `cfg(unix)` — Windows has
/// no native command-line notifier leg at all (OSC 9 is natively
/// supported by Windows Terminal, so `tui::services`'s escape leg covers
/// it alone; see that module's "Notifications" doc). A
/// `#[cfg(all(test, feature = "tui"))]` module with only `cfg(unix)` and
/// `cfg(target_os = "macos")` tests inside compiles to an *empty* module
/// on Windows, whose `use super::*` is then an `unused_imports` warning —
/// and `ci.yml`'s windows-latest "Test (tui feature)" step runs under
/// `RUSTFLAGS: -D warnings`, so that warning is a hard build failure
/// there (and only there: the "Build (tui feature)" step above it sets no
/// such flag, which is why this surfaced as a test-step-only red).
/// Windows-side coverage of `send_notification` lives in
/// `tui::services`'s `notification_osc_tests` instead, which is exactly
/// the whole method on that platform.
#[cfg(all(test, unix, feature = "tui"))]
mod tui_send_notification_command_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn tui_macos_notify_command_uses_osascript_with_the_display_notification_script() {
        let cmd = tui_macos_notify_command("Build failed", "3 errors", false);
        assert_eq!(cmd.get_program(), "osascript");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "-e");
        assert_eq!(
            args[1],
            display_notification_script("Build failed", "3 errors", false).as_str()
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn tui_notify_send_command_passes_title_and_body_as_separate_args() {
        let cmd = tui_notify_send_command("Build failed", "3 errors");
        assert_eq!(cmd.get_program(), "notify-send");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["Build failed", "3 errors"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn tui_notify_send_command_keeps_body_out_of_argv0() {
        // A body containing something that looks like a flag must stay
        // its own argv entry, never get shell-concatenated with the
        // title into something `notify-send` could misparse.
        let cmd = tui_notify_send_command("t", "--urgency=critical");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["t", "--urgency=critical"]
        );
    }
}

#[cfg(all(test, target_os = "macos", any(feature = "macos", feature = "tui")))]
mod display_notification_script_tests {
    use super::*;

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
        // Order check: a backslash followed by a quote in input should
        // produce `\\\"` (escaped slash + escaped quote), not `\\\\\"`
        // (double-escaped slash + quote).
        assert_eq!(applescript_escape("\\\""), "\\\\\\\"");
    }

    #[test]
    fn display_notification_script_not_silent_adds_default_sound_clause() {
        let script = display_notification_script("t", "b", false);
        assert_eq!(
            script,
            "display notification \"b\" with title \"t\" sound name \"\""
        );
    }

    #[test]
    fn display_notification_script_silent_omits_sound_clause() {
        let script = display_notification_script("t", "b", true);
        assert_eq!(script, "display notification \"b\" with title \"t\"");
        assert!(!script.contains("sound"));
    }

    #[test]
    fn display_notification_script_escapes_title_and_body() {
        let script = display_notification_script("t\"itle", "b\\ody", true);
        assert_eq!(
            script,
            "display notification \"b\\\\ody\" with title \"t\\\"itle\""
        );
    }
}
