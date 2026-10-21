//! Build script — embeds the Common-Controls v6 side-by-side manifest for the
//! `win` feature.
//!
//! `cargo:rustc-link-arg` does NOT propagate across crates, so quadraui's own
//! manifest embed (`quadraui/build.rs`, #744) does not cover this crate's
//! binaries — including `cargo test`'s win-feature harness exe. Without it that
//! exe binds the legacy comctl32 5.82, cannot find `TaskDialogIndirect` (reached
//! transitively through quadraui's `win` backend's message-dialog services),
//! and is killed by the loader before any test runs (0xc0000139,
//! STATUS_ENTRYPOINT_NOT_FOUND) — which is why the whole gallery win test binary,
//! not just one test, was unlaunchable (#1642). See `quadraui/build.rs` for the
//! full rationale; this is the "downstream crate must embed its own manifest"
//! note from that file's doc comment, made real.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Read the *target* cfg cargo hands the build script (never `cfg!(…)`,
    // which describes the build host — wrong for the `cargo xwin` cross route).
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let win_feature = std::env::var_os("CARGO_FEATURE_WIN").is_some();

    // MSVC linker flags, gated on the `win` feature so a plain Windows build
    // without it stays byte-identical. `lld-link` (cargo xwin) and `link.exe`
    // both implement these natively.
    if target_os == "windows" && target_env == "msvc" && win_feature {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
