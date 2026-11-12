#!/usr/bin/env python3
"""One-command release prep + promotion helper for `quadraui` (#1501).

A release today is several hand steps: bump `quadraui/Cargo.toml`'s
version, refresh `Cargo.lock`, rewrite the hard-coded version strings in
`README.md` and `quadraui/src/lib.rs` that `quadraui/tests/readme_truth.rs`
cross-checks against `CARGO_PKG_VERSION`, assemble every `changelog.d/`
fragment (#2419) into `CHANGELOG.md`'s `[Unreleased]` section, retitle it
with a date and add a fresh empty one, delete the fragments, commit, open
a bump PR against `develop` — then, separately, hand-open a `develop` ->
`main` promotion PR that must land as a **merge commit** (never
squash/rebase, or `main` and `develop` diverge). This script automates the
mechanical parts of both steps and prints (but never runs) the `git`/`gh`
commands a human still has to execute, since `gh` may not be installed or
authenticated wherever this runs.

`prepare` is the only supported way to bump `quadraui/Cargo.toml`'s
version. A hand-edit of the version field that skips `prepare` leaves
`Cargo.lock` stale; `ci.yml`'s `lockfile-check` job and `release.yml`'s
`check` job both run `scripts/release_decide.py check-lock`, which names
that mismatch before `cargo publish --locked` fails on it during
`publish`.

Two subcommands:

    python3 scripts/release.py prepare 0.1.5
    python3 scripts/release.py promote

`prepare X.Y.Z`, run from a clean checkout of `origin/develop`:

  1. refuses if the working tree is dirty (`git status --porcelain`);
  2. refuses if `X.Y.Z` is not strictly greater than the version currently
     in `quadraui/Cargo.toml`;
  3. refuses if `CHANGELOG.md`'s `## [Unreleased]` section is empty AND
     `changelog.d/` has no fragments (nothing to release);
  4. creates and checks out branch `release-X.Y.Z`;
  5. sets `quadraui/Cargo.toml`'s version to `X.Y.Z`;
  6. refreshes `Cargo.lock` via `cargo update -p quadraui --offline` and
     verifies the resulting diff touches only quadraui's own `version =`
     line in the lockfile;
  7. rewrites README.md's Status section (`` `X.Y.x` ``) and
     getting-started dependency snippet (`quadraui = { version = "X.Y"`),
     and `quadraui/src/lib.rs`'s crate-doc `## Status` section
     (`` `X.Y.x` ``), to match the new version — the exact strings
     `tests/readme_truth.rs` cross-checks against `CARGO_PKG_VERSION`;
  8. assembles every `changelog.d/*.md` fragment into `[Unreleased]`'s own
     body (grouped by `### Kind` heading in Keep-a-Changelog order, sorted
     by issue number within each heading, appended below whatever bullets
     that heading already had), then retitles `## [Unreleased]` to
     `## [X.Y.Z] - YYYY-MM-DD` and inserts a fresh empty `## [Unreleased]`
     above it;
  9. deletes every fragment file `changelog.d/*.md` (except `README.md`)
     it just assembled;
  10. commits the five version-string files plus the deleted fragments;
  11. prints the `git push` and `gh pr create --base develop` commands for
      a human to run.

`--with-py` additionally releases the `quadraui` PyPI wheel (`quadraui-py`)
in lockstep, in the same branch/commit: it bumps
`quadraui-py/pyproject.toml`'s `[project] version` and
`quadraui-py/Cargo.toml`'s `[package] version` to the same `X.Y.Z`, and the
lock refresh in step 6 also passes `-p quadraui-py`. It refuses if `X.Y.Z`
is not greater than the current wheel version (PyPI is append-only — a wheel
version can never go down). Without the flag the wheel is untouched: the two
artifacts version independently by design (the binding surface is a
narrower, slower-moving subset of the crate), so most crate releases do not
ship a wheel. The wheel's own release trigger is a `py-v<version>` tag, which
`promote` prints (see below) — not this bump PR.

All refusals exit 1 before touching the working tree (step 4 is the first
mutation).

`promote`, run from a checkout with `develop` fully merged into it: reads
`CHANGELOG.md`'s most recent released (non-`Unreleased`) section and prints
the `gh pr create --base main --head develop` and `gh pr merge --merge`
commands, with the PR body pre-filled by `promotion_pr_body` — a short
header plus that section, truncated well under GitHub's PR body length
limit so the printed command never fails with "Body is too long" on a
release with a large CHANGELOG section. Never runs them. `--merge` is
load-bearing in that last command: a squash or rebase merge of
`develop` into `main` leaves the two branches' histories diverged, breaking
the next promotion's merge-base. When `quadraui-py`'s current version has no
`py-v<version>` tag yet, `promote` also prints the `git tag py-v<version>` +
`git push` step to run after the promotion merges — the trigger that
publishes the wheel to PyPI — so one `--with-py` release flows crate and
wheel through together.
"""
from __future__ import annotations

