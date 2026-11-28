//! Source-walking helpers shared by the tests that read the crate's own
//! Rust source with `syn` (the backend ratchets, the widget-contract
//! census and the duplicate-symbol deny-list), so every one of them agrees
//! on which files exist and which items are test-only.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Parses `path` as a Rust source file, panicking with the path on failure.
pub fn parse_rust_file(path: &Path) -> syn::File {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()))
}

/// Appends every `.rs` file under `dir`, recursively, in sorted order. A
/// directory that cannot be read panics: a gate scanning nothing passes
/// vacuously.
pub fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

/// Whether an attribute is `#[cfg(test)]` or `#[cfg(all(test, …))]`.
/// Deliberately does not recurse into `not(…)` or `any(…)`:
/// `cfg(not(test))` is active in non-test builds, and `cfg(any(test, …))`
/// can be true without `test`, so neither marks a test-only item. Only a
/// direct `test` conjunct inside `all(…)` counts.
pub fn is_cfg_test(attr: &syn::Attribute) -> bool {
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
