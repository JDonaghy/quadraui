#!/usr/bin/env python3
"""Enforce a `CHANGELOG.md` `[Unreleased]` entry for quadraui behavior/API
changes (#1500).

Preparing the 0.1.4 release found 3 of the last 5 merged changes had no
changelog entry at all, and one was filed under the wrong section -- nothing
enforced `CHANGELOG.md`'s own stated policy ("every PR ... adds an entry
here in the same PR", see that file's *Scope* section), so every release
needed a hand repair of the notes. This script is that enforcement, run as
a CI job on every PR into `develop`/`main`.

Rule: if the diff between `BASE` and `HEAD` changes any non-test Rust file
under `quadraui/src/**`, the diff must also add at least one non-blank line
inside `CHANGELOG.md`'s `## [Unreleased]` section.

A changed `quadraui/src/**` file is exempt from needing an entry when:

  - its diff is comment-only, per `tools/comment_only_check.py`'s own
    strip-and-compare logic (reused here, not reimplemented);
  - every hunk in its diff falls entirely inside a trailing
    `#[cfg(test)] mod tests { ... }` block, in both the base and head
    revisions where that revision has one -- CLAUDE.md's own convention
    ("Tests in `#[cfg(test)]` mod tests at file bottom") is what makes "the
    first `#[cfg(test)]` line to EOF" a safe proxy for "test code" without
    a full Rust parse;
  - its path falls under `quadraui/examples/`, `quadraui/fuzz/`, `.github/`,
    `tools/`, `scripts/`, or any `docs/` directory (listed for completeness
    -- none of these live under `quadraui/src/` today, so this is a defensive
    restatement of the issue's own exemption list, not new coverage), or
    under any `tests/` directory.

The PR as a whole is exempt, regardless of its diff, when it carries the
`no-changelog` label.

Usage: tools/changelog_check.py [--labels L1,L2,...] [BASE] [HEAD]
       (default: no labels, origin/develop, HEAD)
Exit 0 when no entry is required, or a qualifying entry was found.
Exit 1 when a `quadraui/src/**` behavior change has no `[Unreleased]` entry.
"""
from __future__ import annotations

import fnmatch
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import comment_only_check  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent
CHANGELOG_PATH = "CHANGELOG.md"

NO_CHANGELOG_LABEL = "no-changelog"

# Directory-prefix exemptions (see module docstring: listed for completeness
# per the issue, even though none of these overlap `quadraui/src/**` today).
EXEMPT_PATH_GLOBS = (
    "quadraui/examples/*",
    "quadraui/fuzz/*",
    ".github/*",
    "tools/*",
    "scripts/*",
    "docs/*",
    "*/docs/*",
    "*/tests/*",
    "tests/*",
)

SRC_FILE_RE = re.compile(r"^quadraui/src/.*\.rs$")
CFG_TEST_RE = re.compile(r"#\[cfg\(test\)\]")
HUNK_HEADER_RE = re.compile(
    r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@"
)


def run_git(args: list[str], repo_root: Path) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo_root, capture_output=True, text=True, check=True
    ).stdout


def changed_files(base: str, head: str, repo_root: Path) -> list[str]:
    out = run_git(["diff", "--name-only", f"{base}...{head}"], repo_root)
    return [line for line in out.splitlines() if line]


