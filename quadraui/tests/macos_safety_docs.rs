//! Checks that every `pub unsafe fn` in `src/macos/` has a `# Safety` doc section.
//!
//! This test runs on every platform and catches regressions that the `macos-latest`
//! CI runner would miss, since `src/macos/` is only compiled when both the `macos`
//! feature and `target_os = "macos"` are set.
//!
//! Only bare `pub unsafe fn` is checked: `pub(crate)` and `pub(super)` helpers
//! are out of scope, as they are for `clippy::missing_safety_doc`.

use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src/macos/`, recursively.
fn macos_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&manifest_dir().join("src/macos"), &mut out);
    out.sort();
    assert!(
        !out.is_empty(),
        "expected .rs files under quadraui/src/macos — did the module move?"
    );
    out
}

/// Is `line` the signature line of an exported `unsafe fn`?
///
/// Matches bare `pub unsafe fn` only — see the module doc's *Scope*. The
/// leading-whitespace trim keeps nested (e.g. `impl`-block) items in play.
fn is_exported_unsafe_fn(line: &str) -> bool {
    line.trim_start().starts_with("pub unsafe fn ")
}

/// The contiguous doc-comment-plus-attribute block immediately above
/// `lines[idx]`, nearest line first.
///
/// `rustfmt` emits the doc comment directly above any attributes with no
/// blank line between them, so walking up until a blank line — or until a
/// line that closes a previous item (`}` / `;` / `{`) — captures the whole
/// block and nothing from the item before it. That matters because
/// attributes here span multiple lines (`#[deprecated(since = …, note =
/// …)]`), which a naive "skip one attribute line" walk would read straight
/// past and into the previous function's docs.
fn doc_block_above(lines: &[&str], idx: usize) -> Vec<String> {
    let mut block = Vec::new();
    for line in lines[..idx].iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.ends_with('{')
            || trimmed.ends_with('}')
            || trimmed.ends_with(';')
        {
            break;
        }
        block.push(trimmed.to_string());
    }
    block
}

fn has_safety_section(block: &[String]) -> bool {
    block
        .iter()
        .any(|l| l.starts_with("///") && l.trim_start_matches('/').trim() == "# Safety")
}

#[test]
fn every_exported_unsafe_fn_in_macos_has_a_safety_doc() {
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for path in macos_sources() {
        let src = fs::read_to_string(&path).expect("read source");
        let lines: Vec<&str> = src.lines().collect();
        let rel = path
            .strip_prefix(manifest_dir())
            .unwrap_or(&path)
            .display()
            .to_string();

        for (i, line) in lines.iter().enumerate() {
            if !is_exported_unsafe_fn(line) {
                continue;
            }
            checked += 1;
            if !has_safety_section(&doc_block_above(&lines, i)) {
                failures.push(format!(
                    "{rel}:{}: `{}` has no `# Safety` doc section — \
                     `clippy::missing_safety_doc` denies this under CI's \
                     `-D warnings`, but only on the macos-latest runner. Add a \
                     `/// # Safety` section (the convention in this module is \
                     \"`ctx` must be a valid `CGContextRef` borrowed for the \
                     duration of the call.\"). If you are *editing* an existing \
                     doc comment, add to it — do not replace the section.",
                    i + 1,
                    line.trim()
                ));
            }
        }
    }

    // A bug in the scanner that silently matched nothing would make this
    // test permanently green and useless; src/macos is full of
    // `pub unsafe fn draw_*` rasterisers.
    assert!(
        checked > 20,
        "expected many `pub unsafe fn`s under src/macos, found {checked} — \
         is_exported_unsafe_fn() has probably stopped matching"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The scanner itself: both halves of the claim this test makes, proven on
/// fixture text rather than on the tree it guards (where a pass is
/// indistinguishable from a no-op).
#[test]
fn scanner_distinguishes_documented_from_undocumented() {
    let documented: Vec<&str> = vec![
        "/// Paint `view` into `(x, y, w, h)` on `ctx`.",
        "///",
        "/// # Safety",
        "///",
        "/// `ctx` must be a valid `CGContextRef` borrowed for the duration of",
        "/// the call.",
        "#[deprecated(",
        "    since = \"0.0.1\",",
        "    note = \"call `Backend::draw_form` instead\"",
        ")]",
        "#[allow(clippy::too_many_arguments)]",
        "pub unsafe fn draw_thing(",
    ];
    let idx = documented.len() - 1;
    assert!(is_exported_unsafe_fn(documented[idx]));
    assert!(
        has_safety_section(&doc_block_above(&documented, idx)),
        "multi-line attributes must not hide the `# Safety` section above them"
    );

    // A missing `# Safety` section can occur when a doc comment is rewritten
    // and a parameter note replaces the safety section instead of being added to it.
    let undocumented: Vec<&str> = vec![
        "/// Paint `view` into `(x, y, w, h)` on `ctx`.",
        "///",
        "/// `nerd_fonts_enabled` is forwarded to an embedded toolbar.",
        "#[allow(clippy::too_many_arguments)]",
        "pub unsafe fn draw_thing(",
    ];
    let idx = undocumented.len() - 1;
    assert!(!has_safety_section(&doc_block_above(&undocumented, idx)));

    // A previous item's `# Safety` must not be read as this item's: the
    // blank line between them ends the block.
    let neighbour: Vec<&str> = vec![
        "/// # Safety",
        "///",
        "/// `ctx` must be valid.",
        "pub unsafe fn previous(ctx: CGContextRef) {}",
        "",
        "/// Undocumented.",
        "pub unsafe fn next(",
    ];
    let idx = neighbour.len() - 1;
    assert!(!has_safety_section(&doc_block_above(&neighbour, idx)));

    // `pub(crate)` is out of scope, matching clippy's own.
    assert!(!is_exported_unsafe_fn(
        "pub(crate) unsafe fn ns_fill_rect(ctx: CGContextRef) {}"
    ));
}
