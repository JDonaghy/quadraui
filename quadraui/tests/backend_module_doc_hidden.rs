//! Guards the "unstable, not covered by semver" policy the four backend
//! modules (`src/gtk/mod.rs`, `src/tui/mod.rs`, `src/win/mod.rs`,
//! `src/macos/mod.rs`) each declare in their own `//!` header: every
//! `pub mod` and every name a `pub use` re-exports at the top of one of
//! those modules must either be `#[doc(hidden)]` or be on this test's
//! small, named allow-list of consumer-depended-on items.
//!
//! This test text-parses the four source files directly rather than
//! introspecting the compiled crate — the same approach
//! `macos_safety_docs.rs` uses, and for the same reason: `src/macos/`
//! only compiles on `target_os = "macos"`, so a test that needs to run on
//! every CI host (this one does — there is no dedicated "did we hide the
//! right things" check on any platform today) cannot drive it through
//! `rustdoc`/`cargo-semver-checks` output.
//!
//! A name landing on the allow-list below is a real decision, not a
//! rubber stamp: it means `~/src/vimcode/src`, `~/src/coord-tui/src`, or
//! an in-tree consumer (`quadraui/examples`, `quadraui/benches`,
//! `quadraui/tests`, `kubeui*`) reaches that exact flat path today. A new
//! `pub mod`/`pub use` item that isn't one of these must be
//! `#[doc(hidden)]` until a real caller shows up — this test fails loudly
//! instead of silently growing the tracked surface `cargo-semver-checks`
//! has to treat as a semver promise.

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Backend module path, and the fixed allow-list of `pub mod` names and
/// top-level `pub use`-brought-in names that may stay undecorated
/// (no `#[doc(hidden)]`) in that file.
struct BackendModule {
    rel_path: &'static str,
    stable_mods: &'static [&'static str],
    stable_flat_names: &'static [&'static str],
}

const BACKENDS: &[BackendModule] = &[
    BackendModule {
        rel_path: "src/gtk/mod.rs",
        stable_mods: &[
            "backend",
            "events",
            "services",
            "shell_runner",
            "testing",
            "scroll_view",
        ],
        stable_flat_names: &[
            "ACTIVITY_ROW_PX",
            "GtkBackend",
            "editor_col_at_x",
            "draw_editor",
            "draw_data_table",
            "draw_minimap",
            "draw_tree",
            "run",
            "app_id_is_valid",
            "display_available",
            "GtkRunner",
            "RunConfig",
            "StepOutcome",
            // `#[deprecated]` shim, tracked separately until the
            // `TabBarLayout` migration lands — not part of this policy.
            "TabBarHits",
        ],
    },
    BackendModule {
        rel_path: "src/tui/mod.rs",
        stable_mods: &[
            "backend",
            "events",
            "services",
            "shell_runner",
            "testing",
            "text",
            "vt_testing",
        ],
        stable_flat_names: &[
            "TuiBackend",
            "char_cell_width",
            "display_width",
            "draw_form",
            "truncate_to_width",
            "tui_toolbar_layout",
            "run",
            "run_with",
            "RunConfig",
            "StepOutcome",
            "TuiRunner",
            "draw_data_table",
            "draw_editor",
            "draw_minimap",
            "draw_multi_section_view",
            "tui_msv_layout",
            "draw_tree",
            "tui_tree_layout",
        ],
    },
    BackendModule {
        rel_path: "src/win/mod.rs",
        stable_mods: &[
            "backend",
            "events",
            "services",
            "shell_runner",
            "testing",
            "text",
            "scroll_view",
        ],
        stable_flat_names: &[
            "WinBackend",
            "run",
            "run_with",
            "RunConfig",
            "StepOutcome",
            "WinRunner",
        ],
    },
    BackendModule {
        rel_path: "src/macos/mod.rs",
        stable_mods: &[
            "backend",
            "events",
            "services",
            "shell_runner",
            "testing",
            "text",
        ],
        stable_flat_names: &["MacBackend", "run", "RunConfig", "StepOutcome", "MacRunner"],
    },
];

/// Is the non-blank, non-attribute line immediately above `lines[idx]`
/// (skipping any run of attribute lines directly above it, so a `#[cfg]`/
/// `#[allow(deprecated)]` stacked alongside `#[doc(hidden)]` doesn't hide
/// it from this scan) a `#[doc(hidden)]`?
fn preceded_by_doc_hidden(lines: &[&str], idx: usize) -> bool {
    let mut i = idx;
    while i > 0 {
        i -= 1;
        let trimmed = lines[i].trim();
        if trimmed == "#[doc(hidden)]" {
            return true;
        }
        if trimmed.starts_with('#') {
            continue; // another attribute (cfg, allow, ...) — keep walking up
        }
        break;
    }
    false
}

