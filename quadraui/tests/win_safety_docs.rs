//! Ratchets the presence of a `// SAFETY:` comment on every `unsafe {}`
//! block in `src/win/` (issue #1115).
//!
//! ## Why this test exists
//!
//! `src/win/` is gated `#[cfg(target_os = "windows")]` almost
//! everywhere real WinAPI calls happen (see `mod.rs`'s module docs), so
//! nothing about the *content* of an `unsafe` block is type-checked,
//! clippy'd, or even compiled on any leg that runs on Linux — `cargo
//! check -p quadraui --features win` on the `win` leg only proves the
//! `cfg(target_os = "windows")` arms parse, per this crate's own
//! `CLAUDE.md` ("this is a TYPE-CHECK ... not a test"). Windows has no
//! equivalent of `tests/macos_safety_docs.rs`, and unlike that test this
//! one isn't riding on a pre-existing `clippy::missing_safety_doc` lint:
//! `unsafe { }` blocks (as opposed to `pub unsafe fn` signatures) have no
//! clippy lint requiring a safety comment at all, on any platform, ever.
//! The only guard against an undocumented `unsafe` block landing in
//! `src/win/` is a human noticing at review time.
//!
//! This test is deliberately a **ratchet**, not a "must be zero" gate.
//! At the time of writing there are 254 `unsafe {}` blocks in `src/win/`
//! and only 21 carry a `SAFETY:` comment — retrofitting all of them in
//! one PR is out of scope (see the issue's chunking note). Instead this
//! test freezes the current undocumented count as a ceiling: new
//! `unsafe` blocks without a `SAFETY:` comment are still allowed to
//! *exist* (so this doesn't block unrelated work elsewhere in
//! `src/win/`), but the total may only go down, never up. A PR that adds
//! a new undocumented block without documenting an old one fails this
//! test; a PR that documents blocks (as this issue's later chunks do)
//! lowers `MAX_UNDOCUMENTED_UNSAFE_BLOCKS` and the ratchet tightens.
//!
//! ## Scope
//!
//! Every `unsafe { ... }` block counts, not just `pub` items — unlike
//! `clippy::missing_safety_doc`, which only ever looks at exported
//! function signatures, a safety comment on an `unsafe` block is about
//! the invariant the call site itself relies on, and that's just as
//! true for a private helper as for a public one.
//!
//! A block counts as documented when the contiguous run of `//` line
//! comments immediately above it (no blank line, no other statement, in
//! between) contains the literal text `SAFETY:` somewhere in that run.
//! This matches the convention already in use across `src/win/`
//! (`services.rs`, `run.rs`, `tray.rs`) — see `has_safety_comment`'s doc
//! comment for the exact matching rule, including the "same as ... above"
//! shorthand for a repeated invariant.

use std::fs;
use std::path::{Path, PathBuf};

