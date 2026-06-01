//! Pure-computation diff module.
//!
//! Implements a Myers LCS diff and provides [`compute_hunks`] — the
//! public entry point for building [`crate::primitives::diff_view::DiffHunk`]
//! slices from two text strings.
//!
//! This module has **no platform dependencies** and is unconditionally
//! compiled (no feature gate). The diff algorithm is ported from the
//! reference implementation in vimcode (`src/core/engine/mod.rs`).
//!
//! # Algorithm overview
//!
//! 1. Split `left` and `right` into lines.
//! 2. Run Myers diff (`lcs_diff`) → per-line status arrays `(da, db)`.
//! 3. Run `build_aligned_diff(da, db)` → paired `(aligned_a, aligned_b)`
//!    sequences of equal length where each position maps one left line to
//!    one right line (with `None` for padding rows).
//! 4. Classify each position as `Same`, `Changed`, `Removed`, or `Added`.
//! 5. Group into hunks with `CONTEXT_LINES` = 3 lines of context around
//!    each change region.

use crate::primitives::diff_view::{DiffHunk, DiffRow, DiffRowKind};

/// Number of unchanged context lines to include around each change region.
const CONTEXT_LINES: usize = 3;

// ── Internal types ────────────────────────────────────────────────────────────

/// Internal per-line diff status (mirrors vimcode's `DiffLine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineDiff {
    Same,
    Removed,
    Added,
}

/// Internal aligned-diff entry (mirrors vimcode's `AlignedDiffEntry`).
/// `source_line` is the 0-based index into the original line slice, or
/// `None` for a padding row.
#[derive(Debug, Clone, Copy)]
struct AlignedEntry {
    source_line: Option<usize>,
}

// ── Myers diff ────────────────────────────────────────────────────────────────

/// Myers LCS diff. Returns `(da, db)` where each element is
/// `Same / Removed / Added`.
///
/// Falls back to all-`Same` if the edit distance exceeds `MAX_EDIT_DIST`
/// (avoids pathological runtime on completely unrelated files).
fn lcs_diff<'a>(a: &[&'a str], b: &[&'a str]) -> (Vec<LineDiff>, Vec<LineDiff>) {
    let n = a.len();
    let m = b.len();
    if n == 0 && m == 0 {
        return (vec![], vec![]);
    }
    if n == 0 {
        return (vec![], vec![LineDiff::Added; m]);
    }
    if m == 0 {
        return (vec![LineDiff::Removed; n], vec![]);
    }

    const MAX_EDIT_DIST: usize = 2_000;
    let max_d = (n + m).min(MAX_EDIT_DIST);

    let offset = max_d;
    let v_size = 2 * max_d + 1;
    let mut v = vec![0usize; v_size];

    let mut trace: Vec<Vec<usize>> = Vec::with_capacity(max_d);

    let mut found_d = None;
    'outer: for d in 0..=max_d {
        trace.push(v.clone());

        for k in (-(d as isize)..=(d as isize)).step_by(2) {
            let ki = (k + offset as isize) as usize;

            let mut x = if d == 0 {
                0
            } else if k == -(d as isize) || (k != d as isize && v[ki - 1] < v[ki + 1]) {
                v[ki + 1]
            } else {
                v[ki - 1] + 1
            };

            let mut y = (x as isize - k) as usize;

            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }

            v[ki] = x;

            if x >= n && y >= m {
                found_d = Some(d);
                break 'outer;
            }
        }
    }

    if found_d.is_none() {
        return (vec![LineDiff::Same; n], vec![LineDiff::Same; m]);
    }
    let d = found_d.unwrap();

    #[derive(Clone, Copy)]
    enum Edit {
        Insert(usize),
        Delete(usize),
    }
    let mut edits: Vec<Edit> = Vec::with_capacity(d);
    let mut cx = n;
    let mut cy = m;

    for d_step in (1..=d).rev() {
        let v_d = &trace[d_step];
        let k = cx as isize - cy as isize;
        let ki = (k + offset as isize) as usize;

        let is_insert =
            k == -(d_step as isize) || (k != d_step as isize && v_d[ki - 1] < v_d[ki + 1]);

        let prev_k = if is_insert { k + 1 } else { k - 1 };
        let prev_ki = (prev_k + offset as isize) as usize;
        let prev_x = v_d[prev_ki];
        let prev_y = (prev_x as isize - prev_k) as usize;

        if is_insert {
            edits.push(Edit::Insert(prev_y));
        } else {
            edits.push(Edit::Delete(prev_x));
        }

        cx = prev_x;
        cy = prev_y;
    }
    edits.reverse();

    let mut da = vec![LineDiff::Same; n];
    let mut db = vec![LineDiff::Same; m];
    for edit in &edits {
        match *edit {
            Edit::Delete(x) => da[x] = LineDiff::Removed,
            Edit::Insert(y) => db[y] = LineDiff::Added,
        }
    }

    (da, db)
}

