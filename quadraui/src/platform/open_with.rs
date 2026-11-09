// ─────────────────────────────────────────────────────────────────────
// wide_nul_terminated
// ─────────────────────────────────────────────────────────────────────

/// UTF-16, NUL-terminated — the framing every wide (`W`-suffixed) Win32
/// API expects (`ShellExecuteW`'s `operation`/`file`, `CF_UNICODETEXT`'s
/// clipboard payload, `NOTIFYICONDATAW`'s fixed-size fields, a context
/// menu item's label). The single implementation shared by
/// `win::services`, `win::tray`, `win::backend`, and this module's own
/// [`windows_shell_execute_open`] — before this, each of the first three
/// modules carried its own private copy of this exact two-line
/// conversion, and [`windows_shell_execute_open`] needed a fourth.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`: every
/// real caller only exists under `target_os = "windows"` (the WinAPI
/// call sites this feeds, and this module's own Windows-only opener), so
/// on any other host this is unreachable dead code the moment it
/// compiles at all under `--features win` alone.
#[cfg(any(feature = "tui", feature = "win"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wide_nul_terminated(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(all(test, any(feature = "tui", feature = "win")))]
mod wide_nul_terminated_tests {
    use super::*;

    #[test]
    fn encodes_utf16_and_appends_a_nul() {
        assert_eq!(
            wide_nul_terminated("ab"),
            vec!['a' as u16, 'b' as u16, 0u16]
        );
        assert_eq!(wide_nul_terminated(""), vec![0u16]);
    }
}

// ─────────────────────────────────────────────────────────────────────
// open_with_default
// ─────────────────────────────────────────────────────────────────────

/// Open `target` (a URL string or a filesystem path, carried as `&OsStr`
/// so a non-UTF-8 path need not be lossily converted before the call) —
/// the single opener implementation shared by `tui::services`, `macos::services`,
/// and `win::services`.
///
/// Unix: spawns `open` (macOS) / `xdg-open` (other Unix), detached
/// (stdout/stderr nulled so a slow or chatty opener can't wedge the
/// caller's own streams — matters most for `tui`, which shares stdout
/// with the terminal UI) and never through a shell — `execve`, so
/// `target` is never re-parsed for shell metacharacters. Windows:
/// `ShellExecuteW(NULL, "open", target, NULL, NULL, SW_SHOWNORMAL)`
/// through a **minimal, hand-written FFI declaration**, not the
/// `windows` crate — `windows` is `optional`, pulled in only by the
/// `win` feature's `dep:windows`, so it is not a dependency of a bare
/// `--features tui` build, and this must compile there too. Never goes
/// through `cmd.exe`: the `cmd /c start` approach is a CWE-78
/// command-injection hole, since `cmd.exe` re-parses its own command-line
/// text for `&`/`|`/`^`/`%` regardless of how the argument was quoted for
/// `CreateProcess`.
#[cfg(any(
    feature = "tui",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn open_with_default(target: &std::ffi::OsStr) -> crate::backend::ServiceResult<()> {
    if try_open_with_default(target) {
        Ok(())
    } else {
        Err(crate::backend::BackendError::PlatformFailure {
            context: format!("open_with_default: no opener launched for {target:?}"),
        })
    }
}

/// The "did the platform opener genuinely launch" step [`open_with_default`]
/// wraps into a [`crate::backend::ServiceResult`], exposed separately
/// (`pub(crate)`, not folded into `open_with_default` itself) because
/// `tui::services`'s `open_url_via` needs the raw bool to chain its own
/// OSC 8 hyperlink fallback when this returns `false` —
/// `open_with_default`'s `Err` shape has nowhere to carry that extra
/// step.
#[cfg(any(
    feature = "tui",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn try_open_with_default(target: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        windows_shell_execute_open(target)
    }
    #[cfg(not(windows))]
    {
        unix_open_with_default_command(target).spawn().is_ok()
    }
}

