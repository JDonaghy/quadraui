//! Per-backend entry point. Each `fn main` below is the entire
//! backend-specific surface of this crate — everything else
//! (`quadraui_gallery::GalleryApp`, every `Demo`) is plain
//! [`quadraui::ShellApp`] code shared across all four backends
//! (CLAUDE.md's cross-backend portability commitment).
//!
//! Exactly one of the `fn main` bodies below survives `cfg` per feature
//! set — see each arm's `#[cfg(...)]` for the priority order used when
//! more than one backend feature is enabled at once (e.g. the repo's own
//! `--features gtk,tui` CI leg).

#[cfg(feature = "tui")]
fn main() {
    quadraui::tui::shell_runner::run_with_shell(
        quadraui_gallery::GalleryApp::new(),
        quadraui_gallery::GalleryApp::config(),
    );
}

#[cfg(all(feature = "gtk", not(feature = "tui")))]
fn main() {
    quadraui::gtk::shell_runner::run_with_shell(
        quadraui_gallery::GalleryApp::new(),
        quadraui_gallery::GalleryApp::config(),
    );
}

#[cfg(all(
    feature = "macos",
    target_os = "macos",
    not(any(feature = "tui", feature = "gtk"))
))]
fn main() -> std::process::ExitCode {
    quadraui::macos::shell_runner::run_with_shell(
        quadraui_gallery::GalleryApp::new(),
        quadraui_gallery::GalleryApp::config(),
    )
}

#[cfg(all(
    feature = "win",
    not(any(feature = "tui", feature = "gtk")),
    not(all(feature = "macos", target_os = "macos"))
))]
fn main() -> std::process::ExitCode {
    quadraui::win::shell_runner::run_with_shell(
        quadraui_gallery::GalleryApp::new(),
        quadraui_gallery::GalleryApp::config(),
    )
}

// `--features macos` alone, off a macOS target, builds nothing to run:
// `quadraui::macos` itself only exists under `target_os = "macos"` (see
// `quadraui/src/lib.rs`'s `pub mod macos` cfg gate), so there is no
// `quadraui::macos::shell_runner` to call here either. Fail loudly at
// compile time with a message that names the fix, rather than the bare
// "`main` function not found" a missing `fn main` would otherwise leave
// a reader to puzzle out.
#[cfg(all(
    feature = "macos",
    not(target_os = "macos"),
    not(any(feature = "tui", feature = "gtk", feature = "win"))
))]
compile_error!(
    "quadraui-gallery's `macos` feature only builds a runnable binary on \
     target_os = \"macos\" — combine it with --target {x86_64,aarch64}-apple-darwin, \
     or build on a macOS host. Add --features tui or --features gtk instead for a \
     binary that runs here."
);

#[cfg(not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")))]
fn main() {
    eprintln!("quadraui-gallery: enable one of --features tui|gtk|macos|win");
    std::process::exit(1);
}