def show(rev: str, path: str, repo_root: Path) -> str | None:
    """Content of `path` at `rev`, or None if it doesn't exist there."""
    r = subprocess.run(
        ["git", "show", f"{rev}:{path}"],
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    return r.stdout if r.returncode == 0 else None


def is_exempt_path(path: str) -> bool:
    return any(fnmatch.fnmatch(path, pat) for pat in EXEMPT_PATH_GLOBS)


def test_module_boundary(source: str) -> int | None:
    """1-indexed line of the first `#[cfg(test)]` in `source`, or None."""
    for i, line in enumerate(source.splitlines(), start=1):
        if CFG_TEST_RE.search(line):
            return i
    return None


def diff_hunks(
    base: str, head: str, path: str, repo_root: Path
) -> list[tuple[int, int, int, int]]:
    """`(old_start, old_count, new_start, new_count)` per hunk of `path`'s diff."""
    out = run_git(
        ["diff", "--unified=0", f"{base}...{head}", "--", path], repo_root
    )
    hunks = []
    for line in out.splitlines():
        m = HUNK_HEADER_RE.match(line)
        if not m:
            continue
        old_start = int(m.group(1))
        old_count = int(m.group(2)) if m.group(2) is not None else 1
        new_start = int(m.group(3))
        new_count = int(m.group(4)) if m.group(4) is not None else 1
        hunks.append((old_start, old_count, new_start, new_count))
    return hunks


def is_comment_only_change(base: str, head: str, path: str, repo_root: Path) -> bool:
    old = show(base, path, repo_root) or ""
    new = show(head, path, repo_root) or ""
    a = comment_only_check.strip(old)
    b = comment_only_check.strip(new)
    if a[0] != b[0]:
        return False
    if sorted(a[1]) != sorted(b[1]):
        return False
    if b[2] < a[2]:
        return False
    return True


def is_test_module_only_change(base: str, head: str, path: str, repo_root: Path) -> bool:
    """True if every hunk of `path`'s diff lies entirely at/after the first
    `#[cfg(test)]` line, in whichever of base/head it's being measured against.
    A hunk touching a revision with no `#[cfg(test)]` block at all is never
    exempt via this path -- there is nothing to bound it to."""
    old_src = show(base, path, repo_root)
    new_src = show(head, path, repo_root)
    old_boundary = test_module_boundary(old_src) if old_src is not None else None
    new_boundary = test_module_boundary(new_src) if new_src is not None else None

    hunks = diff_hunks(base, head, path, repo_root)
    if not hunks:
        return True  # no textual change (e.g. mode-only); nothing to require an entry for
    for old_start, old_count, new_start, new_count in hunks:
        if old_count > 0:
            if old_boundary is None or old_start < old_boundary:
                return False
        if new_count > 0:
            if new_boundary is None or new_start < new_boundary:
                return False
    return True


def requires_changelog_entry(
    base: str, head: str, repo_root: Path, files: list[str] | None = None
) -> list[str]:
    """Paths among `files` (default: the full base...head diff) that are
    non-test, non-exempt `quadraui/src/**` Rust changes requiring an entry."""
    if files is None:
        files = changed_files(base, head, repo_root)
    required = []
    for path in files:
        if not SRC_FILE_RE.match(path):
            continue
        if is_exempt_path(path):
            continue
        if is_comment_only_change(base, head, path, repo_root):
            continue
        if is_test_module_only_change(base, head, path, repo_root):
            continue
        required.append(path)
    return required


def unreleased_section_bounds(source: str) -> tuple[int, int] | None:
    """`(start, end)` 1-indexed, exclusive-of-heading line range spanned by
    `## [Unreleased]`'s body in `source`: lines strictly after the heading
    up to (not including) the next `## [` heading, or EOF. None if there is
    no `## [Unreleased]` heading at all."""
    lines = source.splitlines()
    start = None
    for i, line in enumerate(lines, start=1):
        if line.strip() == "## [Unreleased]":
            start = i + 1
            break
    if start is None:
        return None
    end = len(lines) + 1
    for i in range(start, len(lines) + 1):
        if lines[i - 1].startswith("## ["):
            end = i
            break
    return start, end


def has_unreleased_entry(base: str, head: str, repo_root: Path) -> bool:
    """True if the diff adds a non-blank line inside HEAD's `[Unreleased]`
    section body."""
    new_src = show(head, CHANGELOG_PATH, repo_root)
    if new_src is None:
        return False
    bounds = unreleased_section_bounds(new_src)
    if bounds is None:
        return False
    start, end = bounds

    hunks = diff_hunks(base, head, CHANGELOG_PATH, repo_root)
    new_lines = new_src.splitlines()
    for _old_start, _old_count, new_start, new_count in hunks:
        if new_count == 0:
            continue
        for lineno in range(new_start, new_start + new_count):
            if start <= lineno < end and new_lines[lineno - 1].strip():
                return True
    return False


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    labels: set[str] = set()
    positional = []
    i = 0
    while i < len(args):
        if args[i] == "--labels":
            i += 1
            if i < len(args):
                labels = {s.strip() for s in args[i].split(",") if s.strip()}
        else:
            positional.append(args[i])
        i += 1

    base = positional[0] if len(positional) > 0 else "origin/develop"
    head = positional[1] if len(positional) > 1 else "HEAD"

    if NO_CHANGELOG_LABEL in labels:
        print(f"exempt: PR carries the '{NO_CHANGELOG_LABEL}' label")
        return 0

    required = requires_changelog_entry(base, head, REPO_ROOT)
    if not required:
        print("ok: no quadraui/src/** behavior change requires a changelog entry")
        return 0

    print("quadraui/src/** changes requiring a CHANGELOG.md entry:")
    for path in required:
        print(f"  {path}")

    if has_unreleased_entry(base, head, REPO_ROOT):
        print("ok: CHANGELOG.md's [Unreleased] section gained an entry")
        return 0

    print(
        "::error::CHANGELOG.md enforcement (#1500): this PR changes "
        "quadraui/src/** without adding a line inside CHANGELOG.md's "
        "[Unreleased] section. Either (1) add an entry under the "
        "appropriate ### heading inside [Unreleased] describing the "
        f"change, or (2) apply the '{NO_CHANGELOG_LABEL}' label if this "
        "change genuinely needs no changelog entry (comment-only and "
        "test-only changes are already exempt automatically)."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
