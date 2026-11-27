//! Every widget kind is on the whole contract: `Primitive` **and**
//! `Interactive`.
//!
//! The census parses the crate's real module tree with [`syn`], starting
//! at `src/lib.rs` and following each `mod` declaration, and collects two
//! sets of widget types:
//!
//! - every type named in the `widget_kinds!` invocation in
//!   `src/widget_kinds.rs` (the kinds a binding can decode by name);
//! - every type with a non-test `impl Primitive for T`.
//!
//! Each of them must also have an `impl Interactive for T` or a
//! `non_interactive!(T)`. A module gated `#[cfg(test)]` (or
//! `#[cfg(all(test, …))]`), inline or out-of-line, is skipped, so a
//! test-only `Primitive` impl is not a widget kind. Every other `cfg` is
//! treated as present: the census is over the source, not over one feature
//! set's build.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::parse::{Parse, ParseStream};
use syn::{Item, Type};

fn src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn parse_rust_file(path: &Path) -> syn::File {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()))
}

/// Whether an attribute is `#[cfg(test)]` or `#[cfg(all(test, …))]`. A
/// `not(…)` or `any(…)` can be true without `test`, so neither counts.
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

/// The last path segment of a type: `ListView` for `crate::ListView`,
/// `Foo` for `Foo<T>`.
fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            p.path.segments.last().map(|seg| seg.ident.to_string())
        }
        _ => None,
    }
}

fn trait_name(imp: &syn::ItemImpl) -> Option<String> {
    imp.trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .map(|seg| seg.ident.to_string())
}

/// What the census found in the non-test module tree.
#[derive(Default)]
struct Census {
    /// Type name → the file holding its `impl Primitive`.
    primitive: BTreeMap<String, PathBuf>,
    /// Types with an `impl Interactive` or a `non_interactive!`.
    interactive: BTreeSet<String>,
    /// The `widget_kinds!` entries, kind string → type name.
    kinds: BTreeMap<String, String>,
}

/// One `widget_kinds!` entry: `#[attrs]* "kind" => path::To::Type`.
struct KindEntry {
    kind: String,
    ty: Type,
}

impl Parse for KindEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _attrs = input.call(syn::Attribute::parse_outer)?;
        let kind: syn::LitStr = input.parse()?;
        input.parse::<syn::Token![=>]>()?;
        let ty: Type = input.parse()?;
        Ok(Self {
            kind: kind.value(),
            ty,
        })
    }
}

fn macro_name(mac: &syn::Macro) -> Option<String> {
    mac.path.segments.last().map(|seg| seg.ident.to_string())
}

