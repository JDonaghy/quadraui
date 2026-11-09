// ─────────────────────────────────────────────────────────────────────
// validate_open_url
// ─────────────────────────────────────────────────────────────────────

/// Schemes [`validate_open_url`] accepts unconditionally: the only
/// things [`super::open_with::open_with_default`]'s `open`/`xdg-open`/`ShellExecuteW` is
/// actually documented to hand to a browser or mail client.
///
/// `#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]`: [`validate_open_url`]'s
/// adopters today are `tui::services::TuiPlatformServices::open_url_result`,
/// `gtk::services::GtkPlatformServices::open_url_result`,
/// `win::services::WinPlatformServices::open_url_result`, and
/// `macos::services::PlatformServices::open_url_result`.
#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]
const OPEN_URL_SAFE_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// File extensions [`validate_open_url`] treats as "launches code or an
/// app bundle" rather than "opens a document", even under the otherwise-
/// allowed `file:` scheme. `app`, `pkg`, `webloc`, `scpt`, `terminal`, and
/// `workflow` cover macOS bundles/launchers (a directory, not a single
/// file, in `app`'s case, but every opener resolves it the same way it
/// resolves a file target); `exe`, `bat`, `cmd`, `com`, `scr`, `msi`,
/// `command`, `sh`, `lnk`, `hta`, `vbs`, `vbe`, `js`, `jse`, `wsf`, `ps1`,
/// `pif`, `cpl`, `msc`, `reg`, and `jar` cover a Windows/cross-platform
/// executable, script, shortcut, or installer; `desktop` covers a Linux
/// launcher.
///
/// This is a denylist on the *last path segment's extension* and is a
/// known-incomplete approximation, not a complete "is this executable"
/// test: on Unix, the executable bit (not a suffix) is what the kernel
/// actually honors, so an extension-less local script with the exec bit
/// set and a shebang (or a well-known binary like `/usr/bin/xterm`) is not
/// caught here. Closing that gap needs a filesystem `stat` on the
/// resolved target, which is out of scope for a pure string check.
#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]
const OPEN_URL_EXECUTABLE_EXTENSIONS: [&str; 28] = [
    "exe", "bat", "cmd", "com", "scr", "msi", "app", "command", "sh", "desktop", "lnk", "hta",
    "vbs", "vbe", "js", "jse", "wsf", "ps1", "pif", "cpl", "msc", "reg", "jar", "pkg", "webloc",
    "scpt", "terminal", "workflow",
];

/// Validate that `url` is safe to hand to [`super::open_with::open_with_default`]/
/// [`super::open_with::try_open_with_default`] before any backend's
/// `PlatformServices::open_url`/`open_url_result` spawns it. Those
/// openers treat their argument as "whatever its scheme (or, for a bare
/// path, its file association) says to launch", not specifically "open a
/// URL in a browser" — an unvalidated call can hand them a local
/// executable or an app bundle instead of a URL.
///
/// Accepts `http:`, `https:`, and `mailto:` unconditionally
/// ([`OPEN_URL_SAFE_SCHEMES`]). Accepts `file:` only when the target's
/// last path segment, after percent-decoding
/// ([`open_url_target_is_executable`]), does not end in one of
/// [`OPEN_URL_EXECUTABLE_EXTENSIONS`]. Rejects everything else with
/// [`crate::backend::BackendError::Unsupported`]:
/// - a bare filesystem path with no scheme at all, including a Windows
///   drive-letter path (`C:\Windows\System32\calc.exe`) — its
///   single-letter prefix before the colon is not one of the three
///   recognized schemes, so it falls through to this same rejection
///   rather than needing a separate "looks like a drive letter" check;
/// - any other scheme this crate's openers have no documented reason to
///   launch, e.g. `javascript:` or `data:`.
#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]
pub(crate) fn validate_open_url(url: &str) -> crate::backend::ServiceResult<()> {
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err(crate::backend::BackendError::Unsupported);
    };
    let scheme = scheme.to_ascii_lowercase();
    if OPEN_URL_SAFE_SCHEMES.contains(&scheme.as_str()) {
        return Ok(());
    }
    if scheme == "file" && !open_url_target_is_executable(rest) {
        return Ok(());
    }
    Err(crate::backend::BackendError::Unsupported)
}

/// Whether a `file:` URL's target (everything after the scheme's `:`,
/// query/fragment included) names something in
/// [`OPEN_URL_EXECUTABLE_EXTENSIONS`] — the check [`validate_open_url`]'s
/// `file` arm runs. Percent-decodes the target first
/// ([`percent_decode_fully`]) so a caller cannot dodge the check by
/// encoding the dot or the extension itself (`Calculator%2Eapp`,
/// `run.e%78e`) — every opener this crate hands a `file:` URL to
/// (`open`/`xdg-open`/`ShellExecuteW`) resolves it through a URL parser
/// that performs the same decoding before touching the filesystem, so
/// checking the raw, still-encoded text would validate a different string
/// than the one that actually gets opened. Then strips a trailing
/// `?query`/`#fragment` and any trailing path separators before looking
/// at the final segment's extension, so
/// `file:///Applications/Calculator.app/` and
/// `file:///C:/tmp/run.exe?x=1` are both caught.
#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]
fn open_url_target_is_executable(target: &str) -> bool {
    let decoded = percent_decode_fully(target);
    let path_part = decoded.split(['?', '#']).next().unwrap_or(&decoded);
    let trimmed = path_part.trim_end_matches('/');
    let last_segment = trimmed.rsplit('/').next().expect(
        "str::rsplit always yields at least one item, even for an empty or separator-only input",
    );
    let Some((_, ext)) = last_segment.rsplit_once('.') else {
        return false;
    };
    let ext_lower = ext.to_ascii_lowercase();
    OPEN_URL_EXECUTABLE_EXTENSIONS.contains(&ext_lower.as_str())
}