import argparse
import datetime
import re
import subprocess
import sys
from pathlib import Path

VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
UNRELEASED_HEADER_RE = re.compile(r"(?m)^## \[Unreleased\]\s*$")
RELEASE_HEADER_RE = re.compile(r"(?m)^## \[(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\][^\n]*$")
LOCK_VERSION_LINE_RE = re.compile(r'^[+-]version = "\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"$')

# The pre-1.0 status marker README.md's Status section and
# quadraui/src/lib.rs's crate-doc `## Status` section both carry, e.g.
# `` `0.3.x` ``. Matches `tests/readme_truth.rs`'s `major_minor_x()`.
PRE_1_0_STATUS_RE = re.compile(r"`\d+\.\d+\.x`")

# The resolvable dependency snippet README.md's Getting started section
# leads with, e.g. `quadraui = { version = "0.3"`. Matches
# `tests/readme_truth.rs`'s `major_minor()`.
DEPENDENCY_SNIPPET_RE = re.compile(r'(quadraui = \{ version = )"\d+\.\d+"')

# -- changelog.d/ fragments (#2419) ------------------------------------
#
# A fragment is one file, `changelog.d/<issue>.<kind>.md`, whose content is
# the bullet text that belongs under `[Unreleased]`'s matching `### Kind`
# heading -- see `changelog.d/README.md` for the authoring convention.
# `tools/changelog_check.py` enforces this shape at PR time; `prepare`
# assembles every fragment into `[Unreleased]` and deletes them in the same
# version-bump commit, so `develop` never carries a stale fragment past a
# release.
FRAGMENT_DIR = "changelog.d"
FRAGMENT_README_NAME = "README.md"
FRAGMENT_KINDS = ("added", "changed", "deprecated", "removed", "fixed")
# Keep-a-Changelog's own section order (minus `Security`, which no fragment
# kind maps to); also the order `assemble_unreleased_body` emits headings in.
CANONICAL_HEADING_ORDER = ["Added", "Changed", "Deprecated", "Removed", "Fixed"]
FRAGMENT_KIND_HEADINGS: dict[str, str] = dict(
    zip(FRAGMENT_KINDS, CANONICAL_HEADING_ORDER)
)
HEADING_TO_FRAGMENT_KIND: dict[str, str] = {
    heading: kind for kind, heading in FRAGMENT_KIND_HEADINGS.items()
}
FRAGMENT_NAME_RE = re.compile(
    r"^(?P<issue>\d+)\.(?P<kind>" + "|".join(FRAGMENT_KINDS) + r")\.md$"
)


def parse_fragment_filename(name: str) -> tuple[int, str] | None:
    """`(issue, kind)` for a `changelog.d/<issue>.<kind>.md` filename, or
    None if `name` doesn't match that shape (including `README.md`, which
    is never a fragment)."""
    match = FRAGMENT_NAME_RE.match(name)
    if not match:
        return None
    return int(match.group("issue")), match.group("kind")


def discover_fragment_paths(repo_dir: Path) -> list[Path]:
    """Every `changelog.d/*.md` fragment file under `repo_dir`, sorted by
    filename, excluding `changelog.d/README.md`. Returns `[]` if
    `changelog.d/` doesn't exist."""
    fragment_dir = repo_dir / FRAGMENT_DIR
    if not fragment_dir.is_dir():
        return []
    return sorted(
        p for p in fragment_dir.glob("*.md") if p.name != FRAGMENT_README_NAME
    )


