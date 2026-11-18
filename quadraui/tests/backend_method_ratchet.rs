//! Ratchet tests over `Backend`'s trait body (quadraui#2608): lock in the
//! direction set by `src/primitive.rs` — a new widget implements
//! [`quadraui::Primitive`] and goes through
//! [`quadraui::backend::DrawsPrimitives::draw_primitive`], it does not add a
//! fresh `draw_*`/`*_layout` pair to the sealed `Backend` trait.
//!
//! Two tests, both parsing real source with [`syn`] rather than grepping
//! line counts (mirrors `tests/backend_todo_gate.rs`'s approach):
//!
//! 1. [`backend_trait_method_count_may_only_shrink`] counts `Backend`'s
//!    trait-body methods and fails if that count rises above a frozen
//!    baseline. The baseline may only be lowered, never raised — the whole
//!    point of a ratchet.
//! 2. [`primitives_dir_types_skip_bespoke_backend_methods`] finds every
//!    `impl Primitive for X` under `src/primitives/` and asserts `Backend`
//!    carries no bespoke `draw_<stem>`/`<stem>_layout` pair for any
//!    leading-word stem of `X`'s name (`ListView` -> `list_view` or
//!    `list`), except the pre-existing legacy pairs this test grandfathers
//!    explicitly.

use std::path::{Path, PathBuf};

use syn::{Item, ItemImpl, ItemTrait, TraitItem, Type};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn parse_rust_file(path: &Path) -> syn::File {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()))
}

/// Find a top-level `trait <name> { ... }` item in a parsed file.
fn find_trait<'a>(file: &'a syn::File, name: &str) -> &'a ItemTrait {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Trait(t) if t.ident == name => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("trait `{name}` not found"))
}

/// Every method name declared directly in a trait's body (default or not).
fn trait_method_names(t: &ItemTrait) -> Vec<String> {
    t.items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// Frozen baseline for `Backend`'s trait-body method count, measured the
/// day quadraui#2608 landed this ratchet. **May only be lowered.** If
/// you're raising this number because a new widget needs a per-backend
/// paint path, that is exactly the case this ratchet exists to catch:
/// implement `quadraui::Primitive` (`src/primitive.rs`) instead, and reach
/// every backend through `DrawsPrimitives::draw_primitive` — see
/// `tests/primitive_public_api.rs` for a worked third-party example.
const BACKEND_METHOD_COUNT_BASELINE: usize = 203;

#[test]
fn backend_trait_method_count_may_only_shrink() {
    let file = parse_rust_file(&repo_root().join("src/backend.rs"));
    let backend_trait = find_trait(&file, "Backend");
    let methods = trait_method_names(backend_trait);

    assert!(
        methods.len() <= BACKEND_METHOD_COUNT_BASELINE,
        "Backend's trait body grew from {BACKEND_METHOD_COUNT_BASELINE} methods to {} — this \
         repo ratchets that count down, never up (quadraui#2608). A new widget should \
         implement `quadraui::Primitive` (src/primitive.rs) and reach every backend through \
         `DrawsPrimitives::draw_primitive`, not add a new `draw_*`/`*_layout` pair to the \
         sealed `Backend` trait. If this growth is genuinely unavoidable, say why in the PR \
         and lower nothing; if it isn't, switch to `Primitive` instead.\n\n\
         Methods found: {:#?}",
        methods.len(),
        methods,
    );

    if methods.len() < BACKEND_METHOD_COUNT_BASELINE {
        println!(
            "backend_method_ratchet: Backend now has {} methods, below the frozen baseline of \
             {BACKEND_METHOD_COUNT_BASELINE} — lower BACKEND_METHOD_COUNT_BASELINE in \
             tests/backend_method_ratchet.rs to this file's count to re-tighten the ratchet.",
            methods.len()
        );
    }
}

/// `(Primitive-implementing type name, legacy Backend method name)` pairs
/// that predate this ratchet and are allowed to keep their bespoke
/// `draw_*`/`*_layout` path alongside their `Primitive` impl.
///
/// `ListView` is quadraui's pilot `Primitive` implementor (quadraui#2647):
/// it kept its pre-existing `draw_list` / `draw_list_with_state` /
/// `list_layout` trio on `Backend` because every existing caller already
/// goes through those, and ripping them out is a separate, larger
/// migration — not something this ratchet forces. A *new* `Primitive`
/// implementor gets no such exemption: it should reach every backend
/// through `draw_primitive` alone.
///
/// `DataTable`, `TreeView` and `TreeTable` extend that same pilot
/// (quadraui#2607): each gained a `Primitive` impl that routes `paint`
/// through its existing `native_surface_paint` helper, but kept its
/// pre-existing bespoke `Backend` methods for the same reason `ListView`
/// did — every existing caller already goes through those, and ripping
/// them out is its own migration.
const KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS: &[(&str, &str)] = &[
    ("ListView", "draw_list"),
    ("ListView", "draw_list_with_state"),
    ("ListView", "list_layout"),
    ("DataTable", "draw_data_table"),
    ("DataTable", "draw_data_table_with_state"),
    ("DataTable", "draw_data_table_with_glyphs"),
    ("DataTable", "data_table_layout"),
    ("TreeView", "draw_tree"),
    ("TreeView", "draw_tree_with_state"),
    ("TreeView", "tree_layout"),
    ("TreeTable", "draw_tree"),
    ("TreeTable", "draw_tree_with_state"),
    ("TreeTable", "draw_tree_table"),
    ("TreeTable", "tree_table_layout"),
    ("TreeTable", "tree_layout"),
];

/// Render a `syn::Type` naming a bare local type (e.g. `ListView`) back to
/// its identifier, or `None` for anything more exotic (a reference, a
/// generic, a path with multiple segments, …) — every `impl Primitive for
/// X` in this crate today names a bare local struct, so this deliberately
/// does not try to handle more.
fn bare_type_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            p.path.segments.last().map(|seg| seg.ident.to_string())
        }
        _ => None,
    }
}