/// macOS opener command, not yet spawned — factored out so a test can
/// assert on the program name and arguments a real call would spawn
/// without actually launching anything. See the Linux/BSD overload of
/// this same function (below) for the shared shape and doc.
#[cfg(all(target_os = "macos", any(feature = "tui", feature = "macos")))]
pub(crate) fn unix_open_with_default_command(target: &std::ffi::OsStr) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Linux/BSD opener command. See the macOS overload of this same
/// function (above) for the shared doc.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
pub(crate) fn unix_open_with_default_command(target: &std::ffi::OsStr) -> std::process::Command {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Windows opener: `ShellExecuteW`, called through a minimal hand-written
/// FFI declaration rather than the `windows` crate — see
/// [`open_with_default`]'s doc for why. Returns whether `ShellExecuteW`
/// reports success, per [`shell_execute_reports_success`]'s `> 32` rule.
#[cfg(all(windows, any(feature = "tui", feature = "win")))]
fn windows_shell_execute_open(target: &std::ffi::OsStr) -> bool {
    shell_execute_reports_success(windows_shell_execute_open_code(target))
}

/// `ShellExecuteW`'s documented success test, split out from the FFI call
/// itself ([`windows_shell_execute_open_code`]) so it is unit-testable on
/// *every* host — the FFI call is not.
///
/// Per `ShellExecuteW`'s own docs the return value is "greater than 32 if
/// successful, or an error value that is less than or equal to 32
/// otherwise" (e.g. `SE_ERR_FNF` = 2, `SE_ERR_NOASSOC` = 31). `32` itself
/// is the boundary and is a *failure* (`SE_ERR_DDEFAIL`), which is the
/// off-by-one this predicate exists to pin down.
///
/// Pure arithmetic with no WinAPI in it, exactly like `win::msg`'s
/// `WM_SIZE` unpacking — only [`windows_shell_execute_open_code`], the
/// part that genuinely calls into `shell32`, stays Windows-only. Gated on
/// `any(target_os = "windows", test)` rather than `cfg(windows)` alone:
/// its only non-test caller is the Windows-only
/// [`windows_shell_execute_open`], but the `test` alternative keeps this a
/// real, unit-testable item under `ci.yml`'s ubuntu-leg
/// `cargo test -p quadraui --features win` too.
#[cfg(any(feature = "tui", feature = "win"))]
#[cfg(any(target_os = "windows", test))]
pub(crate) fn shell_execute_reports_success(code: isize) -> bool {
    code > 32
}

#[cfg(all(test, any(feature = "tui", feature = "win")))]
mod shell_execute_reports_success_tests {
    use super::*;

    /// [`shell_execute_reports_success`] is `ShellExecuteW`'s documented
    /// "greater than 32" rule, and the boundary is the whole reason it's a
    /// named function rather than an inline comparison: `32` itself
    /// (`SE_ERR_DDEFAIL`) is a *failure*, so a `>=` here would silently
    /// turn one real failure code into a reported success. Pure
    /// arithmetic, no WinAPI — so this runs on every host, including
    /// `ci.yml`'s ubuntu-leg `cargo test -p quadraui --features win`,
    /// where nothing else in the Windows opener compiles at all.
    #[test]
    fn success_threshold_is_strictly_above_32() {
        const SE_ERR_FNF: isize = 2;
        const SE_ERR_NOASSOC: isize = 31;
        const SE_ERR_DDEFAIL: isize = 32;

        assert!(!shell_execute_reports_success(0));
        assert!(!shell_execute_reports_success(SE_ERR_FNF));
        assert!(!shell_execute_reports_success(SE_ERR_NOASSOC));
        assert!(!shell_execute_reports_success(SE_ERR_DDEFAIL));
        assert!(shell_execute_reports_success(33));
        assert!(shell_execute_reports_success(42));
    }
}

/// The raw `ShellExecuteW(NULL, "open", target, NULL, NULL,
/// SW_SHOWNORMAL)` return value. Split from
/// [`windows_shell_execute_open`] so a test can assert on the actual
/// documented `SE_ERR_*` code a given target produces on a real host;
/// callers outside this module want the `bool`.
#[cfg(all(windows, any(feature = "tui", feature = "win")))]
fn windows_shell_execute_open_code(target: &std::ffi::OsStr) -> isize {
    // SAFETY: `ShellExecuteW` is a well-known, stable Win32 API. Both
    // wide-string buffers passed below are nul-terminated and kept alive
    // (as local `Vec<u16>`s) for the duration of the call; the remaining
    // arguments are the documented "no window handle / no extra
    // parameters / no explicit working directory" null pointers.
    //
    // `#[link(name = "shell32")]`: `ShellExecuteW` lives in
    // `shell32.dll`/`shell32.lib` — unlike the `kernel32`/`user32`
    // imports the MSVC CRT startup pulls in implicitly, this one needs an
    // explicit link directive since nothing else in a bare `tui`-feature
    // build references `shell32` at all.
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

    let operation = wide_nul_terminated("open");
    let file = wide_nul_terminated(&target.to_string_lossy());
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    }
}

