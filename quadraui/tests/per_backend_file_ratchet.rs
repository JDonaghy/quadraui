//! Pins over the four backend directories and `frame::Surface`, in the
//! same family as `tests/backend_method_ratchet.rs`, parsing real source
//! with [`syn`] rather than grepping:
//!
//! 1. [`each_backend_dir_holds_exactly_its_platform_files`] — each of
//!    `src/tui/`, `src/gtk/`, `src/macos/`, `src/win/` holds exactly the
//!    platform modules [`PLATFORM_FILES`] names. A widget lives in
//!    `src/primitives/` and paints through its `Primitive` impl; a backend
//!    directory carries no widget file.
//!
//! 2. [`per_backend_line_count_may_only_shrink`] — each directory's total
//!    non-test line count is a frozen baseline. A PR that grows it fails;
//!    a PR that shrinks it must lower the baseline in the same change, or
//!    this fails instead of recording slack. `RATCHET_WRITE=1 cargo test
//!    -p quadraui --features tui --test per_backend_file_ratchet` rewrites
//!    `BACKEND_BASELINES` below to the measured values, the same way
//!    `scripts/backend_loc_ratchet.py --write` rewrites its own baseline;
//!    re-run without the env var afterward to confirm the ratchet is
//!    tight.
//!
//! 3. [`surface_has_exactly_widget_and_opaque`] — `frame::Surface` has
//!    exactly the `Widget` and `Opaque` variants.
//!
//! `tests/widget_contract_coverage.rs` checks that every widget type has
//! both `Primitive` and `Interactive`.
//!
//! See `docs/PRIMITIVE_RULES.md` for the widget-authoring rules these
//! pins enforce.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::{Item, ItemEnum};

#[path = "common/syn_source.rs"]
mod syn_source;
use syn_source::{collect_rs_files, is_cfg_test, parse_rust_file};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The platform modules each backend directory holds, by file stem. A
/// new file here is a platform service (clipboard, windowing, tray icon,
/// …) with no widget of its own, added to this list in the same PR.
const PLATFORM_FILES: &[(&str, &[&str])] = &[
    (
        "tui",
        &[
            "backend",
            "caps",
            "cell_surface",
            "color",
            "events",
            "focus_ring",
            "image_decode",
            "kitty_graphics",
            "mod",
            "run",
            "services",
            "shell_runner",
            "testing",
            "text",
            "vt_testing",
        ],
    ),
    (
        "gtk",
        &[
            "app_font",
            "backend",
            "events",
            "mod",
            "painted_text",
            "run",
            "services",
            "shell_runner",
            "surface",
            "testing",
            "tray",
        ],
    ),
    (
        "macos",
        &[
            "backend",
            "caret_blink",
            "cg",
            "events",
            "headless",
            "menu_bar_install",
            "mod",
            "run",
            "services",
            "shell_runner",
            "surface",
            "testing",
            "text",
            "text_selection",
            "tray",
        ],
    ),
    (
        "win",
        &[
            "backend",
            "events",
            "mod",
            "msg",
            "run",
            "services",
            "shell_runner",
            "surface",
            "testing",
            "text",
            "tray",
        ],
    ),
];

