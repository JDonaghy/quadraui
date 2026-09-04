//! Build script — Windows side-by-side manifest only.
//!
//! # Why this file exists (#744)
//!
//! `src/win/services.rs::win_show_message_dialog` calls
//! `TaskDialogIndirect`, which the `windows` crate imports **statically**
//! from `comctl32.dll`. That export does not exist in
//! `%SystemRoot%\System32\comctl32.dll` — that DLL is still the legacy
//! 5.82 build, and `TaskDialogIndirect` (like every other common-controls
//! v6 API) ships *only* in the side-by-side v6 assembly under
//! `%SystemRoot%\WinSxS\…microsoft.windows.common-controls…_6.0.*`.
//!
//! A Win32 binary only gets the v6 assembly if its application manifest
//! declares a dependency on it. Without that manifest the Windows loader
//! resolves `comctl32.dll` to 5.82, fails to find `TaskDialogIndirect` in
//! its export table, and **kills the process before `main` runs** — no
//! output, no panic, no backtrace, just a non-zero exit status. That is
//! not a "the dialog doesn't appear" bug: *every* binary that links the
//! `win` feature becomes unlaunchable, including `cargo test`'s own
//! harness executables, which is exactly how this surfaced (quadraui#744
//! CI: the `Test (win feature, real Windows)` step on the
//! `windows-latest` leg went red with `process didn't exit successfully`
//! and zero captured output, while `Build`/`Clippy` and the whole Linux
//! leg stayed green — a *link*-time import that only the loader on a real
//! Windows host ever tries to resolve).
//!
//! `/MANIFEST:EMBED` + `/MANIFESTDEPENDENCY:` is the linker spelling of
//! the classic `#pragma comment(linker, "/manifestdependency:…")` every
//! C++ TaskDialog sample carries. It embeds an `RT_MANIFEST` resource in
//! each linked binary, so the loader binds `comctl32.dll` to the v6
//! assembly. Both MSVC's `link.exe` (what `windows-latest` CI uses) and
//! `lld-link` (what `cargo xwin` uses to cross-build from Linux)
//! implement these two flags natively — neither needs `mt.exe`.
//!
//! `cargo:rustc-link-arg` applies to *binaries, examples, tests and
//! benches* of this crate, which is the whole set that needs it here.
//!
//! ## Downstream note
//!
//! Link args from a build script do **not** propagate to crates that
//! depend on `quadraui`. A downstream Windows application linking the
//! `win` backend must embed an equivalent Common-Controls v6 manifest of
//! its own (the `embed-manifest` crate, a `.rc` resource, or the same two
//! linker flags). That is a universal Win32 requirement for visual styles
//! and task dialogs, not a quadraui quirk — but it is a real obligation,
//! so it is stated here rather than left to be rediscovered by a loader
//! failure with no output. Today the `win` backend has no downstream
//! consumer (`coord-tui` and `vimcode` both build `tui`/`gtk`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Host-independent gate: read the *target* cfg cargo hands the build
    // script, never `cfg!(…)` (which would describe the build host and so
    // would be wrong for every cross-compile, including the `cargo xwin`
    // route documented in CLAUDE.md).
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let win_feature = std::env::var_os("CARGO_FEATURE_WIN").is_some();

    // `target_env == "msvc"` because these are MSVC linker flags; a
    // `*-pc-windows-gnu` build links with `ld`, which would reject them.
    // The `win` feature gate keeps a plain `--features tui` Windows build
    // (the other half of CI's `windows-latest` leg) byte-identical to what
    // it was before this file existed.
    if target_os == "windows" && target_env == "msvc" && win_feature {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