/// New undocumented `unsafe {}` blocks may not be added to `src/win/`
/// without documenting at least as many existing ones — this is the
/// ratchet. Lower this number (and only this number) whenever a chunk of
/// issue #1115 adds `SAFETY:` comments; never raise it.
///
/// History:
/// - 2026-09-29 (#1115 chunk 1): `run.rs` and `backend.rs` fully
///   documented (every `unsafe {}` block in both files now carries a
///   `SAFETY:` comment, including the `CreateMutexW`/`GetLastError`
///   single-instance-mutex pair in `run.rs::is_primary_instance` — see
///   that call site's comment for why the returned handle is
///   deliberately never closed, rather than an oversight). Remaining
///   undocumented blocks are confined to `text.rs`, `tray.rs`,
///   `services.rs`, `testing.rs`, and `image.rs`, left for follow-up
///   chunks.
/// - 2026-09-29 (#1224, #1115 chunk 2): `text.rs`, `tray.rs`,
///   `services.rs`, `testing.rs`, and `image.rs` fully documented —
///   every remaining `unsafe {}` block in `src/win/` now carries a
///   `SAFETY:` comment. This is the last chunk of #1115; the ceiling
///   drops to `0`.
const MAX_UNDOCUMENTED_UNSAFE_BLOCKS: usize = 0;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src/win/`, recursively.
fn win_sources() -> Vec<PathBuf> {
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
    walk(&manifest_dir().join("src/win"), &mut out);
    out.sort();
    assert!(
        !out.is_empty(),
        "expected .rs files under quadraui/src/win — did the module move?"
    );
    out
}

/// Does `line` open an `unsafe {}` block (as opposed to `unsafe fn` /
/// `unsafe impl` / `unsafe extern`, none of which contain the literal
/// substring `unsafe {`)?
///
/// A word-boundary check on the character before `unsafe` keeps this from
/// matching inside a longer identifier (there are none in this codebase,
/// but the check is free and future-proofs against one appearing).
fn find_unsafe_block_cols(line: &str) -> Vec<usize> {
    let mut cols = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(rel) = line[start..].find("unsafe {") {
        let idx = start + rel;
        let boundary_ok =
            idx == 0 || !(bytes[idx - 1] as char).is_ascii_alphanumeric() && bytes[idx - 1] != b'_';
        if boundary_ok {
            cols.push(idx);
        }
        start = idx + "unsafe {".len();
    }
    cols
}

/// Is the contiguous run of `//` comment lines immediately above
/// `lines[idx]` (stopping at the first blank line or non-comment line) a
/// safety comment?
///
/// "Immediately above" means no blank line and no other code between the
/// comment and the `unsafe` block — the same adjacency `services.rs` and
/// `run.rs` already use. The run may span multiple lines (wrapped prose,
/// or a short "same as the `foo` borrow above" cross-reference to a
/// fuller comment elsewhere) — any line in the run containing `SAFETY:`
/// counts.
fn has_safety_comment(lines: &[&str], idx: usize) -> bool {
    for line in lines[..idx].iter().rev() {
        let trimmed = line.trim();
        if !trimmed.starts_with("//") {
            break;
        }
        if trimmed.contains("SAFETY:") {
            return true;
        }
    }
    false
}

#[test]
fn undocumented_unsafe_blocks_in_win_may_only_decrease() {
    let mut total = 0usize;
    let mut undocumented: Vec<String> = Vec::new();

    for path in win_sources() {
        let src = fs::read_to_string(&path).expect("read source");
        let lines: Vec<&str> = src.lines().collect();
        let rel = path
            .strip_prefix(manifest_dir())
            .unwrap_or(&path)
            .display()
            .to_string();

        for (i, line) in lines.iter().enumerate() {
            let cols = find_unsafe_block_cols(line);
            if cols.is_empty() {
                continue;
            }
            total += cols.len();
            if !has_safety_comment(&lines, i) {
                for _ in &cols {
                    undocumented.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }

    // A scanner bug that stopped matching entirely would make this test
    // permanently (and uselessly) green — src/win is full of `unsafe {}`
    // blocks behind `cfg(target_os = "windows")`.
    assert!(
        total > 200,
        "expected hundreds of `unsafe {{}}` blocks under src/win, found \
         {total} — find_unsafe_block_cols() has probably stopped matching"
    );

    assert!(
        undocumented.len() <= MAX_UNDOCUMENTED_UNSAFE_BLOCKS,
        "found {} undocumented `unsafe {{}}` block(s) in src/win, ceiling is \
         {MAX_UNDOCUMENTED_UNSAFE_BLOCKS} (see this test's module doc — the \
         count may only go down). New undocumented blocks:\n{}",
        undocumented.len(),
        undocumented.join("\n")
    );
    if undocumented.len() < MAX_UNDOCUMENTED_UNSAFE_BLOCKS {
        panic!(
            "found only {} undocumented `unsafe {{}}` block(s) in src/win, \
             but MAX_UNDOCUMENTED_UNSAFE_BLOCKS is still {MAX_UNDOCUMENTED_UNSAFE_BLOCKS} — \
             lower the constant (and update its History note) to match the \
             new, tighter count so the ratchet doesn't loosen back up.",
            undocumented.len()
        );
    }
}

/// The scanner itself: both halves of the claim this test makes, proven
/// on fixture text rather than on the tree it guards.
#[test]
fn scanner_distinguishes_documented_from_undocumented() {
    let documented: Vec<&str> = vec![
        "fn win_displays() -> ServiceResult<Vec<Display>> {",
        "    let mut handles: Vec<HMONITOR> = Vec::new();",
        "    // SAFETY: `win_collect_monitor` only ever dereferences `lparam`",
        "    // as the `Vec<HMONITOR>` constructed on the line above, which",
        "    // outlives the call.",
        "    unsafe {",
    ];
    let idx = documented.len() - 1;
    assert_eq!(find_unsafe_block_cols(documented[idx]), vec![4]);
    assert!(has_safety_comment(&documented, idx));

    let undocumented: Vec<&str> = vec![
        "fn win_cursor_screen_point() -> ServiceResult<Point> {",
        "    let mut point = POINT::default();",
        "    unsafe { GetCursorPos(&mut point) }",
    ];
    let idx = undocumented.len() - 1;
    assert_eq!(find_unsafe_block_cols(undocumented[idx]), vec![4]);
    assert!(!has_safety_comment(&undocumented, idx));

    // A blank line between the comment and the block breaks the
    // adjacency — the comment might be about the statement before it,
    // not this `unsafe` block.
    let blank_separated: Vec<&str> = vec![
        "    // SAFETY: this describes something else entirely.",
        "",
        "    unsafe { GetCursorPos(&mut point) }",
    ];
    let idx = blank_separated.len() - 1;
    assert!(!has_safety_comment(&blank_separated, idx));

    // A previous statement's plain (non-SAFETY) comment must not be
    // mistaken for documentation.
    let unrelated_comment: Vec<&str> = vec![
        "    // `windows::core::BOOL` is a newtype, not a bare `i32`.",
        "    unsafe { GetMonitorInfoW(hmonitor, &mut info) }",
    ];
    let idx = unrelated_comment.len() - 1;
    assert!(!has_safety_comment(&unrelated_comment, idx));

    // The "same as ... above" shorthand counts, matching existing
    // `run.rs` usage.
    let cross_reference: Vec<&str> = vec![
        "    // SAFETY: same as the `attach_surface` borrow above.",
        "    unsafe {",
    ];
    let idx = cross_reference.len() - 1;
    assert!(has_safety_comment(&cross_reference, idx));

    // `unsafe fn` / `unsafe impl` do not open an `unsafe {}` block and
    // must not be matched.
    assert!(find_unsafe_block_cols("pub unsafe fn draw_thing() {").is_empty());
    assert!(find_unsafe_block_cols("unsafe impl Send for Foo {}").is_empty());

    // An inline `unsafe { ... }` embedded mid-expression is still a
    // block and must be found even though it isn't the start of the
    // trimmed line.
    assert_eq!(
        find_unsafe_block_cols("Ok(_handle) => (unsafe { GetLastError() }) != FLAG,"),
        vec![16]
    );
}