/// Process-wide `$PATH` is shared, mutable global state — every test
/// below (and `macos::services`'s `open_url_result` failure test) that
/// temporarily redirects it to prove a real "opener missing" failure must
/// serialize against every *other* such test in the same process, or a
/// concurrently-running test can observe (or clobber) the override. One
/// crate-wide lock rather than a per-module copy so tests under different
/// feature combinations can't race each other.
///
/// **`unix`-gated, deliberately** — every test that takes this lock is a
/// `$PATH`-override test, and the `$PATH`-stub technique only applies to
/// [`unix_open_with_default_command`]'s arm: Windows' opener
/// ([`windows_shell_execute_open`]) is a direct `ShellExecuteW` FFI call
/// with no `$PATH`-resolved binary to intercept, so no Windows test has
/// any reason to hold this. Without the `unix` here, a
/// `cargo test --features tui` on a Windows host compiles a lock with
/// zero call sites — `dead_code`, which `ci.yml`'s workflow-wide
/// `RUSTFLAGS: -D warnings` promotes to a hard error on the
/// `windows-latest` leg of the `tui` job alone, while every Linux and
/// macOS build of the identical source stays green.
#[cfg(all(
    test,
    unix,
    any(feature = "tui", all(feature = "macos", target_os = "macos"))
))]
pub(crate) static PATH_OVERRIDE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(
    test,
    any(
        feature = "tui",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )
))]
mod open_with_default_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn unix_open_with_default_command_uses_macos_open() {
        let cmd = unix_open_with_default_command(std::ffi::OsStr::new("https://example.com/1087"));
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["https://example.com/1087"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn unix_open_with_default_command_uses_xdg_open() {
        let cmd = unix_open_with_default_command(std::ffi::OsStr::new("https://example.com/1087"));
        assert_eq!(cmd.get_program(), "xdg-open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["https://example.com/1087"]
        );
    }

    /// A target that can never resolve, chosen so the real
    /// `ShellExecuteW` call fails *without* handing anything to the shell:
    /// an absolute path under a directory that does not exist, with an
    /// extension no handler is registered for. Verified on a real Windows
    /// host to return `SE_ERR_FNF` (2) immediately, with no UI.
    ///
    /// Deliberately **not** an unregistered URI scheme. That was this
    /// module's first attempt (`quadraui-1087-nonexistent-scheme://x`) and
    /// it is simply wrong on modern Windows: `ShellExecuteW` hands an
    /// unknown scheme off to the shell's "how do you want to open this?"
    /// / Store-lookup flow and returns **42 — success** — before any of
    /// that resolves, so the call reports success for a launch that never
    /// happens. It red-lined `ci.yml`'s "Test (win feature, real Windows)"
    /// step, which is the only leg that executes these `cfg(windows)`
    /// arms at all. Don't reintroduce it.
    #[cfg(windows)]
    fn unopenable_windows_target() -> std::ffi::OsString {
        std::ffi::OsString::from(format!(
            "C:\\quadraui-1087-no-such-directory-{}\\x.quadraui1087nohandler",
            std::process::id()
        ))
    }

    /// The real FFI call's real verdict on a target that cannot open:
    /// `SE_ERR_FNF`, which [`shell_execute_reports_success`] then reports
    /// as `false`. This is the Windows half of the "stop lying about
    /// the launch" bar — the `$PATH`-stub tests below cover the Unix arm,
    /// which has a spawned binary to intercept and Windows does not.
    #[cfg(windows)]
    #[test]
    fn windows_shell_execute_open_reports_a_real_failure_code() {
        const SE_ERR_FNF: isize = 2;
        let target = unopenable_windows_target();
        assert_eq!(windows_shell_execute_open_code(&target), SE_ERR_FNF);
        assert!(!windows_shell_execute_open(&target));
    }

    /// `open_with_default`'s `ServiceResult` wrapping on Windows — the
    /// same bar `open_with_default_reports_err_when_the_opener_is_missing`
    /// sets on Unix, reached through a real `ShellExecuteW` call instead
    /// of a `$PATH` stub.
    #[cfg(windows)]
    #[test]
    fn open_with_default_reports_err_when_windows_cannot_open_the_target() {
        let target = unopenable_windows_target();
        assert_eq!(
            open_with_default(&target),
            Err(crate::backend::BackendError::PlatformFailure {
                context: format!("open_with_default: no opener launched for {target:?}"),
            })
        );
    }

    /// [`windows_shell_execute_open`] never builds a `std::process::Command`
    /// (that's the whole point — no `cmd.exe`/shell in the loop at all),
    /// so there is no command to assert on beyond invoking `ShellExecuteW`
    /// for real. What this pins down: the exact metacharacters (`&`, `|`,
    /// `^`, `%`) that made the original `cmd /c start` approach
    /// exploitable are carried through inertly — a
    /// bare FFI call has no command-line text for those bytes to land in,
    /// so they stay part of the *filename* and the call fails with
    /// `SE_ERR_FNF` rather than executing `calc.exe`.
    ///
    /// The target is an unopenable path (see
    /// [`unopenable_windows_target`]) rather than the `https://` URL this
    /// test first used: a real URL launches the host's actual browser on
    /// every CI run and on every developer's desktop, which is both a
    /// side effect a unit test has no business having and a way to wedge
    /// the run behind a shell dialog.
    #[cfg(windows)]
    #[test]
    fn windows_shell_execute_open_treats_shell_metacharacters_as_inert_text() {
        const SE_ERR_FNF: isize = 2;
        let target = std::ffi::OsString::from(format!(
            "C:\\quadraui-1087-no-such-directory-{}\\q=foo&run=bar|calc.exe^%1.quadraui1087nohandler",
            std::process::id()
        ));
        assert_eq!(windows_shell_execute_open_code(&target), SE_ERR_FNF);
    }

    /// Acceptance bar: `try_open_with_default`/`open_with_default`
    /// through a real delegation — the same `$PATH`-override stub
    /// technique generalised from a previous test. Safe to do without
    /// launching a real browser only because `$PATH` is temporarily
    /// redirected (guarded by [`PATH_OVERRIDE_TEST_LOCK`], and always
    /// restored via a drop guard even on panic) to a directory containing
    /// a stub executable under the exact name
    /// [`unix_open_with_default_command`] looks up on this platform
    /// (`open` on macOS, `xdg-open` elsewhere on Unix) that exits `0`
    /// immediately.
    ///
    /// **Unix only** — Windows' opener ([`windows_shell_execute_open`])
    /// is a direct `ShellExecuteW` FFI call with no `$PATH`-resolved
    /// binary to intercept this way; see
    /// `windows_shell_execute_open_does_not_panic_on_shell_metacharacters`
    /// above for that platform's own coverage of the real function, and
    /// `win::services::open_url_result_reports_failure_for_an_unregistered_scheme`
    /// for its `ServiceResult`-level failure coverage.
    #[cfg(unix)]
    #[test]
    fn try_open_with_default_returns_true_through_a_real_delegation() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        #[cfg(target_os = "macos")]
        const OPENER_NAME: &str = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        const OPENER_NAME: &str = "xdg-open";

        let tmp = tempfile::tempdir().expect("tempdir");
        let stub_path = tmp.path().join(OPENER_NAME);
        std::fs::write(&stub_path, b"#!/bin/sh\nexit 0\n").expect("write stub opener");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x stub opener");
        }
        let _restore = RestorePath::capture();
        std::env::set_var("PATH", tmp.path());

