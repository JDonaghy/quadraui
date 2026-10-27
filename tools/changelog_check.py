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
  - every hunk in its diff falls entirely inside the file's trailing,
    EOF-spanning `#[cfg(test)] mod NAME { ... }` block, in both the base
    and head revisions where that revision has one. The boundary is the
    *last* top-level (column-0) `#[cfg(test)]` immediately followed by a
    top-level `mod NAME {` whose balanced brace block runs all the way to
    EOF -- not the first `#[cfg(test)]` occurrence anywhere in the file.
    A lone `#[cfg(test)] use ...;` import, or a `#[cfg(test)]` test
    submodule nested inside another `mod` (and so closing well before
    EOF), is never a boundary: using "first occurrence" instead silently
    exempted ~99% of `gtk/backend.rs` and `tui/backend.rs` (#1500 review).
    CLAUDE.md's own convention ("Tests in `#[cfg(test)]` mod tests at
    file bottom") is what makes "the trailing, EOF-spanning block" a safe
    proxy for "test code" without a full Rust parse;
  - its path falls under `quadraui/examples/`, `quadraui/fuzz/`, `.github/`,
    `tools/`, `scripts/`, or any `docs/` directory (listed for completeness
    -- none of these live under `quadraui/src/` today, so this is a defensive
    restatement of the issue's own exemption list, not new coverage), or
    under any `tests/` directory.

The PR as a whole is exempt, regardless of its diff, when it carries the
`no-changelog` label, or when the diff has the release-promotion shape:
this diff's own changed files include `quadraui/Cargo.toml`, the manifest
version there is strictly newer at HEAD than at BASE, and HEAD's
`CHANGELOG.md` already matches what `scripts/release.py prepare` leaves
behind for that version -- `[Unreleased]` empty and a `## [X.Y.Z]` section
holding that content. This is keyed on diff shape alone, not on which
branches the PR runs between, so it matches both the `develop -> main`
promotion PR and the `release-X.Y.Z -> develop` bump PR that precedes it.
A promotion PR legitimately moves every entry out of `[Unreleased]`
instead of adding one.

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

REPO_ROOT = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(Path(__file__).resolve().parent))
import comment_only_check  # noqa: E402

# release_decide's version/section parsing is reused here rather than
# reimplemented, which couples this module's imports to scripts/
# release_decide.py's location.
sys.path.insert(0, str(REPO_ROOT / "scripts"))
import release_decide  # noqa: E402

CHANGELOG_PATH = "CHANGELOG.md"
MANIFEST_PATH = "quadraui/Cargo.toml"

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
# Top-level (column-0, no leading whitespace) `#[cfg(test)]` immediately
# followed by a top-level `mod NAME {` -- the shape CLAUDE.md's "tests at
# file bottom" convention guarantees for a *trailing* test module. Anything
# indented (e.g. a test submodule nested inside another `mod`) or not a
# `mod` declaration (e.g. a lone `#[cfg(test)] use ...;`) never matches.
TOP_LEVEL_CFG_TEST_RE = re.compile(r"^#\[cfg\(test\)\]\s*$")
TOP_LEVEL_MOD_OPEN_RE = re.compile(r"^mod\s+\w+\s*\{\s*$")
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
    """1-indexed line of the start of `source`'s trailing, EOF-spanning
    `#[cfg(test)] mod NAME { ... }` block, or None if there isn't one.

    A candidate is a top-level (column-0) `#[cfg(test)]` line immediately
    followed (ignoring blank lines) by a top-level `mod NAME {` line. It
    only qualifies as *the* boundary if that `mod` block's matching
    closing brace -- found by a simple running count of `{`/`}` across the
    following lines, which is sufficient for this repo's convention of one
    plain trailing test module and doesn't need a full Rust parse -- is
    the file's last non-blank line, i.e. every byte from the attribute
    line onward belongs to that single balanced block. This deliberately
    rejects a lone `#[cfg(test)] use ...;` import and a `#[cfg(test)]`
    test submodule nested inside another `mod` (indented, and/or closing
    well before EOF): treating either as the boundary is what let a
    "first #[cfg(test)] occurrence" heuristic silently exempt ~99% of
    `gtk/backend.rs` and `tui/backend.rs` from the changelog gate
    (#1500 review).
    """
    lines = source.splitlines()
    n = len(lines)
    last_nonblank = n - 1
    while last_nonblank >= 0 and lines[last_nonblank].strip() == "":
        last_nonblank -= 1
    if last_nonblank < 0:
        return None

    for i in range(n):
        if not TOP_LEVEL_CFG_TEST_RE.match(lines[i]):
            continue
        j = i + 1
        while j < n and lines[j].strip() == "":
            j += 1
        if j >= n or not TOP_LEVEL_MOD_OPEN_RE.match(lines[j]):
            continue

        depth = lines[j].count("{") - lines[j].count("}")
        closed_at = j if depth <= 0 else None
        k = j
        while closed_at is None and k + 1 < n:
            k += 1
            depth += lines[k].count("{") - lines[k].count("}")
            if depth <= 0:
                closed_at = k
        if closed_at == last_nonblank:
            return i + 1
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


def _version_tuple(version: str) -> tuple[int, ...] | None:
    """Parse the dotted-numeric prefix of a semver-ish string into a
    comparable tuple, e.g. `"0.6.1"` -> `(0, 6, 1)`, `"0.2.0-rc1"` ->
    `(0, 2, 0)`. Returns None if any dot-separated component before a
    `-`/`+` suffix isn't purely digits, so an unparsable version can't be
    silently treated as comparable."""
    core = version.split("-", 1)[0].split("+", 1)[0]
    parts = core.split(".")
    if not parts or not all(p.isdigit() for p in parts):
        return None
    return tuple(int(p) for p in parts)


def _is_version_bump(new_version: str, old_version: str | None) -> bool:
    """True if `new_version` is strictly newer than `old_version`.

    Falls back to a plain inequality when either side doesn't parse as a
    dotted-numeric version, so an unparsable manifest version doesn't
    silently disable the gate. When both sides do parse, this also rules
    out a reverted/downgraded version matching the promotion shape.
    """
    if old_version is None:
        return True
    new_tuple = _version_tuple(new_version)
    old_tuple = _version_tuple(old_version)
    if new_tuple is None or old_tuple is None:
        return new_version != old_version
    return new_tuple > old_tuple


def release_promotion_version(base: str, head: str, repo_root: Path) -> str | None:
    """`quadraui/Cargo.toml`'s version at `head`, if this diff has the
    release-promotion shape `scripts/release.py prepare` leaves a develop
    -> main PR (or a `release-X.Y.Z` -> develop bump PR) in: `base...head`
    itself changes `quadraui/Cargo.toml`, the manifest version there is
    strictly newer at `head` than at `base`, HEAD's `[Unreleased]` body is
    empty (every line blank or the section absent entirely), and a
    `## [<version>]` section matching the new version already holds
    non-blank content.

    Requiring the manifest bump to be part of *this* diff -- not just
    "the versions differ between base and head" -- rules out a stale
    feature branch forked after a bump already landed on `develop`: that
    branch's own diff never touches the manifest, so a later bump on
    `develop` moving `base`'s version ahead must not make this function
    match. The strictly-newer comparison (not just "changed") also rules
    out a downgrade, e.g. a reverted bump.

    Returns None for any other shape, including a promotion PR that also
    picked up an unrelated `[Unreleased]` addition after `prepare` ran --
    that case still needs the ordinary entry check.
    """
    if MANIFEST_PATH not in changed_files(base, head, repo_root):
        return None

    head_manifest_src = show(head, MANIFEST_PATH, repo_root)
    changelog_src = show(head, CHANGELOG_PATH, repo_root)
    if head_manifest_src is None or changelog_src is None:
        return None

    version = release_decide.manifest_version(head_manifest_src)
    if not version:
        return None

    base_manifest_src = show(base, MANIFEST_PATH, repo_root)
    base_version = (
        release_decide.manifest_version(base_manifest_src)
        if base_manifest_src is not None
        else None
    )
    if not _is_version_bump(version, base_version):
        return None

    bounds = unreleased_section_bounds(changelog_src)
    if bounds is not None:
        start, end = bounds
        unreleased_lines = changelog_src.splitlines()[start - 1 : end - 1]
        if any(line.strip() for line in unreleased_lines):
            return None

    if not release_decide.changelog_section(changelog_src, version).strip():
        return None
    return version


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

    promotion_version = release_promotion_version(base, head, REPO_ROOT)
    if promotion_version is not None:
        print(
            f"exempt: CHANGELOG.md's [Unreleased] section is empty and its "
            f"[{promotion_version}] section already holds that content -- "
            "release-promotion PR shape"
        )
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