/// Every `.rs` file under `src/<dir>/`, as a path relative to that
/// directory with the extension dropped (`backend`, or `sub/mod` for a
/// nested module).
fn backend_dir_file_stems(root: &Path, dir: &str) -> BTreeSet<String> {
    let base = root.join("src").join(dir);
    let mut files = Vec::new();
    collect_rs_files(&base, &mut files);
    files
        .iter()
        .map(|path| {
            path.strip_prefix(&base)
                .expect("collected under base")
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn each_backend_dir_holds_exactly_its_platform_files() {
    let root = repo_root();
    for (dir, allowed) in PLATFORM_FILES {
        let found = backend_dir_file_stems(&root, dir);
        let expected: BTreeSet<String> = allowed.iter().map(|s| s.to_string()).collect();
        let extra: Vec<&String> = found.difference(&expected).collect();
        let missing: Vec<&String> = expected.difference(&found).collect();
        assert!(
            extra.is_empty() && missing.is_empty(),
            "src/{dir}/ differs from PLATFORM_FILES: unexpected {extra:?}, missing {missing:?}. \
             A widget lives in src/primitives/ and paints through its `Primitive` impl, never \
             in a backend directory. A new platform service (no widget of its own) goes into \
             PLATFORM_FILES in tests/per_backend_file_ratchet.rs in the same change; a removed \
             one comes out of it. See docs/PRIMITIVE_RULES.md."
        );
    }
}

/// Frozen baselines for each backend directory's total non-test line
/// count. **May only shrink.** A line-count drop must lower the matching
/// entry here in the same change — see the module doc for why both
/// directions are enforced.
///
/// The one growth these numbers absorb is the per-backend half of a
/// backend-neutral platform seam: a new `PreprocessBackend` trait method,
/// or a platform service, needs its field, its constructor initialiser and
/// its body inside this directory. Widget paint code gets no growth at
/// all.
struct BackendBaseline {
    dir: &'static str,
    non_test_lines: usize,
}

const BACKEND_BASELINES: &[BackendBaseline] = &[
    BackendBaseline {
        dir: "tui",
        non_test_lines: 10637,
    },
    BackendBaseline {
        dir: "gtk",
        non_test_lines: 11158,
    },
    BackendBaseline {
        dir: "macos",
        non_test_lines: 11505,
    },
    BackendBaseline {
        dir: "win",
        non_test_lines: 15808,
    },
];

/// A top-level item's own attributes, for the handful of `Item` variants
/// that can plausibly carry `#[cfg(test)]` in this codebase (always
/// `Item::Mod`, per [`is_cfg_test`]'s doc — the other arms are here so a
/// future `#[cfg(test)] fn`/`static` at top level is still caught rather
/// than silently counted as production code).
fn item_attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Mod(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Const(i) => &i.attrs,
        _ => &[],
    }
}

/// A file's total line count minus every top-level `#[cfg(test)]`- or
/// `#[cfg(all(test, …))]`-gated item's own line span (start..=end, via
/// `proc-macro2`'s span-locations feature) — the "non-test line count"
/// this ratchet pins. Line-based rather than a full semantic strip:
/// every embedded test module in `src/{tui,gtk,macos,win}/` is a
/// top-level `mod`, which this resolves, but a `#[cfg(test)]` helper
/// nested inside a non-test top-level item (rather than at the file's
/// top level) is still counted as non-test.
fn non_test_line_count(path: &Path, file: &syn::File) -> usize {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let total_lines = src.lines().count();

    let mut excluded = std::collections::HashSet::new();
    for item in &file.items {
        if item_attrs(item).iter().any(is_cfg_test) {
            let span = item.span();
            for line in span.start().line..=span.end().line {
                excluded.insert(line);
            }
        }
    }
    total_lines - excluded.len()
}

/// Total non-test line count of every `.rs` file under `src/<dir>/` — the
/// measurement both the assertion path and [`write_backend_baselines`]'s
/// `RATCHET_WRITE` path use, kept in one place so they can never disagree.
fn measure_backend_dir(root: &Path, dir: &str) -> usize {
    let path = root.join("src").join(dir);
    let mut files = Vec::new();
    collect_rs_files(&path, &mut files);
    assert!(
        !files.is_empty(),
        "expected at least one .rs file under src/{dir}/"
    );
    files
        .iter()
        .map(|path| non_test_line_count(path, &parse_rust_file(path)))
        .sum()
}

/// Rewrites this file's own `BACKEND_BASELINES` array to `measured`'s
/// `(dir, non_test_lines)` pairs, in `BACKEND_BASELINES`'s
/// existing order. The opt-in side of the `RATCHET_WRITE` env var — see the
/// module doc.
fn write_backend_baselines(root: &Path, measured: &[(&str, usize)]) {
    const START_MARKER: &str = "const BACKEND_BASELINES: &[BackendBaseline] = &[\n";
    const END_MARKER: &str = "\n];\n";

    let path = root.join("tests").join("per_backend_file_ratchet.rs");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    let (before, rest) = src
        .split_once(START_MARKER)
        .unwrap_or_else(|| panic!("could not find BACKEND_BASELINES in {}", path.display()));
    let (_old_block, after) = rest.split_once(END_MARKER).unwrap_or_else(|| {
        panic!(
            "could not find BACKEND_BASELINES' closing `];` in {}",
            path.display()
        )
    });

    let mut block = String::new();
    for (dir, non_test_lines) in measured {
        block.push_str(&format!(
            "    BackendBaseline {{\n        dir: \"{dir}\",\n        non_test_lines: {non_test_lines},\n    }},\n"
        ));
    }
    block.pop(); // the trailing '\n' END_MARKER itself supplies

    let new_src = format!("{before}{START_MARKER}{block}{END_MARKER}{after}");
    std::fs::write(&path, new_src).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    eprintln!(
        "RATCHET_WRITE: rewrote BACKEND_BASELINES in {}",
        path.display()
    );
    for (dir, non_test_lines) in measured {
        eprintln!("  {dir:<6} non_test_lines={non_test_lines}");
    }
}

#[test]
fn per_backend_line_count_may_only_shrink() {
    let root = repo_root();

    let measured: Vec<(&str, usize)> = BACKEND_BASELINES
        .iter()
        .map(|baseline| (baseline.dir, measure_backend_dir(&root, baseline.dir)))
        .collect();

    if std::env::var_os("RATCHET_WRITE").is_some() {
        write_backend_baselines(&root, &measured);
        return;
    }

    for (baseline, &(_dir, non_test_lines)) in BACKEND_BASELINES.iter().zip(&measured) {
        assert!(
            non_test_lines <= baseline.non_test_lines,
            "src/{}/'s non-test line count grew from {} to {} — a per-backend widget's paint \
             path should route through the shared primitive helpers it composes \
             (CLAUDE.md's \"extend, don't fork\"), not accrete bespoke lines in this \
             directory. See docs/PRIMITIVE_RULES.md.",
            baseline.dir,
            baseline.non_test_lines,
            non_test_lines,
        );
        assert_eq!(
            non_test_lines, baseline.non_test_lines,
            "src/{}/'s non-test line count shrank from {} to {} — lower BACKEND_BASELINES' \
             non_test_lines for \"{}\" in tests/per_backend_file_ratchet.rs to {} in the same \
             change that removed the lines, so the ratchet stays tight instead of recording \
             slack. RATCHET_WRITE=1 cargo test -p quadraui --features tui --test \
             per_backend_file_ratchet does this for you.",
            baseline.dir, baseline.non_test_lines, non_test_lines, baseline.dir, non_test_lines,
        );
    }
}

/// The top-level `enum Surface` in `src/frame.rs`.
fn surface_enum(root: &Path) -> ItemEnum {
    parse_rust_file(&root.join("src/frame.rs"))
        .items
        .into_iter()
        .find_map(|item| match item {
            Item::Enum(e) if e.ident == "Surface" => Some(e),
            _ => None,
        })
        .expect("expected a top-level `enum Surface` in src/frame.rs")
}

#[test]
fn surface_has_exactly_widget_and_opaque() {
    let variants: BTreeSet<String> = surface_enum(&repo_root())
        .variants
        .iter()
        .map(|v| v.ident.to_string())
        .collect();
    let expected: BTreeSet<String> = ["Opaque", "Widget"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        variants, expected,
        "frame::Surface must have exactly `Widget` (any type-erased widget, painted through \
         `DrawsPrimitives::draw_dyn`) and `Opaque` (a rect with a hit zone and no paint). A \
         new widget implements `quadraui::Primitive` and is pushed as `Surface::Widget`."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cfg_test_matches_the_bare_form_only() {
        let bare: syn::Attribute = syn::parse_quote!(#[cfg(test)]);
        let not_test: syn::Attribute = syn::parse_quote!(#[cfg(not(test))]);
        let windows: syn::Attribute = syn::parse_quote!(#[cfg(target_os = "windows")]);
        let any_test: syn::Attribute = syn::parse_quote!(#[cfg(any(test, feature = "x"))]);
        assert!(is_cfg_test(&bare));
        assert!(!is_cfg_test(&not_test));
        assert!(!is_cfg_test(&windows));
        assert!(!is_cfg_test(&any_test));
    }

    #[test]
    fn is_cfg_test_matches_the_all_test_conjunction() {
        let win_test: syn::Attribute = syn::parse_quote!(#[cfg(all(test, target_os = "windows"))]);
        let no_test: syn::Attribute =
            syn::parse_quote!(#[cfg(all(target_os = "windows", feature = "tui-images"))]);
        assert!(is_cfg_test(&win_test));
        assert!(!is_cfg_test(&no_test));
    }
}