        assert!(try_open_with_default(std::ffi::OsStr::new(
            "https://example.com/1087"
        )));
    }

    /// The other half of the acceptance bar: with `$PATH` redirected
    /// to a directory containing *no* opener binary at all, the real
    /// spawn genuinely fails (`ENOENT`) and `try_open_with_default`
    /// honestly reports `false`. See `open_with_default_reports_err_when_the_opener_is_missing`
    /// below for the `ServiceResult`-level half of this — `open_with_default`
    /// itself is also compiled for a bare `--features tui` build (it backs
    /// `TuiPlatformServices::open_path`), even though `open_url_result`
    /// still calls `try_open_with_default` directly there so it can chain
    /// its own OSC 8 fallback.
    #[cfg(unix)]
    #[test]
    fn try_open_with_default_returns_false_when_the_opener_is_missing() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _restore = RestorePath::capture();
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        assert!(!try_open_with_default(std::ffi::OsStr::new(
            "https://example.com/1087"
        )));
    }

    /// `open_with_default`'s own `ServiceResult` wrapping, via the `$PATH`
    /// stub technique — Unix only: this technique needs a `$PATH`-resolved
    /// binary to intercept, which only applies to `open_with_default`'s
    /// Unix arm. Runs under either `macos` or bare `tui`. `win::services`'s
    /// own `ServiceResult`-level coverage uses a real `ShellExecuteW` call
    /// instead, since Windows' opener has no `$PATH`-resolved binary to
    /// intercept this way at all.
    #[cfg(any(
        all(feature = "macos", target_os = "macos"),
        all(feature = "tui", unix)
    ))]
    #[test]
    fn open_with_default_reports_err_when_the_opener_is_missing() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _restore = RestorePath::capture();
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        assert_eq!(
            open_with_default(std::ffi::OsStr::new("https://example.com/1087")),
            Err(crate::backend::BackendError::PlatformFailure {
                context: "open_with_default: no opener launched for \"https://example.com/1087\""
                    .to_string(),
            })
        );
    }

    /// RAII guard restoring the process-wide `$PATH` this test module
    /// temporarily overrides, even on an early return or panic-unwind —
    /// same "always restore, drop-guard" posture
    /// [`crate::desktop::ModalPumpGuard`] and this crate's other
    /// global-state test helpers use.
    #[cfg(unix)]
    struct RestorePath(Option<std::ffi::OsString>);

    #[cfg(unix)]
    impl RestorePath {
        fn capture() -> Self {
            Self(std::env::var_os("PATH"))
        }
    }

    #[cfg(unix)]
    impl Drop for RestorePath {
        fn drop(&mut self) {
            match self.0.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}
