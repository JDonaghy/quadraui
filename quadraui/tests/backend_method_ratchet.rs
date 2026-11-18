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
//!    carries no bespoke `draw_<x>`/`<x>_layout` pair for `X`, except the
//!    pre-existing legacy pairs this test grandfathers explicitly.

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
const KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS: &[(&str, &str)] = &[
    ("ListView", "draw_list"),
    ("ListView", "draw_list_with_state"),
    ("ListView", "list_layout"),
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
/// `ListView` -> `list_view`, matching the convention every built-in
/// primitive's own `draw_*` method name already follows (`draw_tree` for
/// `TreeView`, `draw_data_table` for `DataTable`, …).
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

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
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

    let mut violations = Vec::new();
    for (type_name, path) in &primitive_types {
        let snake = to_snake_case(type_name);
        let draw_name = format!("draw_{snake}");
        let layout_name = format!("{snake}_layout");
        for method in &backend_methods {
            let matches_shape = *method == draw_name || *method == layout_name;
            if !matches_shape {
                continue;
            }
            if known.contains(&(type_name.as_str(), method.as_str())) {
                continue;
            }
            violations.push(format!(
                "`{type_name}` (impl Primitive in {}) has a bespoke `Backend::{method}` that is \
                 not in KNOWN_LEGACY_PRIMITIVE_BACKEND_METHODS",
                path.display()
            ));
        }
    }

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

#[cfg(test)]
mod self_test {
    use super::*;

    #[test]
    fn to_snake_case_matches_existing_backend_method_convention() {
        assert_eq!(to_snake_case("ListView"), "list_view");
        assert_eq!(to_snake_case("TreeView"), "tree_view");
        assert_eq!(to_snake_case("DataTable"), "data_table");
        assert_eq!(to_snake_case("Swatch"), "swatch");
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