def read_fragments(repo_dir: Path) -> list[tuple[int, str, str, Path]]:
    """`(issue, kind, content, path)` for every fragment `discover_fragment_paths`
    finds under `repo_dir`. Raises `ReleaseError` naming the offending file if
    its name doesn't match `<issue>.<kind>.md` or its content is empty --
    the same two checks `tools/changelog_check.py` enforces per-PR, re-run
    here defensively since `prepare` is the step that actually deletes
    these files."""
    fragments = []
    for path in discover_fragment_paths(repo_dir):
        parsed = parse_fragment_filename(path.name)
        if parsed is None:
            raise ReleaseError(
                f"{path.relative_to(repo_dir)}: fragment filename must be "
                f"`<issue>.<kind>.md` with kind one of "
                f"{', '.join(FRAGMENT_KINDS)} (got {path.name!r})"
            )
        issue, kind = parsed
        content = path.read_text(encoding="utf-8")
        if not content.strip():
            raise ReleaseError(f"{path.relative_to(repo_dir)}: fragment is empty")
        fragments.append((issue, kind, content, path))
    return fragments


def parse_unreleased_sections(body: str) -> tuple[str, dict[str, str]]:
    """Split a `[Unreleased]` (or any version section's) raw body text into
    `(preamble, sections)`: any text before the first `### ` heading
    (normally empty) and an *ordered* dict of heading name -> that
    heading's own body text (not including the `### ` line itself), in
    first-appearance order. Two occurrences of the same heading name (the
    duplicated-section shape a hand-resolved merge conflict has produced
    before, e.g. #2409/#2416) merge into that heading's one entry rather
    than staying split."""
    lines = body.splitlines()
    preamble_lines: list[str] = []
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in lines:
        if line.startswith("### "):
            current = line[len("### "):].strip()
            sections.setdefault(current, [])
            continue
        if current is None:
            preamble_lines.append(line)
        else:
            sections[current].append(line)
    preamble = "\n".join(preamble_lines).strip("\n")
    return preamble, {
        name: "\n".join(section_lines).strip("\n")
        for name, section_lines in sections.items()
    }


def render_unreleased_body(sections_in_order: list[tuple[str, str]]) -> str:
    """Render `(heading, body_text)` pairs (`body_text` already trimmed of
    its own leading/trailing blank lines) back into the exact raw shape
    `CHANGELOG.md` uses for a version section's body: a leading newline,
    each `### Heading\\n<body>` block separated by one blank line, and a
    trailing blank line before the next `## [` header. `[]` renders as the
    same bare `"\\n\\n"` a fresh, still-empty `[Unreleased]` section has."""
    if not sections_in_order:
        return "\n\n"
    blocks = [f"### {heading}\n{body}" if body else f"### {heading}" for heading, body in sections_in_order]
    return "\n" + "\n\n".join(blocks) + "\n\n"


def assemble_unreleased_body(
    old_body: str, fragments: list[tuple[int, str, str]]
) -> str:
    """Merge `fragments` (`(issue, kind, content)`) into `old_body` (the
    raw text currently sitting between `[Unreleased]`'s header and the next
    `## [` header): each fragment's content is grouped under its matching
    `### Kind` heading -- created, in `CANONICAL_HEADING_ORDER` position,
    if `old_body` didn't already have one -- sorted by issue number within
    that heading, and appended below whatever bullets the heading already
    had. A heading already present in `old_body` keeps its existing
    position when no canonical slot claims it first. Returns the same raw
    shape `old_body` itself was in (see `render_unreleased_body`), so
    passing `fragments=[]` reproduces `old_body` byte-for-byte modulo any
    duplicate-heading merge `parse_unreleased_sections` already performs."""
    preamble, sections = parse_unreleased_sections(old_body)

    by_kind: dict[str, list[tuple[int, str]]] = {}
    for issue, kind, content in fragments:
        by_kind.setdefault(kind, []).append((issue, content.strip()))

    heading_order = list(sections.keys())
    for kind in FRAGMENT_KINDS:
        heading = FRAGMENT_KIND_HEADINGS[kind]
        if by_kind.get(kind) and heading not in heading_order:
            heading_order.append(heading)

    ordered = [h for h in CANONICAL_HEADING_ORDER if h in heading_order]
    ordered += [h for h in heading_order if h not in ordered]

    rendered_sections: list[tuple[str, str]] = []
    for heading in ordered:
        existing = sections.get(heading, "")
        kind = HEADING_TO_FRAGMENT_KIND.get(heading)
        new_bullets = (
            sorted(by_kind.get(kind, []), key=lambda pair: pair[0]) if kind else []
        )
        block_parts = ([existing] if existing else []) + [
            content for _issue, content in new_bullets
        ]
        rendered_sections.append((heading, "\n".join(block_parts)))

    body = render_unreleased_body(rendered_sections)
    if preamble:
        body = f"\n{preamble}\n{body[1:]}"
    return body


