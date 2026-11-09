// ─────────────────────────────────────────────────────────────────────
// reveal_in_file_manager
// ─────────────────────────────────────────────────────────────────────

/// The TUI degrade for `PlatformServices::reveal_in_file_manager`. A
/// terminal session on a desktop can reveal a file in its file manager
/// exactly as readily as it can launch a browser, so `Unsupported` was a
/// placeholder, not a structural fact.
///
/// Tries, per platform:
/// - **Windows**: [`windows_explorer_select`] —
///   `ShellExecuteW(NULL, "open", "explorer.exe", "/select,\"<path>\"",
///   NULL, SW_SHOWNORMAL)`, the same minimal hand-written FFI declaration
///   [`super::open_with::windows_shell_execute_open_code`] uses, pointed at `explorer.exe`
///   with an explicit `/select,` switch instead of handing `path`
///   straight to the registered "open" handler.
/// - **macOS**: [`macos_reveal_in_file_manager_command`] — `open -R
///   <path>`, the CLI front end to the same
///   `NSWorkspace::activateFileViewerSelectingURLs` call
///   `macos::services::MacPlatformServices::reveal_in_file_manager` makes
///   directly; TUI has no AppKit to link against, so this reaches for the
///   command-line door into the identical native call instead.
/// - **Linux/BSD**: [`linux_show_items_via_dbus`] —
///   `org.freedesktop.FileManager1.ShowItems` over the session D-Bus, the
///   same standard method
///   `gtk::services::GtkPlatformServices::reveal_in_file_manager` drives
///   through a linked `gio::DBusConnection::call_sync`; TUI has no `gio`
///   (a `gtk`-feature-only dependency), so this drives the identical
///   well-known method through the `dbus-send` CLI tool instead — the
///   same "reach for the CLI door onto a native facility instead of
///   linking a client library" trade [`write_clipboard_via_native_tool`]
///   already makes for the system clipboard. Falls back to
///   [`super::open_with::try_open_with_default`] on `path`'s *parent directory* when that
///   call fails (no `FileManager1` listener — a headless container, a
///   minimal window manager with no file manager running): not a perfect
///   "reveal, selected" experience, but a real file-manager window opened
///   at the right place, on a desktop session that plainly has one,
///   instead of a blanket `Unsupported`.
///
/// Best-effort throughout, matching every other `ServiceResult`-returning
/// TUI degrade in this crate: a successful spawn/D-Bus call reports
/// `Ok(())` even though the launched file manager may itself fail to open
/// a window with no way for this call to observe it.
#[cfg(feature = "tui")]
pub(crate) fn reveal_in_file_manager(path: &std::path::Path) -> crate::backend::ServiceResult<()> {
    if try_reveal_in_file_manager(path) {
        Ok(())
    } else {
        Err(crate::backend::BackendError::Unsupported)
    }
}

/// The raw "did anything launch" half of [`reveal_in_file_manager`],
/// split out (mirroring [`super::open_with::try_open_with_default`]'s own split from
/// [`super::open_with::open_with_default`]) purely so tests can exercise the per-platform
/// dispatch without wrapping every assertion in a `ServiceResult`.
#[cfg(feature = "tui")]
fn try_reveal_in_file_manager(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        windows_explorer_select(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos_reveal_in_file_manager_command(path).spawn().is_ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if linux_show_items_via_dbus(path) {
            return true;
        }
        match path.parent() {
            Some(parent) => super::open_with::try_open_with_default(parent.as_os_str()),
            None => false,
        }
    }
}

