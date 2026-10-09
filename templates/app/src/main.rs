//! Per-backend entry point. Each `fn main` body below is the entire
//! backend-specific surface of this crate — everything else
//! (`{{crate_name}}::App`) is plain [`quadraui::ShellApp`] code shared
//! across all four backends (quadraui's cross-backend portability
//! commitment — see its `CLAUDE.md`).
//!
//! Exactly one of the `fn main` bodies below survives `cfg` per feature
//! set — see each arm's `#[cfg(...)]` for the priority order used when
//! more than one backend feature is enabled at once.
//!
//! Imported under a short alias so every call below stays on one line
//! regardless of how long the generated crate name is — `use` is the
//! only place `{{crate_name}}` actually appears. `cfg`-gated the same way
//! the `fn main` bodies below are: the no-backend-feature fallback and
//! the macos-off-target `compile_error!` arm never reference `app`, so an
//! unconditional import would warn as unused in both of those builds.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use {{crate_name}} as app;

#[cfg(feature = "tui")]
fn main() {
    quadraui::tui::shell_runner::run_with_shell(app::App::new(), app::App::config());
}

#[cfg(all(feature = "gtk", not(feature = "tui")))]
fn main() {
    quadraui::gtk::shell_runner::run_with_shell(app::App::new(), app::App::config());
}

#[cfg(all(
    feature = "macos",
    target_os = "macos",
    not(any(feature = "tui", feature = "gtk"))
))]
fn main() -> std::process::ExitCode {
    quadraui::macos::shell_runner::run_with_shell(app::App::new(), app::App::config())
}

#[cfg(all(
    feature = "win",
    not(any(feature = "tui", feature = "gtk")),
    not(all(feature = "macos", target_os = "macos"))
))]
fn main() -> std::process::ExitCode {
    quadraui::win::shell_runner::run_with_shell(app::App::new(), app::App::config())
}

// `--features macos` alone, off a macOS target, builds nothing to run:
// `quadraui::macos` itself only exists under `target_os = "macos"`, so
// there is no `quadraui::macos::shell_runner` to call here either. Fail
// loudly at compile time with a message that names the fix, rather than
// the bare "`main` function not found" a missing `fn main` would
// otherwise leave a reader to puzzle out.
#[cfg(all(
    feature = "macos",
    not(target_os = "macos"),
    not(any(feature = "tui", feature = "gtk", feature = "win"))
))]
compile_error!(
    "{{project-name}}'s `macos` feature only builds a runnable binary on \
     target_os = \"macos\" — combine it with --target {x86_64,aarch64}-apple-darwin, \
     or build on a macOS host. Add --features tui or --features gtk instead for a \
     binary that runs here."
);

#[cfg(not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")))]
fn main() {
    eprintln!("{{project-name}}: enable one of --features tui|gtk|macos|win");
    std::process::exit(1);
}