class ReleaseError(RuntimeError):
    """A refusal condition, or an unexpected failure, in `prepare`/`promote`."""


def run_git(args: list[str], cwd: Path, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=check
    )


def git_is_clean(repo_dir: Path) -> bool:
    result = run_git(["status", "--porcelain"], repo_dir)
    return result.stdout.strip() == ""


def parse_version(text: str) -> tuple[int, int, int]:
    match = VERSION_RE.match(text.strip())
    if not match:
        raise ReleaseError(f"not a valid X.Y.Z version: {text!r}")
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)))


def read_version_line(text: str, *, label: str) -> str:
    """Return the first `version = "..."` value in `text`.

    Works for both a `[package]` Cargo.toml and a `[project]` pyproject.toml:
    in each the package/project version is the first bare `version =` line
    (comment lines start with `#`, so they never match)."""
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', text)
    if not match:
        raise ReleaseError(f'could not find a `version = "..."` line in {label}')
    return match.group(1)


def set_version_line(text: str, new_version: str, *, label: str) -> str:
    """Rewrite the first `version = "..."` line in `text` to `new_version`."""
    pattern = re.compile(r'(?m)^(version\s*=\s*)"[^"]+"')
    new_text, count = pattern.subn(rf'\1"{new_version}"', text, count=1)
    if count != 1:
        raise ReleaseError(f'could not find a `version = "..."` line in {label}')
    return new_text


def read_package_version(cargo_toml_text: str) -> str:
    return read_version_line(cargo_toml_text, label="quadraui/Cargo.toml")


def set_package_version(cargo_toml_text: str, new_version: str) -> str:
    return set_version_line(cargo_toml_text, new_version, label="quadraui/Cargo.toml")


def pre_1_0_status_string(version: str) -> str:
    """The `` `X.Y.x` `` pre-1.0 status marker for `version`, e.g. "0.3.1"
    -> "0.3.x". Mirrors `tests/readme_truth.rs`'s `major_minor_x()`."""
    major, minor, _ = parse_version(version)
    return f"{major}.{minor}.x"


def published_dependency_version(version: str) -> str:
    """The crates.io dependency requirement for `version`, e.g. "0.3.1" ->
    "0.3". Mirrors `tests/readme_truth.rs`'s `major_minor()`."""
    major, minor, _ = parse_version(version)
    return f"{major}.{minor}"


def set_pre_1_0_status_string(text: str, version: str, *, label: str) -> str:
    """Rewrite every `` `X.Y.x` `` pre-1.0 status marker in `text` to match
    `version`. Used on README.md's Status section and lib.rs's crate-doc
    `## Status` section."""
    new_text, count = PRE_1_0_STATUS_RE.subn(
        f"`{pre_1_0_status_string(version)}`", text
    )
    if count == 0:
        raise ReleaseError(
            f"{label} has no `` `X.Y.x` `` pre-1.0 status marker to update"
        )
    return new_text


def set_dependency_snippet_version(text: str, version: str, *, label: str) -> str:
    """Rewrite README.md's `quadraui = { version = "X.Y"` getting-started
    snippet to match `version`."""
    new_text, count = DEPENDENCY_SNIPPET_RE.subn(
        rf'\1"{published_dependency_version(version)}"', text
    )
    if count == 0:
        raise ReleaseError(
            f'{label} has no `quadraui = {{ version = "..." }}` dependency '
            "snippet to update"
        )
    return new_text