/// Whether an `impl` block's trait path names `Primitive` — matches both
/// `Primitive` (direct `use`) and a qualified `crate::primitive::Primitive`
/// spelling, the same way `list.rs` writes it.
fn impl_trait_is_primitive(item: &ItemImpl) -> bool {
    item.trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .map(|seg| seg.ident == "Primitive")
        .unwrap_or(false)
}

/// Every type name with an `impl Primitive for <Type>` in one parsed file.
fn primitive_impl_type_names(file: &syn::File) -> Vec<String> {
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(imp) if impl_trait_is_primitive(imp) => bare_type_ident(&imp.self_ty),
            _ => None,
        })
        .collect()
}

/// Lowercase ASCII `snake_case` of a `PascalCase`/`camelCase` identifier —
/// `ListView` -> `list_view`. `Backend`'s bespoke methods are not always
/// named after this *full* form (`draw_list`/`list_layout` for `ListView`,
/// `draw_tree`/`tree_layout` for `TreeView`); [`candidate_stems`] below
/// derives the shorter leading-word forms those methods actually use.
fn to_snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, ch) in ident.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Leading-word prefixes of a `snake_case` identifier, longest first —
/// `list_view` -> `["list_view", "list"]`, `data_table` -> `["data_table",
/// "data"]`, `swatch` -> `["swatch"]`. A bespoke `Backend` method is named
/// after one of these stems, not necessarily the type's full snake_case
/// name, so every candidate below is checked against each stem.
fn candidate_stems(snake: &str) -> Vec<String> {
    let words: Vec<&str> = snake.split('_').collect();
    (1..=words.len())
        .rev()
        .map(|n| words[..n].join("_"))
        .collect()
}

