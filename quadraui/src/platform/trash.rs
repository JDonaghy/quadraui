// ─────────────────────────────────────────────────────────────────────
// move_to_trash
// ─────────────────────────────────────────────────────────────────────

/// Move `path` to the platform trash/recycle bin via the cross-platform
/// `trash` crate — the one implementation shared by every
/// `PlatformServices::move_to_trash` override
/// (`tui`/`gtk`/`macos`/`win::services`), TUI included. Unlike this
/// module's other genuinely backend-neutral helpers, this one *does*
/// wrap a native call — but the same one on every platform: `trash`
/// itself dispatches internally to `NSFileManager
/// -trashItemAtURL:resultingItemURL:error:` on macOS, the
/// freedesktop.org trash spec on Linux, and
/// `SHFileOperationW(FOF_ALLOWUNDO)` on Windows, none of which need a
/// live desktop/window-server session — only a filesystem — so there is
/// no backend-specific variation left to hand-write. See
/// [`crate::backend::PlatformServices::move_to_trash`]'s doc for why
/// that means TUI gets this exact function too, instead of the
/// `Err(BackendError::Unsupported)` degrade TUI's remaining
/// desktop-shell-flavoured methods (native dialogs, notably) still fall
/// back to — [`reveal_in_file_manager`] below is the other
/// one that has since earned a real, best-effort implementation instead
/// of that placeholder.
///
/// `#[cfg]`-gated on every adopting feature (mirroring this module's own
/// "why every item is gated" note) — `win`'s production call site is
/// itself nested inside `win::services`'s own `#[cfg(target_os =
/// "windows")]` arm (see `WinPlatformServices::move_to_trash`), so the
/// `win` alternative here needs the identical `any(target_os =
/// "windows", test)` qualifier [`ModalPumpDepth`]'s gate note explains:
/// without it, `cargo check --features win` on a non-Windows host would
/// compile this function with no caller at all and trip `-D warnings`'
/// dead-code lint. The `test` half of that `any(..)` is what keeps this a
/// real, unit-testable consumer under `cargo test --features win` on any
/// host, matching that same precedent.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn move_to_trash(path: &std::path::Path) -> crate::backend::ServiceResult<()> {
    trash::delete(path).map_err(|e| crate::backend::BackendError::PlatformFailure {
        context: format!("trash::delete: {e}"),
    })
}

// ─────────────────────────────────────────────────────────────────────
// restore_from_trash
// ─────────────────────────────────────────────────────────────────────

/// Restore `path` from the platform trash/recycle bin back to its
/// original location — the inverse of [`move_to_trash`] above, shared by
/// every `PlatformServices::restore_from_trash` override that has one
/// (`tui`/`gtk`/`win::services`; macOS deliberately has none — see
/// [`crate::backend::PlatformServices::restore_from_trash`]'s doc for
/// why).
///
/// Unlike [`move_to_trash`], this one genuinely *is* Windows/Linux-only:
/// `trash`'s `os_limited` module — the only part of that crate able to
/// list what's in the trash and restore a specific item back out of it —
/// is `#[cfg]`-gated to `target_os = "windows"` and freedesktop-ish Unix
/// (`all(unix, not(macos), not(ios), not(android))`) in the crate's own
/// `lib.rs`; there is no third branch to add here, only an honest
/// `Unsupported` on every other target this function might still get
/// compiled for (a `tui`/`gtk` build running on macOS, say — the feature
/// gate below says nothing about the host it ends up built on).
///
/// Looks up every [`trash::TrashItem`] whose `original_path()` matches
/// `path` and restores the most recently trashed one (`time_deleted`'s
/// max) — the one an "Undo" affordance right after a `move_to_trash` call
/// almost always means, even if the same path was trashed more than once
/// in this session.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn restore_from_trash(path: &std::path::Path) -> crate::backend::ServiceResult<()> {
    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    {
        let items = trash::os_limited::list().map_err(|e| {
            crate::backend::BackendError::PlatformFailure {
                context: format!("trash::os_limited::list: {e}"),
            }
        })?;
        let item = items
            .into_iter()
            .filter(|item| item.original_path() == path)
            .max_by_key(|item| item.time_deleted)
            .ok_or_else(|| crate::backend::BackendError::PlatformFailure {
                context: format!(
                    "restore_from_trash: no trashed item found for {}",
                    path.display()
                ),
            })?;
        trash::os_limited::restore_all([item]).map_err(|e| {
            crate::backend::BackendError::PlatformFailure {
                context: format!("trash::os_limited::restore_all: {e}"),
            }
        })
    }
    #[cfg(not(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    )))]
    {
        let _ = path;
        Err(crate::backend::BackendError::Unsupported)
    }
}

