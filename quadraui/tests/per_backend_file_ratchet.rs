//! Two more down-only ratchets in the same family as
//! `tests/backend_method_ratchet.rs` (quadraui#2803), both parsing real
//! source with [`syn`] rather than grepping line counts:
//!
//! 1. [`per_backend_widget_file_count_and_line_count_may_only_shrink`] —
//!    counts the `.rs` files (and their total non-test line count) under
//!    each of `src/tui/`, `src/gtk/`, `src/macos/`, `src/win/`. A per-
//!    backend widget gets built by extending an existing file in that
//!    directory, not by adding a new one (`CLAUDE.md`'s "extend, don't
//!    fork" mandate) — this is the mechanical enforcement of that for the
//!    one axis a code reviewer can't eyeball across four directories at
//!    once. Both numbers are frozen baselines: a PR that adds a file or
//!    grows a directory's line count fails here unless the new file is a
//!    platform service (clipboard, windowing, …) named in
//!    `ALLOWED_NEW_FILES` below, not a widget. A PR that *removes* a file
//!    or shrinks the line count must lower the matching baseline in the
//!    same change, or this fails instead of recording slack — the same
//!    down-only rule `backend_method_ratchet.rs` enforces for `Backend`'s
//!    method count.
//!
//! 2. [`primitive_coverage_gap_may_only_grow_smaller`] — counts
//!    `frame::Surface` variants whose resolved widget type has no `impl
//!    Primitive`. A new `Surface` variant for a widget that never got a
//!    `Primitive` impl silently stays off the one dispatch path
//!    (`DrawsPrimitives::draw_primitive`) every new backend gets for free;
//!    pinning this count and failing if it grows catches that at review
//!    time instead of at "someone tries to write backend five" time.
//!
//! See `docs/PRIMITIVE_RULES.md` for the widget-authoring rules both
//! ratchets enforce.

use std::collections::BTreeMap;
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

/// `(backend dir name, file name)` pairs exempt from the per-backend file
/// count below: a platform service (clipboard, windowing, tray icon, …)
/// with no widget of its own, added here in the same PR that adds the
/// file. This is the *only* sanctioned way to grow a backend directory's
/// file count — everything else is a widget, and a widget extends an
/// existing file in this directory rather than adding a new one.
const ALLOWED_NEW_FILES: &[(&str, &str)] = &[
    // The TUI half of the backend-neutral `CellSurface` paint seam
    // (`src/cell_surface.rs`): a paint service every widget reaches
    // through `Primitive::paint_cells`, with no widget of its own.
    ("tui", "cell_surface.rs"),
];

/// Frozen baselines for each backend directory's `.rs` file count and
/// total non-test line count, measured the day quadraui#2803 landed this
/// ratchet. **May only shrink.** A file count drop or line count drop
/// must lower the matching entry here in the same change — see the module
/// doc for why both directions are enforced.
///
/// The one growth these numbers absorb is the per-backend half of a
/// backend-neutral seam: a new `Backend`/`PreprocessBackend` trait method
/// — required, or defaulted with one backend overriding it — needs its
/// field, its constructor initialiser and its body inside this directory,
/// and no amount of composing shared helpers moves those few lines out. A
/// seam implementation large enough to be worth its own file goes in one,
/// listed in [`ALLOWED_NEW_FILES`], so only the irreducible forwarding
/// lines reach these counts. Widget paint code still gets no growth at
/// all.
struct BackendBaseline {
    dir: &'static str,
    file_count: usize,
    non_test_lines: usize,
}

