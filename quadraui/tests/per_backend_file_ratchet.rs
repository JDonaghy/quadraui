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
//! 4. [`primitive_coverage_gap_is_zero`] — no `frame::Surface` variant
//!    names a widget type without an `impl Primitive`.
//!
//! See `docs/PRIMITIVE_RULES.md` for the widget-authoring rules these
//! pins enforce.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::{Fields, Item, ItemEnum, Type};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn parse_rust_file(path: &Path) -> syn::File {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()))
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
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
        non_test_lines: 10913,
    },
    BackendBaseline {
        dir: "gtk",
        non_test_lines: 12950,
    },
    BackendBaseline {
        dir: "macos",
        non_test_lines: 12592,
    },
    BackendBaseline {
        dir: "win",
        non_test_lines: 17257,
    },
];

/// Whether an attribute is `#[cfg(test)]` or `#[cfg(all(test, …))]` — the
/// shapes a backend file's embedded unit-test module(s) use (`CLAUDE.md`'s
/// "Tests in `#[cfg(test)] mod tests` at file bottom" convention, plus the
/// extra `target_os = "windows"` conjunct the `win/` modules that compile
/// everywhere need). Deliberately does **not** recurse into `not(…)` or
/// `any(…)` — `cfg(not(test))` is active in non-test builds, and
/// `cfg(any(test, …))` can be true without `test`, so neither is a
/// test-only item. Only a direct `test` conjunct inside `all(…)` counts.
fn is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    match attr.parse_args::<syn::Meta>() {
        Ok(syn::Meta::Path(p)) => p.is_ident("test"),
        Ok(syn::Meta::List(list)) if list.path.is_ident("all") => list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .map(|metas| {
                metas
                    .iter()
                    .any(|m| matches!(m, syn::Meta::Path(p) if p.is_ident("test")))
            })
            .unwrap_or(false),
        _ => false,
    }
}

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

/// Render a `syn::Type` naming a bare local type (e.g. `ListView`) back to
/// its identifier, or `None` for anything more exotic (a reference, a
/// generic like `Option<T>` whose own segment ident is the outer `Option`,
/// a path with multiple segments, …) — mirrors
/// `backend_method_ratchet.rs`'s `bare_type_ident`.
fn bare_type_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            p.path.segments.last().map(|seg| seg.ident.to_string())
        }
        _ => None,
    }
}

/// Whether `ty` is a smart pointer wrapping a trait object
/// (`Box<dyn DynWidget>`) — the shape a type-erased `Surface` variant
/// carries in place of a concrete widget type.
fn wraps_trait_object(ty: &Type) -> bool {
    let Type::Path(p) = ty else {
        return false;
    };
    if p.qself.is_some() {
        return false;
    }
    let Some(seg) = p.path.segments.last() else {
        return false;
    };
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    args.args
        .iter()
        .any(|arg| matches!(arg, syn::GenericArgument::Type(Type::TraitObject(_))))
}

/// Find the one field in a `Surface` variant whose type is its actual
/// widget (as opposed to `rect: Rect`, a `*Layout` the variant carries
/// instead of a bare `rect`, or a plain `Option<...>`/`usize` piece of
/// paint state): the field whose bare type ident contains the variant's
/// own name, or vice versa (`Tree { tree: TreeView }`, `FindReplace {
/// panel: FindReplacePanel }`), after dropping any candidate whose type
/// name ends in `Layout` (every transient-overlay variant also carries its
/// own resolved `*Layout`, which otherwise also matches this substring
/// rule and makes the match ambiguous).
///
/// `None` for a variant with no widget at all (`Opaque { rect }`, only
/// `Rect` fields), and for a type-erased variant — one carrying a trait
/// object (`Box<dyn DynWidget>`) instead of a named widget type. Such a variant
/// has no single widget to resolve: it stands in for every widget reached
/// through `DrawsPrimitives::draw_dyn`, which is the dispatch path the
/// coverage census below exists to push widgets onto, so it is counted
/// neither as covered nor as a gap.
///
/// Panics if zero or more than one *named* field matches — every concrete
/// `Surface` variant has exactly one widget field today; an enum shape
/// this can't resolve needs a human to extend the rule, not a silent
/// guess.
fn surface_widget_type(variant: &syn::Variant) -> Option<String> {
    let Fields::Named(fields) = &variant.fields else {
        panic!(
            "Surface::{} has no named fields — this ratchet only knows how to resolve the \
             widget type of a named-field variant",
            variant.ident
        );
    };
    if fields.named.iter().any(|f| wraps_trait_object(&f.ty)) {
        return None;
    }
    if fields
        .named
        .iter()
        .all(|f| bare_type_ident(&f.ty).as_deref() == Some("Rect"))
    {
        return None;
    }
    let variant_name = variant.ident.to_string();
    let candidates: Vec<String> = fields
        .named
        .iter()
        .filter_map(|f| bare_type_ident(&f.ty))
        .filter(|ident| ident != "Rect" && !ident.ends_with("Layout"))
        .filter(|ident| ident.contains(&variant_name) || variant_name.contains(ident.as_str()))
        .collect();
    match candidates.as_slice() {
        [one] => Some(one.clone()),
        [] => panic!(
            "Surface::{variant_name} has no field whose type name relates to its own — extend \
             `surface_widget_type` in tests/per_backend_file_ratchet.rs to resolve it"
        ),
        many => panic!(
            "Surface::{variant_name} has {} candidate widget fields ({:?}) — extend \
             `surface_widget_type` in tests/per_backend_file_ratchet.rs to disambiguate",
            many.len(),
            many
        ),
    }
}