/// macOS opener command, not yet spawned — `open -R <path>` ("Reveal in
/// Finder" from the command line). Factored out so a test can assert on
/// the program name and arguments a real call would spawn without
/// actually launching anything, the same shape
/// [`super::open_with::unix_open_with_default_command`] uses.
#[cfg(all(target_os = "macos", feature = "tui"))]
fn macos_reveal_in_file_manager_command(path: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Windows opener: `ShellExecuteW`, pointed at `explorer.exe` with an
/// explicit `/select,"<path>"` parameter — see [`reveal_in_file_manager`]'s
/// doc for why. Returns whether `ShellExecuteW` reports success, per
/// [`super::open_with::shell_execute_reports_success`]'s `> 32` rule, the same predicate
/// [`super::open_with::windows_shell_execute_open`] already uses.
#[cfg(all(windows, feature = "tui"))]
fn windows_explorer_select(path: &std::path::Path) -> bool {
    super::open_with::shell_execute_reports_success(windows_explorer_select_code(path))
}

/// The raw `ShellExecuteW(NULL, "open", "explorer.exe",
/// "/select,\"<path>\"", NULL, SW_SHOWNORMAL)` return value. Split from
/// [`windows_explorer_select`] so a test can assert on the actual
/// documented `SE_ERR_*` code a given path produces on a real host —
/// same split [`super::open_with::windows_shell_execute_open_code`] has from
/// [`super::open_with::windows_shell_execute_open`]. `path.display()` is safe to embed
/// unquoted-of-inner-quotes here: Windows' filesystem itself forbids `"`
/// in a path, so there is no in-band way for `path` to break out of the
/// `/select,"..."` quoting this builds. That covers embedded quotes, but
/// not the classic MSVCRT argv-splitting edge case where a path's literal
/// string ends in a backslash immediately before the closing quote (an
/// odd run of trailing backslashes there escapes the quote instead of
/// closing the string) — not exploitable for injection (nothing follows
/// for an escaped quote to expose) and not reachable through
/// `path.display()` for an ordinary file/directory path (which doesn't
/// carry a trailing separator), so [`windows_explorer_select_parameters`]
/// strips one defensively rather than leaving it as a latent trap.
#[cfg(all(windows, feature = "tui"))]
fn windows_explorer_select_code(path: &std::path::Path) -> isize {
    // SAFETY: see `windows_shell_execute_open_code`'s identical SAFETY
    // note — both wide-string buffers below are nul-terminated and kept
    // alive for the duration of the call.
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;

    let operation = super::open_with::wide_nul_terminated("open");
    let file = super::open_with::wide_nul_terminated("explorer.exe");
    let parameters =
        super::open_with::wide_nul_terminated(&windows_explorer_select_parameters(path));
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    }
}

/// Build the `/select,"<path>"` parameter string
/// [`windows_explorer_select_code`] hands `explorer.exe`, split out (pure
/// string formatting, no WinAPI) so it is unit-testable on every host —
/// the FFI call itself is not. Mirrors
/// [`super::open_with::shell_execute_reports_success`]'s "no WinAPI, so it runs everywhere"
/// posture: unconditionally compiled under `feature = "tui"` rather than
/// `cfg(windows)`-gated, so `ci.yml`'s ubuntu-leg `cargo test --features
/// tui` exercises it too.
///
/// Gated on `any(windows, test)` for the same reason
/// [`super::open_with::shell_execute_reports_success`] is: its only
/// non-test caller ([`windows_explorer_select_code`]) is Windows-only, but
/// the `test` alternative keeps this a real, unit-testable item on every
/// host.
#[cfg(feature = "tui")]
#[cfg(any(windows, test))]
fn windows_explorer_select_parameters(path: &std::path::Path) -> String {
    // Strip a trailing separator before quoting: an odd run of
    // backslashes immediately before the closing `"` escapes that quote
    // rather than closing the string under MSVCRT-style argv splitting
    // (see this function's doc comment above its caller,
    // `windows_explorer_select_code`, for the full reasoning). Ordinary
    // file/directory paths never carry one, so this is a defensive
    // no-op in practice, not a behavior change for real callers.
    let mut rendered = path.display().to_string();
    while rendered.len() > 1 && (rendered.ends_with('\\') || rendered.ends_with('/')) {
        rendered.pop();
    }
    format!("/select,\"{rendered}\"")
}

/// Build the `dbus-send` invocation for
/// `org.freedesktop.FileManager1.ShowItems` — not yet
/// spawned, so a test can assert on the constructed command without a
/// live D-Bus session. Drives the same standard method
/// `gtk::services::GtkPlatformServices::reveal_in_file_manager` reaches
/// through a linked `gio::DBusConnection::call_sync`, at the CLI-tool
/// level instead — see [`reveal_in_file_manager`]'s doc for why TUI can't
/// link `gio` directly.
///
/// `None` when `path` has no well-defined `file://` URI — see
/// [`path_to_file_uri`] — because `ShowItems` takes URIs and there is no
/// honest invocation to build without one.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
fn linux_show_items_via_dbus_command(path: &std::path::Path) -> Option<std::process::Command> {
    // `PlatformServices::reveal_in_file_manager` is public and forwards
    // the consumer's `&Path` verbatim, so a relative path reaches here.
    // Bail out rather than substituting an empty URI: `array:string:`
    // with an empty value is a one-element array holding the empty
    // string, which a session that *does* have a `FileManager1` listener
    // answers with a success exit — a false `Ok(())` for a reveal that
    // never happened. Reporting `None` lets the caller fall through to
    // the `xdg-open`-the-parent leg instead.
    let uri = path_to_file_uri(path)?;
    let mut cmd = std::process::Command::new("dbus-send");
    cmd.args([
        "--session",
        "--dest=org.freedesktop.FileManager1",
        "--type=method_call",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1.ShowItems",
    ])
    .arg(format!("array:string:{uri}"))
    .arg("string:")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    Some(cmd)
}