/// Names a `pub use <path>::<tail>;` statement (already joined onto one
/// line, semicolon included) brings into this module's own namespace.
fn names_from_pub_use(stmt: &str) -> Vec<String> {
    let inner = stmt
        .trim_start_matches("pub use")
        .trim()
        .trim_end_matches(';')
        .trim();
    // Drop the leading `module::` (or `module::sub::`) path, keeping
    // only the final `{...}` brace group or trailing identifier.
    if let Some(brace_start) = inner.rfind("::{") {
        let items = &inner[brace_start + 3..inner.len() - 1];
        items
            .split(',')
            .map(|it| it.trim())
            .filter(|it| !it.is_empty())
            .map(|it| it.rsplit(" as ").next().unwrap().trim().to_string())
            .collect()
    } else {
        let tail = inner.rsplit("::").next().unwrap_or(inner);
        vec![tail.rsplit(" as ").next().unwrap().trim().to_string()]
    }
}

#[test]
fn backend_modules_hide_unstable_painter_internals() {
    let mut failures: Vec<String> = Vec::new();
    let mut pub_mod_checked = 0usize;
    let mut pub_use_checked = 0usize;

    for module in BACKENDS {
        let path = manifest_dir().join(module.rel_path);
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let lines: Vec<&str> = src.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            if let Some(rest) = trimmed.strip_prefix("pub mod ") {
                let name = rest.trim_end_matches(';').trim();
                pub_mod_checked += 1;
                if !module.stable_mods.contains(&name) && !preceded_by_doc_hidden(&lines, i) {
                    failures.push(format!(
                        "{}:{}: `pub mod {name};` is neither on the stable \
                         allow-list ({:?}) nor `#[doc(hidden)]` — a new \
                         painter-internal submodule needs `#[doc(hidden)]` \
                         (see `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`), \
                         and a genuinely consumer-depended-on one needs this \
                         test's allow-list updated with the grep that proves it.",
                        module.rel_path,
                        i + 1,
                        module.stable_mods
                    ));
                }
            }

            if trimmed.starts_with("pub use ") {
                // Join a brace-form statement that spans multiple lines.
                let mut end = i;
                while !lines[end].contains(';') {
                    end += 1;
                }
                let stmt = lines[i..=end].join(" ");
                for name in names_from_pub_use(&stmt) {
                    pub_use_checked += 1;
                    if !module.stable_flat_names.contains(&name.as_str())
                        && !preceded_by_doc_hidden(&lines, i)
                    {
                        failures.push(format!(
                            "{}:{}: flat re-export `{name}` is neither on the \
                             stable allow-list nor `#[doc(hidden)]` — see this \
                             test's module doc.",
                            module.rel_path,
                            i + 1
                        ));
                    }
                }
            }
        }
    }

    // A scanner bug that matched nothing would make this test permanently
    // green — these four files have a few hundred `pub use`-brought-in
    // names and a few dozen `pub mod`s between them.
    assert!(
        pub_mod_checked > 20,
        "expected many `pub mod` declarations across the four backend \
         modules, found {pub_mod_checked} — has the scanner stopped matching?"
    );
    assert!(
        pub_use_checked > 100,
        "expected a few hundred flat re-exported names across the four \
         backend modules, found {pub_use_checked} — has the scanner \
         stopped matching?"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The scanner itself, proven on fixture text rather than on the tree it
/// guards (where a pass is indistinguishable from a no-op).
#[test]
fn scanner_distinguishes_hidden_from_tracked() {
    let hidden: Vec<&str> = vec!["#[doc(hidden)]", "pub use board::gtk_board_layout;"];
    assert!(preceded_by_doc_hidden(&hidden, 1));

    // `#[cfg(...)]`/`#[allow(deprecated)]` stacked alongside
    // `#[doc(hidden)]` must not hide it from the walk-up.
    let stacked: Vec<&str> = vec![
        "#[doc(hidden)]",
        "#[allow(deprecated)]",
        "pub use tab_bar::draw_tab_bar_icons;",
    ];
    assert!(preceded_by_doc_hidden(&stacked, 2));

    let tracked: Vec<&str> = vec!["pub use backend::GtkBackend;"];
    assert!(!preceded_by_doc_hidden(&tracked, 0));

    assert_eq!(
        names_from_pub_use("pub use backend::GtkBackend;"),
        vec!["GtkBackend".to_string()]
    );
    assert_eq!(
        names_from_pub_use("pub use run::{run, run_with, RunConfig, StepOutcome};"),
        vec!["run", "run_with", "RunConfig", "StepOutcome"]
    );
    assert_eq!(
        names_from_pub_use("pub use multi_section_view::{draw_multi_section_view, metrics_for as multi_section_view_metrics};"),
        vec!["draw_multi_section_view", "multi_section_view_metrics"]
    );
}
