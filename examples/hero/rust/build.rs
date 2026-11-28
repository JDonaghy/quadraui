//! Embeds the Common-Controls v6 side-by-side manifest for the `win` feature.
//!
//! quadraui's `win` backend reaches `TaskDialogIndirect` (its message-dialog
//! service), which only comctl32 v6 exports. `cargo:rustc-link-arg` does not
//! propagate across crates, so quadraui's own manifest embed
//! (`quadraui/build.rs`) does not cover this crate's binaries. Without this,
//! `counter-native` and `hero-capture` bind the legacy comctl32 5.82 and the
//! loader kills them before `main` with 0xc0000139
//! (STATUS_ENTRYPOINT_NOT_FOUND). `quadraui-gallery/build.rs` does the same.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // The *target* cfg cargo hands the build script, not `cfg!(...)`, which
    // describes the build host.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let win_feature = std::env::var_os("CARGO_FEATURE_WIN").is_some();

    if target_os == "windows" && target_env == "msvc" && win_feature {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