/// Every type name with a top-level `impl (crate::primitive::)?Primitive
/// for <Type>` anywhere under `src/`.
fn primitive_impl_type_names(root: &Path) -> std::collections::HashSet<String> {
    let mut files = Vec::new();
    collect_rs_files(&root.join("src"), &mut files);

    let mut names = std::collections::HashSet::new();
    for path in &files {
        let file = parse_rust_file(path);
        for item in &file.items {
            if let Item::Impl(imp) = item {
                let is_primitive = imp
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .map(|seg| seg.ident == "Primitive")
                    .unwrap_or(false);
                if is_primitive {
                    if let Some(name) = bare_type_ident(&imp.self_ty) {
                        names.insert(name);
                    }
                }
            }
        }
    }
    names
}

/// How many `frame::Surface` variants name a widget type with no `impl
/// Primitive`: none. Every widget reaches every backend through
/// `DrawsPrimitives::draw_dyn`.
const PRIMITIVE_COVERAGE_GAP_BASELINE: usize = 0;

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

#[test]
fn primitive_coverage_gap_is_zero() {
    let root = repo_root();
    let surface_enum = surface_enum(&root);

    let primitive_types = primitive_impl_type_names(&root);

    let mut gaps: BTreeMap<String, String> = BTreeMap::new();
    for variant in &surface_enum.variants {
        let Some(widget_type) = surface_widget_type(variant) else {
            continue;
        };
        if !primitive_types.contains(&widget_type) {
            gaps.insert(variant.ident.to_string(), widget_type);
        }
    }

    assert_eq!(
        gaps.len(),
        PRIMITIVE_COVERAGE_GAP_BASELINE,
        "frame::Surface has variant(s) naming a widget with no `Primitive` impl: {gaps:#?}. A \
         widget implements `quadraui::Primitive` (src/primitive.rs) and is pushed as \
         `Surface::Widget`. See docs/PRIMITIVE_RULES.md."
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

    #[test]
    fn surface_widget_type_resolves_a_plain_variant() {
        let variant: syn::Variant = syn::parse_quote!(Tree {
            rect: Rect,
            tree: TreeView
        });
        assert_eq!(surface_widget_type(&variant).as_deref(), Some("TreeView"));
    }

    #[test]
    fn surface_widget_type_drops_the_sibling_layout_field() {
        let variant: syn::Variant = syn::parse_quote!(Tooltip {
            tooltip: Tooltip,
            layout: TooltipLayout
        });
        assert_eq!(surface_widget_type(&variant).as_deref(), Some("Tooltip"));
    }

    #[test]
    fn surface_widget_type_matches_when_the_type_name_is_longer() {
        let variant: syn::Variant = syn::parse_quote!(FindReplace {
            rect: Rect,
            panel: FindReplacePanel
        });
        assert_eq!(
            surface_widget_type(&variant).as_deref(),
            Some("FindReplacePanel")
        );
    }

    /// The type-erased variant resolves to no widget type at all, so the
    /// coverage census skips it instead of panicking on a `Box<dyn …>`
    /// field it cannot name.
    #[test]
    fn surface_widget_type_skips_a_type_erased_variant() {
        let variant: syn::Variant = syn::parse_quote!(Widget {
            rect: Rect,
            widget: Box<dyn DynWidget>
        });
        assert_eq!(surface_widget_type(&variant), None);
    }

    /// A concrete widget behind a smart pointer is still a named widget
    /// type — only a trait object makes the variant type-erased.
    #[test]
    fn surface_widget_type_resolves_a_boxed_concrete_widget() {
        let variant: syn::Variant = syn::parse_quote!(Tree {
            rect: Rect,
            tree: TreeView
        });
        assert_eq!(surface_widget_type(&variant).as_deref(), Some("TreeView"));
        assert!(!wraps_trait_object(
            &syn::parse_quote!(Box<TreeView>) as &Type
        ));
    }

    #[test]
    fn surface_widget_type_skips_a_rect_only_variant() {
        let variant: syn::Variant = syn::parse_quote!(Opaque { rect: Rect });
        assert_eq!(surface_widget_type(&variant), None);
    }

    #[test]
    #[should_panic(expected = "has no field whose type name relates to its own")]
    fn surface_widget_type_panics_with_no_candidate() {
        let variant: syn::Variant = syn::parse_quote!(Spinner {
            rect: Rect,
            hovered: bool
        });
        let _ = surface_widget_type(&variant);
    }
}