/// Run [`linux_show_items_via_dbus_command`], reporting whether
/// `dbus-send` itself exited successfully — `dbus-send` reports a
/// non-zero exit for a D-Bus-level failure (no session bus, no
/// `FileManager1` listener) exactly the way a failed native
/// `gio::DBusConnection::call_sync` would, so an exit-status check is
/// enough; there is no reply payload this call needs to read back.
///
/// `false` when there is no command to run at all (an un-URI-able path),
/// which reads the same way to the caller as a failed D-Bus call: this
/// leg didn't reveal anything, try the next one.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
fn linux_show_items_via_dbus(path: &std::path::Path) -> bool {
    linux_show_items_via_dbus_command(path)
        .and_then(|mut cmd| cmd.status().ok())
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Percent-encode `path` into a `file://` URI (RFC 8089), honoring the
/// host's native path flavor rather than assuming POSIX:
///
/// - **Unix** (including macOS): every byte outside RFC 3986's
///   unreserved set (`ALPHA` / `DIGIT` / `-._~`) plus the path separator
///   `/` is escaped as `%XX`. Operates on the path's raw `OsStr` bytes,
///   not a lossy `&str` conversion, so it is lossless for a path with no
///   valid UTF-8 representation. In particular escapes `,` (which would
///   otherwise be misread as a `dbus-send` `array:string:` element
///   separator — [`linux_show_items_via_dbus_command`]'s use) and
///   whitespace, both of which a real filesystem path can easily carry.
/// - **Windows**: routed through [`file_uri_from_windows_path_str`] for
///   drive-letter (`C:\a\b.txt` -> `file:///C:/a/b.txt`) and UNC
///   (`\\server\share\f` -> `file://server/share/f`) handling — a raw
///   `file://{backslash-path}` concatenation is not a parseable URI.
///
/// Returns `None` for a path with no well-defined `file://` form: a
/// relative path (no leading `/` on Unix, no drive letter or UNC prefix
/// on Windows) would otherwise render with its first path segment read
/// back as the URI *authority* instead of as part of the path. A host
/// that is neither Unix nor Windows has no native path flavor this crate
/// knows how to render and gets `None` for everything.
///
/// [`gtk::services::GtkPlatformServices::reveal_in_file_manager`] gets
/// the Unix encoding for free from `gio::File::for_path(path).uri()`;
/// TUI has no `gio` to call, so this is the hand-rolled equivalent, also
/// reused unconditionally by [`crate::drag::DragOutPayload::to_uri_list`]
/// — which is why this function itself carries no backend/feature `cfg`.
pub(crate) fn path_to_file_uri(path: &std::path::Path) -> Option<String> {
    #[cfg(windows)]
    {
        file_uri_from_windows_path_str(&path.to_string_lossy())
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let bytes = path.as_os_str().as_bytes();
        if !bytes.starts_with(b"/") {
            return None;
        }
        let mut out = String::from("file://");
        for byte in bytes {
            percent_encode_byte(&mut out, *byte, true);
        }
        Some(out)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        None
    }
}

/// Percent-encode one byte into `out` per RFC 3986's unreserved set
/// (`ALPHA` / `DIGIT` / `-._~`); `keep_slash` additionally passes `/`
/// through literally, for a caller encoding a whole `/`-separated path
/// body in one pass rather than per-segment.
fn percent_encode_byte(out: &mut String, b: u8, keep_slash: bool) {
    let is_unreserved = b.is_ascii_alphanumeric()
        || matches!(b, b'-' | b'.' | b'_' | b'~')
        || (keep_slash && b == b'/');
    if is_unreserved {
        out.push(b as char);
    } else {
        out.push_str(&format!("%{b:02X}"));
    }
}

/// Percent-encode a single Windows path segment (no `/` passthrough —
/// the caller has already split on separators).
///
/// Gated on `any(windows, test)`: [`path_to_file_uri`] only reaches this
/// (via [`file_uri_from_windows_path_str`]) inside its own `cfg(windows)`
/// arm, but `file_uri_tests` calls [`file_uri_from_windows_path_str`]
/// directly and unconditionally, so the `test` alternative keeps this a
/// real, unit-testable item on every host.
#[cfg(any(windows, test))]
fn encode_windows_segment(out: &mut String, segment: &str) {
    for b in segment.bytes() {
        percent_encode_byte(out, b, false);
    }
}

/// Pure string-level translation of a Windows-flavored path string into
/// its `file://` form — factored out of [`path_to_file_uri`] (rather
/// than operating on a host `&Path`) so the drive-letter and UNC shapes
/// can be pinned by a test on any host, not only a real Windows one:
/// `Path`'s own component parsing is host-dependent (a Linux host treats
/// a backslash as an ordinary filename byte, not a separator), so
/// driving this logic through `Path` there would silently test nothing.
///
/// - A UNC prefix (`\\server\share\path`, or `//server/share/path`)
///   becomes `file://server/share/path` — the server name is the URI
///   authority.
/// - A drive letter (`C:\a\b.txt`, or `C:/a/b.txt`) becomes
///   `file:///C:/a/b.txt` — empty authority, drive letter as the first
///   path segment, and a bare drive root (`C:\`) keeps its trailing
///   slash (`file:///C:/`). The colon is kept literal (RFC 8089
///   Appendix A convention) rather than percent-encoded.
/// - A verbatim prefix (`\\?\C:\a`, `\\?\UNC\server\share\f`) is
///   stripped first and the remainder classified as the drive-letter or
///   UNC form it names. This is the shape `std::fs::canonicalize`
///   returns on Windows, and therefore the obvious way a caller
///   satisfies this function's absolute-path requirement.
/// - Anything else (no drive letter, no UNC prefix — a relative path)
///   returns `None`; see [`path_to_file_uri`]'s doc for why.
///
/// Gated on `any(windows, test)` for the same reason
/// [`encode_windows_segment`] is: only [`path_to_file_uri`]'s `cfg(windows)`
/// arm calls this in production, but `file_uri_tests` calls it directly
/// and unconditionally.
#[cfg(any(windows, test))]
fn file_uri_from_windows_path_str(raw: &str) -> Option<String> {
    // `\\?\UNC\` names a UNC path whose body no longer carries the
    // leading `\\`, so remember that the UNC branch still applies;
    // without this pre-pass both verbatim forms take the UNC branch
    // below with `?` read back as the server name.
    let (raw, verbatim_unc) = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        (rest, true)
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        (rest, false)
    } else {
        (raw, false)
    };

    let unc = if verbatim_unc {
        Some(raw)
    } else {
        raw.strip_prefix(r"\\").or_else(|| raw.strip_prefix("//"))
    };
    if let Some(unc) = unc {
        let mut parts = unc.splitn(2, ['\\', '/']);
        let server = parts.next().filter(|s| !s.is_empty())?;
        let share_and_path = parts.next().unwrap_or("");

        let mut out = String::from("file://");
        encode_windows_segment(&mut out, server);
        for segment in share_and_path.split(['\\', '/']).filter(|s| !s.is_empty()) {
            out.push('/');
            encode_windows_segment(&mut out, segment);
        }
        return Some(out);
    }

    let mut chars = raw.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }

    let mut out = format!("file:///{drive}:/");
    for (i, segment) in raw[2..]
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        if i > 0 {
            out.push('/');
        }
        encode_windows_segment(&mut out, segment);
    }
    Some(out)
}

