// ─────────────────────────────────────────────────────────────────────
// Headless smoke-mode config + predicates
// ─────────────────────────────────────────────────────────────────────

/// Headless smoke-mode config. `None` unless the `ms`-suffixed env var
/// [`Self::from_env`] is asked to read is set — see `gtk::run`'s module
/// doc's "Headless smoke mode" section for the full motivating rationale
/// (a live-window class of bug a display-free driver test structurally
/// can't catch).
///
/// `#[cfg(any(feature = "gtk", all(feature = "win", any(target_os =
/// "windows", test))))]` — same reasoning as [`ModalPumpDepth`]'s gate
/// note: `win::run`'s real smoke lane only exists once
/// `target_os = "windows"` also compiles `mod win32`, but the `test`
/// alternative keeps this type (and its pure predicates below) a real,
/// unit-testable consumer under a plain `cargo test --features win` on
/// any host.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SmokeConfig {
    /// Delay after the window is presented before the one-shot check
    /// fires and the window is closed.
    pub(crate) after_ms: u64,
    /// Optional text to round-trip through the real OS clipboard and
    /// replay as a synthetic paste.
    pub(crate) paste_text: Option<String>,
}

#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
impl SmokeConfig {
    /// Reads the smoke-mode env vars once. `ms_var` is the env var that
    /// enables smoke mode (e.g. `QUADRAUI_GTK_SMOKE_MS`) — returns
    /// `None` (the default — zero behavioral change) unless it's set and
    /// parses as a `u64`. `paste_var` (e.g. `QUADRAUI_GTK_SMOKE_PASTE`)
    /// is optional; if set, its value round-trips through the paste
    /// path. Parameterised by name (rather than hardcoding the `GTK`
    /// infix) so a future backend can reuse this under its own env-var
    /// names without colliding with GTK's.
    pub(crate) fn from_env(ms_var: &str, paste_var: &str) -> Option<Self> {
        let after_ms = std::env::var(ms_var).ok()?.parse().ok()?;
        let paste_text = std::env::var(paste_var).ok();
        Some(Self {
            after_ms,
            paste_text,
        })
    }
}

/// Is `width`x`height` a plausible, non-broken window/surface
/// allocation? The direct regression check for a tiny/wrapped-window bug
/// class. Pure and display-free. Each backend supplies its own floor
/// (`min_width`/`min_height`) — its own default window size and how far
/// below it is still "plausible" — this function only owns the comparison.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn smoke_size_ok(width: i32, height: i32, min_width: i32, min_height: i32) -> bool {
    width >= min_width && height >= min_height
}

/// Did the OS clipboard round-trip `written` back byte-for-byte? Pure
/// comparison, factored out so the pass/fail rule is unit-testable
/// without a real clipboard.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn smoke_clipboard_round_trip_ok(written: &str, read_back: Option<&str>) -> bool {
    read_back == Some(written)
}

#[cfg(all(test, any(feature = "gtk", feature = "win")))]
mod smoke_config_tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // `env::set_var`/`remove_var` are process-global; serialise this
    // module's tests against each other so they don't race (matches the
    // pattern `gtk::run`'s own smoke tests didn't need, since those only
    // tested the pure predicates — `from_env` itself is new coverage
    // here). Shared by both the GTK and `win` adopters — the env
    // var names below are test-local sentinels, not either backend's real
    // `QUADRAUI_*_SMOKE_MS` names, so there's no cross-backend collision
    // risk running both features' tests in the same process.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_none_when_ms_var_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_UNSET";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_UNSET";
        env::remove_var(ms_var);
        env::remove_var(paste_var);
        assert_eq!(SmokeConfig::from_env(ms_var, paste_var), None);
    }

    #[test]
    fn from_env_none_when_ms_var_unparseable() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_BAD";
        env::set_var(ms_var, "not-a-number");
        assert_eq!(
            SmokeConfig::from_env(ms_var, "QUADRAUI_DESKTOP_TEST_UNSET"),
            None
        );
        env::remove_var(ms_var);
    }

    #[test]
    fn from_env_reads_ms_and_optional_paste() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_OK";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_OK";
        env::set_var(ms_var, "250");
        env::set_var(paste_var, "hello");
        assert_eq!(
            SmokeConfig::from_env(ms_var, paste_var),
            Some(SmokeConfig {
                after_ms: 250,
                paste_text: Some("hello".to_string()),
            })
        );
        env::remove_var(ms_var);
        env::remove_var(paste_var);
    }

    #[test]
    fn from_env_paste_none_when_paste_var_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_NOPASTE";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_NOPASTE_UNSET";
        env::remove_var(paste_var);
        env::set_var(ms_var, "10");
        assert_eq!(
            SmokeConfig::from_env(ms_var, paste_var),
            Some(SmokeConfig {
                after_ms: 10,
                paste_text: None,
            })
        );
        env::remove_var(ms_var);
    }

    #[test]
    fn smoke_size_ok_accepts_at_or_above_the_floor() {
        assert!(smoke_size_ok(200, 150, 200, 150));
        assert!(smoke_size_ok(800, 600, 200, 150));
    }

    #[test]
    fn smoke_size_ok_rejects_below_the_floor() {
        assert!(!smoke_size_ok(199, 150, 200, 150));
        assert!(!smoke_size_ok(200, 149, 200, 150));
        // Content wrapped into an ~8px-wide column.
        assert!(!smoke_size_ok(8, 600, 200, 150));
    }

    #[test]
    fn clipboard_round_trip_ok_when_read_back_matches() {
        assert!(smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("quadraui smoke")
        ));
    }

    #[test]
    fn clipboard_round_trip_rejects_a_missing_or_mismatched_read() {
        assert!(!smoke_clipboard_round_trip_ok("quadraui smoke", None));
        assert!(!smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("something else")
        ));
    }
}