impl Census {
    /// Walks `items`, which live in the module whose own file is `file`
    /// and whose child modules resolve under `child_dir`.
    fn walk_items(&mut self, items: &[Item], file: &Path, child_dir: &Path) {
        for item in items {
            match item {
                Item::Mod(m) => {
                    if m.attrs.iter().any(is_cfg_test) {
                        continue;
                    }
                    let name = m.ident.to_string();
                    match &m.content {
                        Some((_, inner)) => self.walk_items(inner, file, &child_dir.join(&name)),
                        None => self.walk_module_file(child_dir, &name),
                    }
                }
                Item::Impl(imp) => {
                    if imp.attrs.iter().any(is_cfg_test) {
                        continue;
                    }
                    let (Some(t), Some(ty)) = (trait_name(imp), type_name(&imp.self_ty)) else {
                        continue;
                    };
                    match t.as_str() {
                        "Primitive" => {
                            self.primitive.insert(ty, file.to_path_buf());
                        }
                        "Interactive" => {
                            self.interactive.insert(ty);
                        }
                        _ => {}
                    }
                }
                Item::Macro(m) => {
                    if m.attrs.iter().any(is_cfg_test) {
                        continue;
                    }
                    match macro_name(&m.mac).as_deref() {
                        Some("non_interactive") => {
                            let ty: Type = m.mac.parse_body().unwrap_or_else(|e| {
                                panic!("{}: non_interactive! body: {e}", file.display())
                            });
                            if let Some(name) = type_name(&ty) {
                                self.interactive.insert(name);
                            }
                        }
                        Some("widget_kinds") => {
                            let entries = m
                                .mac
                                .parse_body_with(
                                    syn::punctuated::Punctuated::<KindEntry, syn::Token![,]>::parse_terminated,
                                )
                                .unwrap_or_else(|e| {
                                    panic!("{}: widget_kinds! body: {e}", file.display())
                                });
                            for entry in entries {
                                let name = type_name(&entry.ty).unwrap_or_else(|| {
                                    panic!("widget_kinds! entry {:?} names no type", entry.kind)
                                });
                                self.kinds.insert(entry.kind, name);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Resolves `mod name;` declared by a module whose children live
    /// under `dir`: `dir/name.rs` (whose children live under `dir/name/`)
    /// or `dir/name/mod.rs`.
    fn walk_module_file(&mut self, dir: &Path, name: &str) {
        let flat = dir.join(format!("{name}.rs"));
        let nested = dir.join(name).join("mod.rs");
        let path = if flat.is_file() {
            flat
        } else if nested.is_file() {
            nested
        } else {
            panic!(
                "module `{name}` under {} resolves to neither {name}.rs nor {name}/mod.rs",
                dir.display()
            );
        };
        let file = parse_rust_file(&path);
        self.walk_items(&file.items, &path, &dir.join(name));
    }

    fn of_crate() -> Self {
        let root = src_root();
        let lib = root.join("lib.rs");
        let mut census = Self::default();
        census.walk_items(&parse_rust_file(&lib).items, &lib, &root);
        census
    }
}

#[test]
fn every_widget_kind_is_primitive_and_interactive() {
    let census = Census::of_crate();

    assert!(
        census.kinds.len() > 10,
        "expected the widget_kinds! table to be found and parsed; found {:?}",
        census.kinds
    );
    assert!(
        census.primitive.len() > 50,
        "expected the census to find the crate's Primitive impls; found {}",
        census.primitive.len()
    );

    let mut gaps: BTreeMap<String, String> = BTreeMap::new();
    for (kind, ty) in &census.kinds {
        if !census.primitive.contains_key(ty) {
            gaps.insert(
                ty.clone(),
                format!("widget_kinds! \"{kind}\" has no `impl Primitive`"),
            );
        } else if !census.interactive.contains(ty) {
            gaps.insert(
                ty.clone(),
                format!("widget_kinds! \"{kind}\" is not Interactive"),
            );
        }
    }
    let root = src_root();
    for (ty, file) in &census.primitive {
        if !census.interactive.contains(ty) {
            let rel = file.strip_prefix(&root).unwrap_or(file);
            gaps.entry(ty.clone()).or_insert_with(|| {
                format!(
                    "`impl Primitive` in src/{} is not Interactive",
                    rel.display()
                )
            });
        }
    }

    assert!(
        gaps.is_empty(),
        "every widget implements both `Primitive` and `Interactive` (a widget with no input \
         story uses `quadraui::non_interactive!(T)`); missing: {gaps:#?}"
    );
}

/// The walker skips a test-only module and still finds the impls around
/// it, so the census above is not vacuous.
#[test]
fn the_census_skips_test_only_modules() {
    let file: syn::File = syn::parse_quote! {
        impl Primitive for Real {}
        quadraui::non_interactive!(Real);
        #[cfg(test)]
        mod tests {
            impl Primitive for OnlyInTests {}
        }
        #[cfg(all(test, feature = "tui"))]
        mod more_tests {
            impl Primitive for AlsoOnlyInTests {}
        }
        mod inner {
            impl crate::primitive::Primitive for Nested<T> {}
            impl Interactive for Nested<T> {}
        }
    };
    let mut census = Census::default();
    census.walk_items(&file.items, Path::new("lib.rs"), Path::new("."));
    assert_eq!(
        census.primitive.keys().cloned().collect::<Vec<_>>(),
        vec!["Nested".to_string(), "Real".to_string()]
    );
    assert!(census.interactive.contains("Real"));
    assert!(census.interactive.contains("Nested"));
}

#[test]
fn the_census_reads_widget_kinds_entries_with_attributes() {
    let file: syn::File = syn::parse_quote! {
        widget_kinds! {
            "cell_grid" => crate::CellGrid,
            #[cfg(feature = "tui")]
            "icon_grid" => crate::IconGrid,
        }
    };
    let mut census = Census::default();
    census.walk_items(&file.items, Path::new("widget_kinds.rs"), Path::new("."));
    assert_eq!(
        census.kinds.into_iter().collect::<Vec<_>>(),
        vec![
            ("cell_grid".to_string(), "CellGrid".to_string()),
            ("icon_grid".to_string(), "IconGrid".to_string()),
        ]
    );
}