def unreleased_body(changelog_text: str) -> str:
    """Return the body text of CHANGELOG.md's `## [Unreleased]` section."""
    match = UNRELEASED_HEADER_RE.search(changelog_text)
    if not match:
        raise ReleaseError("CHANGELOG.md has no '## [Unreleased]' header")
    rest = changelog_text[match.end():]
    next_header = re.search(r"(?m)^## \[", rest)
    return rest[: next_header.start()] if next_header else rest


def retitle_changelog(
    changelog_text: str,
    new_version: str,
    date: str,
    *,
    fragments: list[tuple[int, str, str]] | None = None,
) -> str:
    """Retitle `## [Unreleased]` to `## [X.Y.Z] - date`, adding a fresh
    empty one above it.

    `fragments` not given (or empty) leaves the retitled section's own body
    byte-for-byte as it was -- this function's pre-fragment behavior. When
    given (every `changelog.d/*.md` fragment discovered by
    `read_fragments`, as `(issue, kind, content)`), the retitled section's
    body is re-assembled via `assemble_unreleased_body` first: each
    fragment is grouped under its matching `### Kind` heading, in
    Keep-a-Changelog order and sorted by issue number, appended below
    whatever bullets that heading already had.
    """
    matches = list(UNRELEASED_HEADER_RE.finditer(changelog_text))
    if len(matches) != 1:
        raise ReleaseError(
            f"expected exactly one '## [Unreleased]' header in CHANGELOG.md, found {len(matches)}"
        )
    match = matches[0]

    if not fragments:
        replacement = f"## [Unreleased]\n\n## [{new_version}] - {date}"
        return changelog_text[: match.start()] + replacement + changelog_text[match.end():]

    rest = changelog_text[match.end():]
    next_header = re.search(r"(?m)^## \[", rest)
    old_body = rest[: next_header.start()] if next_header else rest
    tail = rest[next_header.start():] if next_header else ""

    assembled_body = assemble_unreleased_body(old_body, fragments)
    replacement = f"## [Unreleased]\n\n## [{new_version}] - {date}{assembled_body}"
    return changelog_text[: match.start()] + replacement + tail


def latest_release_version(changelog_text: str) -> str:
    """Return the version of the first released (non-`Unreleased`) section."""
    for line in changelog_text.splitlines():
        match = RELEASE_HEADER_RE.match(line)
        if match:
            return match.group(1)
    raise ReleaseError("CHANGELOG.md has no released ('## [X.Y.Z]') section")


def release_section_body(changelog_text: str, version: str) -> str:
    header = re.compile(rf"(?m)^## \[{re.escape(version)}\][^\n]*$")
    match = header.search(changelog_text)
    if not match:
        raise ReleaseError(f"CHANGELOG.md has no '## [{version}]' header")
    rest = changelog_text[match.end():]
    next_header = re.search(r"(?m)^## \[", rest)
    return (rest[: next_header.start()] if next_header else rest).strip()


# GitHub's hard cap on a PR/issue body (`gh pr create --body` fails with
# "Body is too long" past this). A large CHANGELOG release section can
# exceed it on its own, so this constant leaves real margin under the
# actual limit for the summary header/truncation marker this function adds
# on top of the section body, and for GitHub trimming whitespace at the
# edges.
PR_BODY_MAX_LEN = 60_000

PROMOTION_BODY_TRUNCATION_MARKER = (
    "\n\n…(truncated — see CHANGELOG.md's full `## [{version}]` section for "
    "the rest)\n"
)


def promotion_pr_body(
    changelog_text: str, version: str, *, max_len: int = PR_BODY_MAX_LEN
) -> str:
    """The `develop` -> `main` promotion PR body for `version`: a short
    header plus `release_section_body`'s content, truncated well under
    GitHub's PR body length limit instead of risking `gh pr create` failing
    with "Body is too long" on a release with a large CHANGELOG section.

    `max_len` must leave room for the header and truncation marker on their
    own -- raises `ReleaseError` rather than silently returning a body
    longer than `max_len` when it doesn't.
    """
    section = release_section_body(changelog_text, version)
    header = (
        f"Promotes `develop` to `main` for v{version}. Full release notes "
        f"are CHANGELOG.md's `## [{version}]` section.\n\n"
    )
    marker = PROMOTION_BODY_TRUNCATION_MARKER.format(version=version)
    if len(header) + len(section) <= max_len:
        return header + section
    budget = max_len - len(header) - len(marker)
    if budget < 0:
        raise ReleaseError(
            f"max_len={max_len} leaves no room for the promotion PR body's "
            f"own header + truncation marker ({len(header) + len(marker)} "
            "chars combined)"
        )
    return header + section[:budget].rstrip() + marker


