//! Guards `build.rs`'s embedded Common-Controls v6 side-by-side manifest
//! against the comctl32 APIs `src/win/` actually calls (#744 CI fix).
//!
//! ## Why this test exists
//!
//! `%SystemRoot%\System32\comctl32.dll` is still the legacy **5.82**
//! build. Every common-controls **v6** entry point — `TaskDialogIndirect`
//! among them — exists only in the side-by-side assembly under
//! `%SystemRoot%\WinSxS\…microsoft.windows.common-controls…_6.0.*`, and a
//! binary is only bound to that assembly if its application manifest
//! declares a dependency on it.
//!
//! The `windows` crate imports those entry points **statically**. So a
//! binary that calls one without the manifest does not "fail to show a
//! dialog" — the Windows loader cannot resolve the import and **kills the
//! process before `main` runs**: no output, no panic, no backtrace, just a
//! non-zero exit status. Every `--features win` binary is affected,
//! including `cargo test`'s own harness executables, so the whole suite
//! goes dark at once.
//!
//! That failure is invisible on every Linux machine in this fleet.
//! `cargo check -p quadraui --features win` and `cargo test -p quadraui
//! --features win` (the two `ubuntu-latest` win steps in `ci.yml`) compile
//! the `cfg(target_os = "windows")` arms only as `todo!()` fallbacks —
//! there is no import table and no loader involved. It is a *link*-time
//! import that only a real Windows host ever tries to resolve, which is
//! why #744 shipped green on Linux and red on `windows-latest` with a
//! `process didn't exit successfully` and zero captured output.
//!
//! This test closes that hole the same way `macos_appkit_features.rs`
//! closes the `objc2-app-kit` one: a plain text check that runs on every
//! leg, on any OS, in milliseconds. If `src/win/` names a v6-only API from
//! [`V6_ONLY_APIS`], `build.rs` must still be emitting the manifest link
//! args — deleting or "tidying" them fails here instead of on a Windows
//! runner with an empty log.

use std::fs;
use std::path::{Path, PathBuf};

/// comctl32 entry points that exist **only** in the v6 side-by-side
/// assembly, not in `System32\comctl32.dll` (5.82). Naming any of these
/// from `src/win/` is what makes the embedded manifest load-bearing.
///
/// Deliberately a short, hand-written list rather than "everything under
/// `Win32::UI::Controls`": plenty of that module *is* in 5.82 (the classic
/// `ImageList_*`, `PropertySheet*`, `InitCommonControlsEx` surface), and a
/// blanket match would make this test fire on imports that need no
/// manifest at all. Add an entry when a new v6-only call lands.
const V6_ONLY_APIS: &[&str] = &[
    // Vista-era task dialogs (#744) — `src/win/services.rs`'s
    // `show_message_dialog`.
    "TaskDialogIndirect",
    "TaskDialog",
];

/// The assembly identity a manifest must name for the loader to bind
/// `comctl32.dll` to the v6 assembly. All four attributes matter: a
/// dependency missing the `publicKeyToken`, or naming `5.82.0.0`, binds
/// right back to the DLL that lacks the exports.
const REQUIRED_MANIFEST_FRAGMENTS: &[&str] = &[
    "/MANIFEST:EMBED",
    "/MANIFESTDEPENDENCY:",
    "name='Microsoft.Windows.Common-Controls'",
    "version='6.0.0.0'",
    "publicKeyToken='6595b64144ccf1df'",
];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src/win/`, read as source text.
fn win_sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src =
                    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
                out.push((path, src));
            }
        }
    }
    let mut out = Vec::new();
    walk(&crate_root().join("src/win"), &mut out);
    assert!(!out.is_empty(), "src/win/ has no .rs files — wrong path?");
    out
}

fn build_rs() -> String {
    let path = crate_root().join("build.rs");
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "quadraui/build.rs is missing ({e}) — it embeds the Common-Controls v6 manifest \
             that `src/win/`'s comctl32 v6 calls need to even load. See this test's module doc."
        )
    })
}

/// The load-bearing case: a v6-only call in `src/win/` requires the
/// manifest link args in `build.rs`.
#[test]
fn v6_comctl32_calls_require_the_embedded_sxs_manifest() {
    let sources = win_sources();
    let used: Vec<(&str, String)> = V6_ONLY_APIS
        .iter()
        .filter_map(|api| {
            sources
                .iter()
                .find(|(_, src)| src.contains(api))
                .map(|(path, _)| (*api, path.display().to_string()))
        })
        .collect();

    if used.is_empty() {
        // Nothing in `src/win/` needs the manifest today. Not a failure —
        // but the manifest is then dead weight rather than load-bearing,
        // and `v6_api_list_is_not_silently_empty` below is what notices if
        // that happened by accident rather than by design.
        return;
    }

    let build = build_rs();
    for fragment in REQUIRED_MANIFEST_FRAGMENTS {
        assert!(
            build.contains(fragment),
            "quadraui/build.rs no longer emits `{fragment}`, but src/win/ still calls a \
             common-controls v6 API that cannot load without it: {used:?}. Removing the \
             manifest makes EVERY `--features win` binary — including this test harness — \
             die in the Windows loader before `main`, with no output to diagnose it. See \
             this test's module doc."
        );
    }
}

/// The manifest must stay scoped to Windows/MSVC/`win`: these are MSVC
/// linker flags, and CI's `windows-latest` leg also runs a plain
/// `--features tui` build that must not start passing `/MANIFEST:EMBED` to
/// a linker it was never passed to before.
#[test]
fn manifest_link_args_are_gated_on_windows_msvc_and_the_win_feature() {
    let build = build_rs();
    for needle in [
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_ENV",
        "CARGO_FEATURE_WIN",
    ] {
        assert!(
            build.contains(needle),
            "quadraui/build.rs must gate its MSVC manifest link args on `{needle}` — they are \
             MSVC-linker-only and `win`-feature-only. See this test's module doc."
        );
    }
    assert!(
        !build.contains("cfg!(target_os"),
        "build.rs must read the *target* via CARGO_CFG_TARGET_OS, not `cfg!(target_os = …)` — \
         `cfg!` in a build script describes the build HOST, so it is wrong for every \
         cross-compile (including CLAUDE.md's `cargo xwin` route)."
    );
}

/// [`V6_ONLY_APIS`] going empty would silently disarm the test above.
#[test]
fn v6_api_list_is_not_silently_empty() {
    assert!(
        !V6_ONLY_APIS.is_empty(),
        "V6_ONLY_APIS is empty, so `v6_comctl32_calls_require_the_embedded_sxs_manifest` can \
         never fail. Add the v6-only entry points src/win/ calls."
    );
}