/// Decode `%XX` percent-escapes in `s`, repeating until a pass changes
/// nothing or a small iteration cap is hit, so a doubly-encoded escape
/// (`%2561` → `%61` → `a`) cannot slip past a single decode pass the way
/// it slipped past [`open_url_target_is_executable`] having no decode
/// step at all. An invalid or truncated `%` escape (not followed by two
/// hex digits) is left as literal text rather than treated as an error:
/// this function backs a conservative security check, not a strict URL
/// parser, so "decode what's decodable, leave the rest alone" is the
/// right failure mode — it only ever makes the executable check see
/// *more* of the target, never less.
#[cfg(any(feature = "tui", feature = "gtk", feature = "win", feature = "macos"))]
fn percent_decode_fully(s: &str) -> std::borrow::Cow<'_, str> {
    fn decode_once(s: &str) -> Option<String> {
        if !s.contains('%') {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        let mut changed = false;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    changed = true;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        changed.then(|| String::from_utf8_lossy(&out).into_owned())
    }

    const MAX_PASSES: u8 = 4;
    let mut current = std::borrow::Cow::Borrowed(s);
    for _ in 0..MAX_PASSES {
        match decode_once(&current) {
            Some(next) => current = std::borrow::Cow::Owned(next),
            None => break,
        }
    }
    current
}

#[cfg(all(
    test,
    any(feature = "tui", feature = "gtk", feature = "win", feature = "macos")
))]
mod validate_open_url_tests {
    use super::*;
    use crate::backend::BackendError;

    #[test]
    fn accepts_http_https_and_mailto() {
        assert_eq!(validate_open_url("http://example.com/docs"), Ok(()));
        assert_eq!(validate_open_url("https://example.com/docs"), Ok(()));
        assert_eq!(validate_open_url("mailto:user@example.com"), Ok(()));
    }

    #[test]
    fn accepts_scheme_case_insensitively() {
        assert_eq!(validate_open_url("HTTPS://example.com/docs"), Ok(()));
    }

    #[test]
    fn accepts_a_file_url_pointing_at_an_ordinary_document() {
        assert_eq!(validate_open_url("file:///home/user/report.pdf"), Ok(()));
    }

    #[test]
    fn rejects_a_bare_unix_path_with_no_scheme() {
        assert_eq!(
            validate_open_url("/Applications/Calculator.app"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_bare_windows_executable_path() {
        assert_eq!(
            validate_open_url("C:\\Windows\\System32\\calc.exe"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_pointing_at_a_windows_executable() {
        assert_eq!(
            validate_open_url("file:///C:/Windows/System32/calc.exe"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_pointing_at_a_macos_app_bundle() {
        assert_eq!(
            validate_open_url("file:///Applications/Calculator.app"),
            Err(BackendError::Unsupported)
        );
        // A trailing slash on the bundle directory must not dodge the check.
        assert_eq!(
            validate_open_url("file:///Applications/Calculator.app/"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_pointing_at_an_executable_with_a_query_string() {
        assert_eq!(
            validate_open_url("file:///C:/tmp/run.exe?x=1"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_javascript_and_data_schemes() {
        assert_eq!(
            validate_open_url("javascript:alert(1)"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(
            validate_open_url("data:text/html,<script>alert(1)</script>"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_with_a_percent_encoded_dot_in_a_bundle_name() {
        // "Calculator%2Eapp" has no literal `.`, so a check against the raw
        // text would miss it; every opener this validates for decodes the
        // percent-escape before resolving the path, so the check must too.
        assert_eq!(
            validate_open_url("file:///Applications/Calculator%2Eapp"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_with_a_percent_encoded_executable_extension() {
        assert_eq!(
            validate_open_url("file:///C:/tmp/run%2Eexe"),
            Err(BackendError::Unsupported)
        );
        // The extension's own letters percent-encoded, not just the dot.
        assert_eq!(
            validate_open_url("file:///C:/tmp/run.e%78e"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn rejects_a_file_url_with_a_doubly_percent_encoded_extension() {
        // `%2561` decodes to `%61` on the first pass and `a` on the second;
        // a single decode pass would still miss this.
        assert_eq!(
            validate_open_url("file:///tmp/run.ex%2565"),
            Err(BackendError::Unsupported)
        );
    }

    #[test]
    fn accepts_a_file_url_with_an_unrelated_percent_escape() {
        // A percent-escaped space in an ordinary document name must not be
        // rejected just because it contains a `%`.
        assert_eq!(
            validate_open_url("file:///home/user/My%20Report.pdf"),
            Ok(())
        );
    }
}