const BACKEND_BASELINES: &[BackendBaseline] = &[
    BackendBaseline {
        dir: "tui",
        file_count: 65,
        non_test_lines: 27996,
    },
    BackendBaseline {
        dir: "gtk",
        file_count: 53,
        non_test_lines: 21224,
    },
    BackendBaseline {
        dir: "macos",
        file_count: 56,
        non_test_lines: 19195,
    },
    BackendBaseline {
        dir: "win",
        file_count: 55,
        non_test_lines: 25044,
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

#[test]
fn per_backend_widget_file_count_and_line_count_may_only_shrink() {
    let root = repo_root();
    let allowed: std::collections::HashSet<(&str, &str)> =
        ALLOWED_NEW_FILES.iter().copied().collect();

    for baseline in BACKEND_BASELINES {
        let dir = root.join("src").join(baseline.dir);
        let mut files = Vec::new();
        collect_rs_files(&dir, &mut files);
        assert!(
            !files.is_empty(),
            "expected at least one .rs file under src/{}/",
            baseline.dir
        );

        let counted_files: Vec<&PathBuf> = files
            .iter()
            .filter(|path| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !allowed.contains(&(baseline.dir, name))
            })
            .collect();

        let file_count = counted_files.len();
        let non_test_lines: usize = counted_files
            .iter()
            .map(|path| non_test_line_count(path, &parse_rust_file(path)))
            .sum();

        assert!(
            file_count <= baseline.file_count,
            "src/{}/ grew from {} counted .rs files to {} — a new per-backend widget extends \
             an existing file in this directory rather than adding a new one (CLAUDE.md's \
             \"extend, don't fork\"). If the new file is a platform service with no widget of \
             its own (clipboard, windowing, …), add it to ALLOWED_NEW_FILES in \
             tests/per_backend_file_ratchet.rs in this same change; otherwise fold the widget \
             into an existing file. See docs/PRIMITIVE_RULES.md.",
            baseline.dir,
            baseline.file_count,
            file_count,
        );
        assert_eq!(
            file_count, baseline.file_count,
            "src/{}/ shrank from {} counted .rs files to {} — lower BACKEND_BASELINES' \
             file_count for \"{}\" in tests/per_backend_file_ratchet.rs to {} in the same \
             change that removed the file, so the ratchet stays tight instead of recording \
             slack.",
            baseline.dir, baseline.file_count, file_count, baseline.dir, file_count,
        );

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
             slack.",
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
/// `None` for a type-erased variant — one carrying a trait object
/// (`Box<dyn DynWidget>`) instead of a named widget type. Such a variant
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

/// Frozen baseline for how many `frame::Surface` variants have no
/// `Primitive`-implementing widget behind them, measured the day
/// quadraui#2803 landed this ratchet. **May only shrink** — see the
/// module doc for why a growth here is a review-time catch, not just a
/// "nice to migrate eventually" note.
const PRIMITIVE_COVERAGE_GAP_BASELINE: usize = 38;

#[test]
fn primitive_coverage_gap_may_only_grow_smaller() {
    let root = repo_root();
    let frame_file = parse_rust_file(&root.join("src/frame.rs"));
    let surface_enum = frame_file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Enum(e @ ItemEnum { ident, .. }) if ident == "Surface" => Some(e),
            _ => None,
        })
        .expect("expected a top-level `enum Surface` in src/frame.rs");

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

    assert!(
        gaps.len() <= PRIMITIVE_COVERAGE_GAP_BASELINE,
        "frame::Surface grew from {PRIMITIVE_COVERAGE_GAP_BASELINE} variants with no \
         `Primitive`-implementing widget to {} — a new widget should implement \
         `quadraui::Primitive` (src/primitive.rs) before it gets a `Surface` variant, so every \
         backend reaches it through `DrawsPrimitives::draw_primitive` for free. If this growth \
         is genuinely unavoidable, say why in the PR; if it isn't, add the `Primitive` impl. \
         See docs/PRIMITIVE_RULES.md.\n\nGaps found: {:#?}",
        gaps.len(),
        gaps,
    );

    if gaps.len() < PRIMITIVE_COVERAGE_GAP_BASELINE {
        println!(
            "primitive_coverage_gap_may_only_grow_smaller: {} variant(s) now lack a `Primitive` \
             impl, below the frozen baseline of {PRIMITIVE_COVERAGE_GAP_BASELINE} — lower \
             PRIMITIVE_COVERAGE_GAP_BASELINE in tests/per_backend_file_ratchet.rs to this \
             file's count to re-tighten the ratchet.",
            gaps.len()
        );
    }
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
    #[should_panic(expected = "has no field whose type name relates to its own")]
    fn surface_widget_type_panics_with_no_candidate() {
        let variant: syn::Variant = syn::parse_quote!(Spinner {
            rect: Rect,
            hovered: bool
        });
        let _ = surface_widget_type(&variant);
    }
}