def refresh_lock(repo_dir: Path, packages: tuple[str, ...] = ("quadraui",)) -> None:
    """Run `cargo update -p <pkg>… --offline` to sync Cargo.lock's version lines.

    Defaults to just `quadraui`; a `--with-py` release also passes
    `quadraui-py` so both path crates' lock lines move in one invocation."""
    package_args = [arg for pkg in packages for arg in ("-p", pkg)]
    cmd = ["cargo", "update", *package_args, "--offline"]
    result = subprocess.run(cmd, cwd=repo_dir, capture_output=True, text=True)
    if result.returncode != 0:
        raise ReleaseError(f"`{' '.join(cmd)}` failed:\n" + result.stderr)


def verify_lock_diff_is_version_lines_only(repo_dir: Path) -> None:
    """Refuse if `cargo update` touched anything in Cargo.lock besides a `version =` line.

    Deliberately paranoid: `cargo update -p quadraui[ -p quadraui-py]` should
    only ever bump those path crates' own `version =` lines (nothing in the
    lock depends on their registry resolution), but if a future cargo version
    ever resolves something else too, this must fail loudly rather than let an
    unrelated lockfile change slip into a release commit.
    """
    diff = run_git(["diff", "--unified=0", "--", "Cargo.lock"], repo_dir).stdout
    changed_lines = [
        line
        for line in diff.splitlines()
        if (line.startswith("+") or line.startswith("-"))
        and not line.startswith("+++")
        and not line.startswith("---")
    ]
    if not changed_lines:
        raise ReleaseError("`cargo update --offline` made no changes to Cargo.lock")
    bad_lines = [line for line in changed_lines if not LOCK_VERSION_LINE_RE.match(line)]
    if bad_lines:
        raise ReleaseError(
            "`cargo update --offline` changed more than a `version =` line in "
            "Cargo.lock:\n" + "\n".join(bad_lines)
        )