#[cfg(all(
    test,
    any(
        feature = "tui",
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod move_to_trash_tests {
    use super::*;

    /// Real round trip against this host's actual trash/recycle bin: a
    /// freshly-written temp file must be gone from its original path
    /// (moved, not copied) once [`move_to_trash`] reports success. Skips
    /// gracefully (rather than failing) when the underlying OS call
    /// itself fails — same posture as this crate's OS-clipboard tests on
    /// a headless display: on macOS, `trash`'s implementation goes
    /// through a Finder AppleEvent, which needs a live desktop session
    /// (and, on a *first* run, an Automation permission grant) that a
    /// sandboxed/headless dev box or CI container genuinely may not have,
    /// even though the *filesystem* `move_to_trash` itself only ever
    /// touches is always present.
    #[allow(clippy::print_stderr)]
    #[test]
    fn move_to_trash_removes_the_file_from_its_original_path() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-956-move-to-trash-test-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, b"quadraui#956").expect("write temp file");
        assert!(path.exists(), "temp file must exist before trashing it");

        if let Err(e) = move_to_trash(&path) {
            eprintln!(
                "skipping: move_to_trash failed in this environment ({e:?}) — \
                 likely no live desktop/trash session"
            );
            let _ = std::fs::remove_file(&path);
            return;
        }

        assert!(
            !path.exists(),
            "the original path must no longer exist once move_to_trash succeeds"
        );
    }

    /// A path that never existed is a real, reportable failure — not
    /// silently `Ok(())` — matching every other `PlatformServices` method
    /// in this crate that surfaces the native call's actual outcome
    /// rather than papering over it.
    #[test]
    fn move_to_trash_on_a_nonexistent_path_reports_a_failure() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-956-does-not-exist-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        assert!(!path.exists());
        assert!(move_to_trash(&path).is_err());
    }
}

#[cfg(all(test, any(feature = "tui", feature = "gtk", feature = "win")))]
mod restore_from_trash_tests {
    use super::*;

    /// Real round trip against this host's actual trash/recycle bin:
    /// [`move_to_trash`] followed by [`restore_from_trash`] must put the
    /// same content back at the original path. Skips gracefully (rather
    /// than failing) when either OS call fails in this environment — same
    /// posture as `move_to_trash_tests`'s own round-trip test, for the
    /// identical "no live desktop/trash session" reason.
    #[allow(clippy::print_stderr)]
    #[test]
    fn restore_from_trash_puts_the_file_back_after_move_to_trash() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-1924-restore-from-trash-test-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, b"quadraui#1924").expect("write temp file");

        if let Err(e) = move_to_trash(&path) {
            eprintln!(
                "skipping: move_to_trash failed in this environment ({e:?}) — \
                 likely no live desktop/trash session"
            );
            let _ = std::fs::remove_file(&path);
            return;
        }

        if let Err(e) = restore_from_trash(&path) {
            eprintln!(
                "skipping: restore_from_trash failed in this environment ({e:?}) — \
                 likely no live desktop/trash session"
            );
            return;
        }

        assert!(
            path.exists(),
            "the original path must exist again once restore_from_trash succeeds"
        );
        assert_eq!(
            std::fs::read(&path).unwrap_or_default(),
            b"quadraui#1924",
            "the restored file must carry its original content"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A path that was never trashed is a real, reportable failure — not
    /// silently `Ok(())` — matching [`move_to_trash`]'s own
    /// never-existed-path test just above.
    #[test]
    fn restore_from_trash_on_a_path_never_trashed_reports_a_failure() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-1924-never-trashed-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        assert!(!path.exists());
        assert!(restore_from_trash(&path).is_err());
    }
}