/// Align the two per-file diff-status arrays so that `Same` lines appear at
/// the same visual row. Padding entries (`source_line = None`) are inserted
/// on the shorter side of a change run.
///
/// Returns `(aligned_a, aligned_b)` — both the same length.
fn build_aligned_diff(da: &[LineDiff], db: &[LineDiff]) -> (Vec<AlignedEntry>, Vec<AlignedEntry>) {
    let mut aligned_a = Vec::new();
    let mut aligned_b = Vec::new();
    let mut i = 0;
    let mut j = 0;

    while i < da.len() || j < db.len() {
        if i < da.len() && j < db.len() && da[i] == LineDiff::Same && db[j] == LineDiff::Same {
            aligned_a.push(AlignedEntry {
                source_line: Some(i),
            });
            aligned_b.push(AlignedEntry {
                source_line: Some(j),
            });
            i += 1;
            j += 1;
            continue;
        }

        let mut removed = Vec::new();
        let mut added = Vec::new();
        while i < da.len() && da[i] != LineDiff::Same {
            removed.push(i);
            i += 1;
        }
        while j < db.len() && db[j] != LineDiff::Same {
            added.push(j);
            j += 1;
        }

        if removed.is_empty() && added.is_empty() {
            if i < da.len() {
                aligned_a.push(AlignedEntry {
                    source_line: Some(i),
                });
                aligned_b.push(AlignedEntry { source_line: None });
                i += 1;
            }
            if j < db.len() {
                aligned_a.push(AlignedEntry { source_line: None });
                aligned_b.push(AlignedEntry {
                    source_line: Some(j),
                });
                j += 1;
            }
            continue;
        }

        let max_len = removed.len().max(added.len());
        for k in 0..max_len {
            if k < removed.len() {
                aligned_a.push(AlignedEntry {
                    source_line: Some(removed[k]),
                });
            } else {
                aligned_a.push(AlignedEntry { source_line: None });
            }
            if k < added.len() {
                aligned_b.push(AlignedEntry {
                    source_line: Some(added[k]),
                });
            } else {
                aligned_b.push(AlignedEntry { source_line: None });
            }
        }
    }

    (aligned_a, aligned_b)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute diff hunks from two text strings.
///
/// Splits `left` and `right` on `\n`, runs the Myers LCS diff, aligns the
/// result into paired rows, classifies each row, and groups the rows into
/// hunks with [`CONTEXT_LINES`] lines of context around each change region.
///
/// Returns an empty `Vec` when the two strings are identical.
pub fn compute_hunks(left: &str, right: &str) -> Vec<DiffHunk> {
    let left_lines: Vec<&str> = left.split('\n').collect();
    let right_lines: Vec<&str> = right.split('\n').collect();

    let (da, db) = lcs_diff(&left_lines, &right_lines);
    let (aligned_a, aligned_b) = build_aligned_diff(&da, &db);

    let n = aligned_a.len();
    if n == 0 {
        return vec![];
    }

    // Build flat row list with kind classification.
    let mut rows: Vec<DiffRow> = Vec::with_capacity(n);
    for idx in 0..n {
        let la = &aligned_a[idx];
        let lb = &aligned_b[idx];
        let left_text = la.source_line.map(|li| left_lines[li].to_string());
        let right_text = lb.source_line.map(|ri| right_lines[ri].to_string());

        let kind = match (la.source_line, lb.source_line) {
            (Some(li), Some(_)) => {
                // Both sides have content. Same-Same pairs come from matching
                // lines (da[li] == LineDiff::Same). Removed-Added pairs come
                // from a change hunk and classify as Changed.
                if li < da.len() && da[li] == LineDiff::Same {
                    DiffRowKind::Same
                } else {
                    DiffRowKind::Changed
                }
            }
            (Some(_), None) => DiffRowKind::Removed,
            (None, Some(_)) => DiffRowKind::Added,
            (None, None) => DiffRowKind::Same, // degenerate padding; treat as Same
        };

        rows.push(DiffRow {
            left: left_text,
            right: right_text,
            kind,
        });
    }

    // Find all "changed" row indices.
    let change_indices: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind != DiffRowKind::Same)
        .map(|(i, _)| i)
        .collect();

    if change_indices.is_empty() {
        return vec![];
    }

    // Expand each change index into a context interval [start, end].
    let mut intervals: Vec<(usize, usize)> = change_indices
        .iter()
        .map(|&i| {
            let start = i.saturating_sub(CONTEXT_LINES);
            let end = (i + CONTEXT_LINES).min(n - 1);
            (start, end)
        })
        .collect();

    // Merge overlapping intervals.
    intervals.sort_unstable_by_key(|&(s, _)| s);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    // Build one DiffHunk per merged interval.
    merged
        .into_iter()
        .map(|(start, end)| {
            let hunk_rows = rows[start..=end].to_vec();

            // 1-based left_start: first row with left content.
            let left_start = hunk_rows
                .iter()
                .find_map(|r| r.left.as_ref().map(|_| ()))
                .map(|_| rows[..start].iter().filter(|r| r.left.is_some()).count() + 1)
                .unwrap_or(1);

            // 1-based right_start: first row with right content.
            let right_start = hunk_rows
                .iter()
                .find_map(|r| r.right.as_ref().map(|_| ()))
                .map(|_| rows[..start].iter().filter(|r| r.right.is_some()).count() + 1)
                .unwrap_or(1);

            DiffHunk {
                left_start,
                right_start,
                rows: hunk_rows,
            }
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hunks_identical() {
        let text = "line one\nline two\nline three";
        let hunks = compute_hunks(text, text);
        assert!(
            hunks.is_empty(),
            "identical texts should produce no hunks, got {hunks:?}"
        );
    }

    #[test]
    fn test_compute_hunks_both_empty() {
        let hunks = compute_hunks("", "");
        assert!(hunks.is_empty(), "both-empty should produce no hunks");
    }

    #[test]
    fn test_compute_hunks_add_only() {
        // Left is truly empty (no lines), right has 3 lines.
        // lcs_diff handles n=0 specially — all right lines are Added.
        let right = "line one\nline two\nline three";
        let left_lines: Vec<&str> = vec![];
        let right_lines: Vec<&str> = right.split('\n').collect();
        let (da, db) = lcs_diff(&left_lines, &right_lines);
        // da is empty (no left lines), all db entries are Added.
        assert!(da.is_empty());
        assert!(db.iter().all(|d| *d == LineDiff::Added));

        // compute_hunks with left="" splits into [""], giving one empty left
        // line, but right has 3 different lines — all rows should be non-Same.
        let hunks = compute_hunks("", right);
        assert!(
            !hunks.is_empty(),
            "add-only diff should produce at least one hunk"
        );
        let has_change = hunks
            .iter()
            .any(|h| h.rows.iter().any(|r| r.kind != DiffRowKind::Same));
        assert!(
            has_change,
            "add-only diff should have at least one non-Same row"
        );
    }

    #[test]
    fn test_compute_hunks_remove_only() {
        // Left has 3 lines, right is truly empty.
        let left = "line one\nline two\nline three";
        let left_lines: Vec<&str> = left.split('\n').collect();
        let right_lines: Vec<&str> = vec![];
        let (da, db) = lcs_diff(&left_lines, &right_lines);
        // All left lines are Removed, db is empty.
        assert!(da.iter().all(|d| *d == LineDiff::Removed));
        assert!(db.is_empty());

        // compute_hunks with right="" produces similar all-removal result.
        let hunks = compute_hunks(left, "");
        assert!(
            !hunks.is_empty(),
            "remove-only diff should produce at least one hunk"
        );
        let has_change = hunks
            .iter()
            .any(|h| h.rows.iter().any(|r| r.kind != DiffRowKind::Same));
        assert!(
            has_change,
            "remove-only diff should have at least one non-Same row"
        );
    }

    #[test]
    fn test_compute_hunks_mixed() {
        let left = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    println!(\"{}\", add(3, 4));\n}";
        let right = "fn add(a: i32, b: i32) -> i64 {\n    (a + b) as i64\n}\n\nfn subtract(a: i32, b: i32) -> i32 {\n    a - b\n}\n\nfn main() {\n    println!(\"{}\", add(3, 4));\n    println!(\"{}\", subtract(10, 3));\n}";
        let hunks = compute_hunks(left, right);
        assert!(
            !hunks.is_empty(),
            "mixed diff should produce at least one hunk"
        );
        // There should be at least one Changed or Added row somewhere.
        let has_change = hunks
            .iter()
            .any(|h| h.rows.iter().any(|r| r.kind != DiffRowKind::Same));
        assert!(has_change, "mixed diff should contain non-Same rows");
    }
}
