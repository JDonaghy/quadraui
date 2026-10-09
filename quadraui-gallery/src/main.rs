//! Per-backend entry point. Each `fn main` below is the entire
//! backend-specific surface of this crate — everything else
//! (`quadraui_gallery::GalleryApp`, every `Demo`) is plain
//! [`quadraui::ShellApp`] code shared across all four backends
//! (CLAUDE.md's cross-backend portability commitment).
//!
//! Exactly one of the `fn main` bodies below survives `cfg` per feature
//! set — see each arm's `#[cfg(...)]` for the priority order used when
//! more than one backend feature is enabled at once (e.g. the repo's own
//! `--features gtk,tui` CI leg). Every surviving arm calls
//! [`maybe_run_capture`] and [`maybe_dump_registry`] first: `--capture
//! <dir>` short-circuits the interactive runner and drives
//! [`quadraui_gallery::capture::run_capture`] instead, and `--dump-registry
//! <file>` drives [`quadraui_gallery::site_export::dump_registry`] — the
//! same functions, not parallel copies, that `tests/capture_driver.rs`
//! and `quadraui_gallery::site_export`'s own tests call directly.
//! `tools/site_gen.py` (quadraui#1349) is the one consumer of both: it
//! joins a `--dump-registry` JSON file against a `--capture` directory's
//! `manifest.json` to build the mdBook gallery site.

/// If argv requests `--capture <dir>` or `--capture=<dir>`, run headless
/// capture mode into `<dir>` and exit the process (`0` on success, `1`
/// on I/O failure, `2` on a malformed flag) — that path never returns.
/// Otherwise (no `--capture` flag present) returns normally so the
/// caller's own interactive runner proceeds.
fn maybe_run_capture() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .iter()
        .find_map(|a| a.strip_prefix("--capture=").map(str::to_string))
        .or_else(|| {
            let pos = args.iter().position(|a| a == "--capture")?;
            args.get(pos + 1).cloned()
        });
    let Some(dir) = dir else {
        if args.iter().any(|a| a == "--capture") {
            eprintln!("quadraui-gallery: --capture requires a directory argument");
            std::process::exit(2);
        }
        return;
    };
    match quadraui_gallery::capture::run_capture(std::path::Path::new(&dir)) {
        Ok(entries) => {
            eprintln!(
                "quadraui-gallery: captured {} manifest entries into {dir}",
                entries.len()
            );
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("quadraui-gallery: capture into {dir} failed: {err}");
            std::process::exit(1);
        }
    }
}

/// If argv requests `--dump-registry <file>` or `--dump-registry=<file>`,
/// write the demo registry's JSON description to `<file>` and exit the
/// process (`0` on success, `1` on I/O failure, `2` on a malformed flag)
/// — that path never returns. Otherwise returns normally. Needs no
/// backend feature at all (see `site_export`'s module doc), so it runs
/// identically in every `fn main` arm below, including the
/// no-backend-features fallback.
fn maybe_dump_registry() {
    let args: Vec<String> = std::env::args().collect();
    let dest = args
        .iter()
        .find_map(|a| a.strip_prefix("--dump-registry=").map(str::to_string))
        .or_else(|| {
            let pos = args.iter().position(|a| a == "--dump-registry")?;
            args.get(pos + 1).cloned()
        });
    let Some(dest) = dest else {
        if args.iter().any(|a| a == "--dump-registry") {
            eprintln!("quadraui-gallery: --dump-registry requires a file argument");
            std::process::exit(2);
        }
        return;
    };
    match quadraui_gallery::site_export::dump_registry(std::path::Path::new(&dest)) {
        Ok(entries) => {
            eprintln!(
                "quadraui-gallery: dumped {} registry entries into {dest}",
                entries.len()
            );
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("quadraui-gallery: dump-registry into {dest} failed: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "tui")]
fn main() {
    maybe_run_capture();
    maybe_dump_registry();
    quadraui::tui::shell_runner::run_with_shell(
        quadraui_gallery::GalleryApp::new(),
        quadraui_gallery::GalleryApp::config(),
    );
}

#[cfg(all(feature = "gtk", not(feature = "tui")))]
fn main() {
    maybe_run_capture();
    maybe_dump_registry();
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
    maybe_run_capture();
    maybe_dump_registry();
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
    maybe_run_capture();
    maybe_dump_registry();
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
    // Still honour `--capture` with zero backend features enabled: every
    // entry comes back `"status": "unsupported"`/`"capture-pending"`
    // (see `capture::run_capture`'s doc), which is a degenerate but
    // honest manifest rather than a silent no-op. `--dump-registry` needs
    // no backend feature either, so it runs for real here rather than
    // degrading.
    maybe_run_capture();
    maybe_dump_registry();
    eprintln!("quadraui-gallery: enable one of --features tui|gtk|macos|win");
    std::process::exit(1);
}