/// Covers [`path_to_file_uri`] and its Windows string-level half. Not
/// gated on `feature = "tui"` — unlike
/// [`reveal_in_file_manager_tests`], whose subjects are TUI-only — so
/// these also run under the real-Windows command
/// (`tools/win-test.sh`, `--features win`), the host they describe.
#[cfg(test)]
mod file_uri_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn path_to_file_uri_percent_encodes_spaces_and_commas() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new("/tmp/a,b c/d.txt")),
            Some("file:///tmp/a%2Cb%20c/d.txt".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_to_file_uri_is_none_for_a_relative_path() {
        // A relative path has no well-defined `file://` form: rendering
        // it anyway would read its first segment back as the URI
        // authority instead of as part of the path.
        assert_eq!(path_to_file_uri(std::path::Path::new("a.txt")), None);
    }

    #[cfg(windows)]
    #[test]
    fn path_to_file_uri_renders_a_windows_drive_letter_path() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new(r"C:\dir\a b.txt")),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
        assert_eq!(path_to_file_uri(std::path::Path::new(r"dir\a.txt")), None);
    }

    /// Pure string-level logic, no WinAPI — runs on every host, same
    /// posture as
    /// [`reveal_in_file_manager_tests::windows_explorer_select_parameters_quotes_the_path`].
    #[test]
    fn file_uri_from_windows_path_str_handles_a_drive_letter() {
        assert_eq!(
            file_uri_from_windows_path_str(r"C:\dir\a b.txt"),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_keeps_a_bare_drive_root_slash() {
        assert_eq!(
            file_uri_from_windows_path_str(r"C:\"),
            Some("file:///C:/".to_string())
        );
        assert_eq!(
            file_uri_from_windows_path_str("C:"),
            Some("file:///C:/".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_handles_a_unc_share() {
        assert_eq!(
            file_uri_from_windows_path_str(r"\\server\share\a b.txt"),
            Some("file://server/share/a%20b.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_strips_a_verbatim_prefix() {
        // `std::fs::canonicalize` hands back these forms on Windows.
        assert_eq!(
            file_uri_from_windows_path_str(r"\\?\C:\dir\a b.txt"),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
        assert_eq!(
            file_uri_from_windows_path_str(r"\\?\UNC\server\share\f.txt"),
            Some("file://server/share/f.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_is_none_for_a_relative_path() {
        assert_eq!(file_uri_from_windows_path_str(r"dir\a.txt"), None);
    }
}

#[cfg(all(test, feature = "tui"))]
mod reveal_in_file_manager_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reveal_in_file_manager_command_uses_open_dash_r() {
        let cmd = macos_reveal_in_file_manager_command(std::path::Path::new("/tmp/quadraui1092"));
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["-R", "/tmp/quadraui1092"]
        );
    }

    /// Pure string formatting, no WinAPI — runs on every host, including
    /// `ci.yml`'s ubuntu-leg `cargo test --features tui`, same posture
    /// [`super::open_with::shell_execute_reports_success_tests`]'s own doc explains.
    /// `Path::new(r"C:\a b\c.txt")` renders identically via `.display()`
    /// on Unix (one opaque path component, backslashes preserved as
    /// literal bytes) and on Windows (a real backslash-separated path),
    /// so the expected output is the same on either host.
    #[test]
    fn windows_explorer_select_parameters_quotes_the_path() {
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\a b\c.txt")),
            r#"/select,"C:\a b\c.txt""#
        );
    }

    #[test]
    fn windows_explorer_select_parameters_strips_a_trailing_separator() {
        // A path ending in a backslash right before the closing quote
        // would otherwise escape that quote under MSVCRT-style argv
        // splitting instead of closing the string.
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\dir\")),
            r#"/select,"C:\dir""#
        );
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\a b\c.txt")),
            r#"/select,"C:\a b\c.txt""#
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_show_items_via_dbus_command_targets_file_manager1() {
        let cmd = linux_show_items_via_dbus_command(std::path::Path::new("/tmp/quadraui 1092"))
            .expect("an absolute path has a file:// URI to show");
        assert_eq!(cmd.get_program(), "dbus-send");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                "array:string:file:///tmp/quadraui%201092",
                "string:",
            ]
        );
    }

    /// A relative path has no `file://` URI, so there is no `ShowItems`
    /// invocation to build; the caller reads that as "this leg found
    /// nothing" and falls through to the `xdg-open`-the-parent leg
    /// rather than sending a one-element array holding the empty string.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_show_items_via_dbus_command_is_none_for_a_relative_path() {
        assert!(linux_show_items_via_dbus_command(std::path::Path::new("a.txt")).is_none());
    }

    /// Acceptance bar: when neither the D-Bus leg nor the
    /// `xdg-open`-the-parent fallback can reach anything (`$PATH`
    /// redirected to an empty directory), `reveal_in_file_manager` honestly
    /// reports `Unsupported` rather than a false `Ok(())`. Doesn't exercise
    /// the D-Bus leg for real (that needs a live session bus this test
    /// environment may not have) — only pins the "both legs failed" outcome
    /// via the `$PATH` stub, which is the fallback this crate can actually
    /// control in a test.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn reveal_in_file_manager_reports_unsupported_when_every_leg_fails() {
        let _guard = super::super::open_with::PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
        let _restore = RestorePath(std::env::var_os("PATH"));
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        let target = std::env::temp_dir().join("quadraui-1092-no-such-file.txt");
        // `dbus-send` itself is very unlikely to be on an empty `$PATH`
        // (it's the binary this leg spawns), so this exercises the "not
        // even `dbus-send` present" corner of the D-Bus leg alongside the
        // `xdg-open`-the-parent fallback's own missing-opener leg.
        assert_eq!(
            reveal_in_file_manager(&target),
            Err(crate::backend::BackendError::Unsupported)
        );
    }
}