def cmd_prepare(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    cargo_toml_path = repo_dir / "quadraui" / "Cargo.toml"
    changelog_path = repo_dir / "CHANGELOG.md"
    readme_path = repo_dir / "README.md"
    lib_rs_path = repo_dir / "quadraui" / "src" / "lib.rs"
    py_pyproject_path = repo_dir / "quadraui-py" / "pyproject.toml"
    py_cargo_toml_path = repo_dir / "quadraui-py" / "Cargo.toml"

    try:
        if not git_is_clean(repo_dir):
            raise ReleaseError(
                "working tree is dirty; commit or stash before running `prepare`"
            )

        target = parse_version(args.version)
        cargo_toml_text = cargo_toml_path.read_text(encoding="utf-8")
        current_version_str = read_package_version(cargo_toml_text)
        current = parse_version(current_version_str)
        if target <= current:
            raise ReleaseError(
                f"{args.version} is not greater than the current version "
                f"({current_version_str})"
            )

        if args.with_py:
            if not py_pyproject_path.exists():
                raise ReleaseError(
                    "--with-py given but quadraui-py/pyproject.toml does not exist"
                )
            py_current_str = read_version_line(
                py_pyproject_path.read_text(encoding="utf-8"),
                label="quadraui-py/pyproject.toml",
            )
            # PyPI is append-only: a wheel version can never go down.
            if target <= parse_version(py_current_str):
                raise ReleaseError(
                    f"--with-py: {args.version} is not greater than the current "
                    f"quadraui-py version ({py_current_str})"
                )

        changelog_text = changelog_path.read_text(encoding="utf-8")
        fragments = read_fragments(repo_dir)
        if not unreleased_body(changelog_text).strip() and not fragments:
            raise ReleaseError(
                "CHANGELOG.md's '## [Unreleased]' section is empty and "
                "changelog.d/ has no fragments; nothing to release"
            )

        branch = f"release-{args.version}"
        branch_exists = (
            run_git(["rev-parse", "--verify", "--quiet", f"refs/heads/{branch}"],
                    repo_dir, check=False).returncode
            == 0
        )
        if branch_exists:
            # A prior `prepare` run (or a resumed `release_full.py`) already
            # created this branch. Reuse it instead of letting `checkout -b`
            # abort with "a branch named ... already exists" — but only when
            # its tip already carries the version bump; otherwise this is a
            # genuine conflict a human needs to resolve.
            run_git(["checkout", branch], repo_dir)
            branch_version = read_package_version(
                cargo_toml_path.read_text(encoding="utf-8")
            )
            if branch_version == args.version:
                print(
                    f"Branch {branch} already exists and is already bumped to "
                    f"{args.version} — reusing it."
                )
                print("\nRun these commands to open the bump PR:\n")
                print(f"  git push -u origin release-{args.version}")
                print(
                    f'  gh pr create --base develop --head release-{args.version} '
                    f'--title "Release v{args.version} (version bump + CHANGELOG)" '
                    f'--body "Bumps quadraui to {args.version} and retitles CHANGELOG.md\'s '
                    f'[Unreleased] section."'
                )
                return 0
            raise ReleaseError(
                f"branch {branch!r} already exists but its quadraui/Cargo.toml "
                f"version is {branch_version!r}, not {args.version!r} — resolve manually"
            )
        run_git(["checkout", "-b", branch], repo_dir)

        cargo_toml_path.write_text(
            set_package_version(cargo_toml_text, args.version), encoding="utf-8"
        )

        lock_packages = ("quadraui",)
        if args.with_py:
            py_pyproject_path.write_text(
                set_version_line(
                    py_pyproject_path.read_text(encoding="utf-8"),
                    args.version,
                    label="quadraui-py/pyproject.toml",
                ),
                encoding="utf-8",
            )
            py_cargo_toml_path.write_text(
                set_version_line(
                    py_cargo_toml_path.read_text(encoding="utf-8"),
                    args.version,
                    label="quadraui-py/Cargo.toml",
                ),
                encoding="utf-8",
            )
            lock_packages = ("quadraui", "quadraui-py")

        refresh_lock(repo_dir, lock_packages)
        verify_lock_diff_is_version_lines_only(repo_dir)

        readme_text = readme_path.read_text(encoding="utf-8")
        readme_text = set_pre_1_0_status_string(
            readme_text, args.version, label="README.md's Status section"
        )
        readme_text = set_dependency_snippet_version(
            readme_text, args.version, label="README.md's Getting started section"
        )
        readme_path.write_text(readme_text, encoding="utf-8")

        lib_rs_text = lib_rs_path.read_text(encoding="utf-8")
        lib_rs_text = set_pre_1_0_status_string(
            lib_rs_text, args.version, label="quadraui/src/lib.rs's crate doc"
        )
        lib_rs_path.write_text(lib_rs_text, encoding="utf-8")

        date = args.date or datetime.date.today().isoformat()
        fragment_triples = [
            (issue, kind, content) for issue, kind, content, _path in fragments
        ]
        changelog_path.write_text(
            retitle_changelog(
                changelog_text, args.version, date, fragments=fragment_triples
            ),
            encoding="utf-8",
        )

        fragment_rel_paths = []
        for _issue, _kind, _content, path in fragments:
            rel_path = str(path.relative_to(repo_dir))
            path.unlink()
            fragment_rel_paths.append(rel_path)

        paths_to_add = [
            "quadraui/Cargo.toml",
            "Cargo.lock",
            "README.md",
            "quadraui/src/lib.rs",
            "CHANGELOG.md",
            *fragment_rel_paths,
        ]
        if args.with_py:
            paths_to_add += ["quadraui-py/pyproject.toml", "quadraui-py/Cargo.toml"]
        run_git(["add", *paths_to_add], repo_dir)

        commit_subject = (
            f"Release v{args.version} (crate + quadraui-py version bump + CHANGELOG)"
            if args.with_py
            else f"Release v{args.version} (version bump + CHANGELOG)"
        )
        run_git(["commit", "-m", commit_subject], repo_dir)
    except ReleaseError as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as exc:
        print(f"::error::`{' '.join(exc.cmd)}` failed:\n{exc.stderr}", file=sys.stderr)
        return 1

    if args.with_py:
        print(
            f"Prepared release-{args.version} with the crate + quadraui-py version "
            "bump + README/lib.rs version strings + CHANGELOG commit."
        )
    else:
        print(
            f"Prepared release-{args.version} with the version bump + README/lib.rs "
            "version strings + CHANGELOG commit."
        )
    print("\nRun these commands to open the bump PR:\n")
    print(f"  git push -u origin release-{args.version}")
    print(
        f'  gh pr create --base develop --head release-{args.version} '
        f'--title "Release v{args.version} (version bump + CHANGELOG)" '
        f'--body "Bumps quadraui to {args.version} and retitles CHANGELOG.md\'s '
        f'[Unreleased] section."'
    )
    if args.with_py:
        print(
            f"\nThis release also bumps quadraui-py to {args.version}. After the "
            f"develop -> main promotion merges, `promote` will print the "
            f"`py-v{args.version}` tag step that publishes the wheel to PyPI."
        )
    return 0


def cmd_promote(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    changelog_path = repo_dir / "CHANGELOG.md"

    try:
        changelog_text = changelog_path.read_text(encoding="utf-8")
        version = latest_release_version(changelog_text)
        body = promotion_pr_body(changelog_text, version)
    except ReleaseError as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 1

    print(f"Promoting develop -> main for v{version}.")
    print(
        "\nRun these commands (the second only after the first's PR is reviewed "
        "and CI is green):\n"
    )
    print(
        f'  gh pr create --base main --head develop --title "Release v{version}" '
        f"--body \"$(cat <<'EOF'\n{body}\nEOF\n)\""
    )
    print("  gh pr merge --merge <PR number or URL>")
    print(
        "\n`--merge` is required: a squash or rebase merge here diverges "
        "`main` and `develop`'s histories."
    )

    # Opt-in PyPI lockstep: if quadraui-py's current version has no `py-v<version>`
    # tag yet, this promotion also carries an unreleased wheel — print the tag
    # step that `quadraui-py-release.yml` fires on.
    py_pyproject_path = repo_dir / "quadraui-py" / "pyproject.toml"
    if py_pyproject_path.exists():
        try:
            py_version = read_version_line(
                py_pyproject_path.read_text(encoding="utf-8"),
                label="quadraui-py/pyproject.toml",
            )
        except ReleaseError as exc:
            print(f"::error::{exc}", file=sys.stderr)
            return 1
        tag = f"py-v{py_version}"
        existing_tag = run_git(["tag", "-l", tag], repo_dir).stdout.strip()
        if not existing_tag:
            print(
                f"\nquadraui-py is at {py_version} with no {tag} tag yet, so this "
                "promotion also carries an unpublished wheel. After the promotion "
                "merges to `main`, publish it to PyPI:\n"
            )
            print("  git fetch origin main")
            print(f"  git tag {tag} origin/main")
            print(f"  git push origin {tag}")
            print(
                f"\nThe {tag} push triggers `quadraui-py-release.yml`, which asserts "
                "the tag matches quadraui-py/pyproject.toml before building."
            )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    prepare = sub.add_parser(
        "prepare", help="bump version + retitle CHANGELOG.md, on a new release branch"
    )
    prepare.add_argument("version", help="the new version, e.g. 0.1.5")
    prepare.add_argument("--repo-dir", default=".")
    prepare.add_argument(
        "--date",
        default=None,
        help="override today's date (YYYY-MM-DD); mainly for tests",
    )
    prepare.add_argument(
        "--with-py",
        action="store_true",
        help=(
            "also bump quadraui-py (pyproject.toml + Cargo.toml) to the same "
            "version for a lockstep PyPI release; refuses if it would downgrade "
            "the wheel version"
        ),
    )
    prepare.set_defaults(func=cmd_prepare)

    promote = sub.add_parser(
        "promote", help="print the develop -> main promotion PR commands"
    )
    promote.add_argument("--repo-dir", default=".")
    promote.set_defaults(func=cmd_promote)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