/// Whether `method` is a `Backend` method shaped like a bespoke
/// `draw_<stem>`, `<stem>_layout`, or `draw_<stem>_with_*` state variant
/// (e.g. `draw_list_with_state`) for any leading-word stem of `type_name`.
fn bespoke_method_for_type(method: &str, type_name: &str) -> bool {
    let snake = to_snake_case(type_name);
    candidate_stems(&snake).iter().any(|stem| {
        *method == format!("draw_{stem}")
            || *method == format!("{stem}_layout")
            || method.starts_with(&format!("draw_{stem}_with"))
    })
}

/// For every `type_name` in `primitive_type_names`, find `Backend` methods
/// in `backend_methods` that match a bespoke `draw_*`/`*_layout` shape for
/// one of its stems and are not exempted by `known`. Returns
/// `(type_name, method)` for each violation — pulled out of the test body
/// so it can be driven with synthetic inputs in the `tests` module below.
fn find_bespoke_violations<'a>(
    primitive_type_names: &'a [String],
    backend_methods: &'a [String],
    known: &std::collections::HashSet<(&str, &str)>,
) -> Vec<(&'a str, &'a str)> {
    let mut violations = Vec::new();
    for type_name in primitive_type_names {
        for method in backend_methods {
            if !bespoke_method_for_type(method, type_name) {
                continue;
            }
            if known.contains(&(type_name.as_str(), method.as_str())) {
                continue;
            }
            violations.push((type_name.as_str(), method.as_str()));
        }
    }
    violations
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

#[test]
fn primitives_dir_types_skip_bespoke_backend_methods() {
    let root = repo_root();

    let mut primitive_files = Vec::new();
    collect_rs_files(&root.join("src/primitives"), &mut primitive_files);
    assert!(
        !primitive_files.is_empty(),
        "expected at least one .rs file under src/primitives/"
    );

    let mut primitive_types: Vec<(String, PathBuf)> = Vec::new();
    for path in &primitive_files {
        let file = parse_rust_file(path);
        for name in primitive_impl_type_names(&file) {
            primitive_types.push((name, path.clone()));
        }
    }
    assert!(
        !primitive_types.is_empty(),
        "expected at least one `impl Primitive for ...` under src/primitives/ (ListView's \
         pilot impl, quadraui#2647) — none found; did it move or get renamed?"
    );

    let backend_file = parse_rust_file(&root.join("src/backend.rs"));
    let backend_trait = find_trait(&backend_file, "Backend");
    let backend_methods = trait_method_names(backend_trait);

    let known: std::collections::HashSet<(&str, &str)> = KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS
        .iter()
        .copied()
        .collect();

    let type_names: Vec<String> = primitive_types
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let violating = find_bespoke_violations(&type_names, &backend_methods, &known);

    let violations: Vec<String> = violating
        .iter()
        .map(|(type_name, method)| {
            let path = primitive_types
                .iter()
                .find(|(name, _)| name == type_name)
                .map(|(_, p)| p.display().to_string())
                .unwrap_or_default();
            format!(
                "`{type_name}` (impl Primitive in {path}) has a bespoke `Backend::{method}` that \
                 is not in KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS"
            )
        })
        .collect();

    assert!(
        violations.is_empty(),
        "found Primitive implementor(s) with a stray per-backend draw path alongside their \
         Primitive impl (quadraui#2608): a widget that implements `quadraui::Primitive` must \
         reach every backend through `DrawsPrimitives::draw_primitive` alone, not a fresh \
         `draw_*`/`*_layout` pair on the sealed `Backend` trait. If this pair genuinely \
         predates the Primitive impl and is deliberate legacy debt (the `ListView` shape), add \
         it to KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS with a comment explaining why; if it's \
         new, delete the bespoke methods and rely on `draw_primitive` instead:\n{}",
        violations.join("\n"),
    );
}

mod tests {
    use super::*;

    #[test]
    fn to_snake_case_matches_existing_backend_method_convention() {
        assert_eq!(to_snake_case("ListView"), "list_view");
        assert_eq!(to_snake_case("TreeView"), "tree_view");
        assert_eq!(to_snake_case("DataTable"), "data_table");
        assert_eq!(to_snake_case("Swatch"), "swatch");
    }

    #[test]
    fn candidate_stems_orders_longest_first() {
        assert_eq!(candidate_stems("list_view"), vec!["list_view", "list"]);
        assert_eq!(candidate_stems("data_table"), vec!["data_table", "data"]);
        assert_eq!(candidate_stems("swatch"), vec!["swatch"]);
    }

    #[test]
    fn bespoke_method_for_type_matches_a_leading_word_stem() {
        // `FooView`'s bespoke `draw_foo`/`foo_layout` should be caught even
        // though the type's full snake_case form is `foo_view` — this
        // mirrors ListView's real `draw_list`/`list_layout` pair, which is
        // named after the `list` stem, not the full `list_view` name.
        assert!(bespoke_method_for_type("draw_foo", "FooView"));
        assert!(bespoke_method_for_type("foo_layout", "FooView"));
        assert!(bespoke_method_for_type("draw_foo_with_state", "FooView"));
        assert!(!bespoke_method_for_type("draw_bar", "FooView"));
        assert!(!bespoke_method_for_type("foo_view_char_width", "FooView"));
    }

    #[test]
    fn find_bespoke_violations_catches_a_non_grandfathered_pair() {
        let types = vec!["FooView".to_string()];
        let methods = vec![
            "draw_foo".to_string(),
            "foo_layout".to_string(),
            "unrelated_method".to_string(),
        ];
        let known = std::collections::HashSet::new();

        let violations = find_bespoke_violations(&types, &methods, &known);

        assert_eq!(
            violations,
            vec![("FooView", "draw_foo"), ("FooView", "foo_layout")]
        );
    }

    #[test]
    fn find_bespoke_violations_grandfathers_a_known_pair() {
        let types = vec!["FooView".to_string()];
        let methods = vec!["draw_foo".to_string(), "foo_layout".to_string()];
        let known: std::collections::HashSet<(&str, &str)> =
            [("FooView", "draw_foo"), ("FooView", "foo_layout")]
                .into_iter()
                .collect();

        let violations = find_bespoke_violations(&types, &methods, &known);

        assert!(violations.is_empty());
    }

    #[test]
    fn find_bespoke_violations_would_catch_the_real_list_view_pair_if_ungrandfathered() {
        // Mirrors ListView's actual `Backend` methods: without the
        // grandfather entries, this is exactly the shape the ratchet must
        // flag for any *new* `Primitive` implementor.
        let types = vec!["ListView".to_string()];
        let methods = vec![
            "draw_list".to_string(),
            "list_layout".to_string(),
            "draw_list_with_state".to_string(),
        ];
        let known = std::collections::HashSet::new();

        let violations = find_bespoke_violations(&types, &methods, &known);

        assert_eq!(violations.len(), 3);
    }

    #[test]
    fn bare_type_ident_reads_a_simple_struct_name() {
        let ty: Type = syn::parse_str("ListView").unwrap();
        assert_eq!(bare_type_ident(&ty), Some("ListView".to_string()));
    }

    #[test]
    fn impl_trait_is_primitive_matches_qualified_path() {
        let item: ItemImpl = syn::parse_str(
            "impl crate::primitive::Primitive for ListView { type Layout = (); type Event = (); \
             fn layout(&self, _c: &LayoutCtx, _r: Rect) {} fn paint(&self, _l: &(), _s: &mut dyn \
             PaintSurface, _t: &Theme, _st: &Style) {} }",
        )
        .unwrap();
        assert!(impl_trait_is_primitive(&item));
    }

    #[test]
    fn impl_trait_is_primitive_rejects_a11y_primitive() {
        let item: ItemImpl = syn::parse_str(
            "impl A11yPrimitive for ListView { fn a11y_info(&self) -> A11yInfo { \
             A11yInfo::default() } }",
        )
        .unwrap();
        assert!(!impl_trait_is_primitive(&item));
    }
}
