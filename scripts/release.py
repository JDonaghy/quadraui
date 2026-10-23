#!/usr/bin/env python3
"""One-command release prep + promotion helper for `quadraui` (#1501).

A release today is several hand steps: bump `quadraui/Cargo.toml`'s
version, refresh `Cargo.lock`, rewrite the hard-coded version strings in
`README.md` and `quadraui/src/lib.rs` that `quadraui/tests/readme_truth.rs`
cross-checks against `CARGO_PKG_VERSION`, retitle `CHANGELOG.md`'s
`[Unreleased]` section with a date and add a fresh empty one, commit, open
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
  3. refuses if `CHANGELOG.md`'s `## [Unreleased]` section is empty (nothing
     to release);
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
  8. retitles `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and inserts a
     fresh empty `## [Unreleased]` above it;
  9. commits the five files;
  10. prints the `git push` and `gh pr create --base develop` commands for
      a human to run.

All refusals exit 1 before touching the working tree (step 4 is the first
mutation).

`promote`, run from a checkout with `develop` fully merged into it: reads
`CHANGELOG.md`'s most recent released (non-`Unreleased`) section and prints
the `gh pr create --base main --head develop` and `gh pr merge --merge`
commands, with the PR body pre-filled with that section. Never runs them.
`--merge` is load-bearing in that last command: a squash or rebase merge of
`develop` into `main` leaves the two branches' histories diverged, breaking
the next promotion's merge-base.
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


def read_package_version(cargo_toml_text: str) -> str:
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', cargo_toml_text)
    if not match:
        raise ReleaseError("could not find a `version = \"...\"` line in quadraui/Cargo.toml")
    return match.group(1)


def set_package_version(cargo_toml_text: str, new_version: str) -> str:
    pattern = re.compile(r'(?m)^(version\s*=\s*)"[^"]+"')
    new_text, count = pattern.subn(rf'\1"{new_version}"', cargo_toml_text, count=1)
    if count != 1:
        raise ReleaseError("could not find a `version = \"...\"` line in quadraui/Cargo.toml")
    return new_text


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


def retitle_changelog(changelog_text: str, new_version: str, date: str) -> str:
    """Retitle `## [Unreleased]` to `## [X.Y.Z] - date`, adding a fresh one above it."""
    matches = list(UNRELEASED_HEADER_RE.finditer(changelog_text))
    if len(matches) != 1:
        raise ReleaseError(
            f"expected exactly one '## [Unreleased]' header in CHANGELOG.md, found {len(matches)}"
        )
    match = matches[0]
    replacement = f"## [Unreleased]\n\n## [{new_version}] - {date}"
    return changelog_text[: match.start()] + replacement + changelog_text[match.end():]


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


def refresh_lock(repo_dir: Path) -> None:
    """Run `cargo update -p quadraui --offline` to sync Cargo.lock's version line."""
    result = subprocess.run(
        ["cargo", "update", "-p", "quadraui", "--offline"],
        cwd=repo_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ReleaseError(
            "`cargo update -p quadraui --offline` failed:\n" + result.stderr
        )


def verify_lock_diff_is_quadraui_version_only(repo_dir: Path) -> None:
    """Refuse if `cargo update` touched anything in Cargo.lock besides quadraui's version line.

    Deliberately paranoid: `cargo update -p quadraui` should only ever bump
    quadraui's own `version =` line (it's a path dependency, nothing else in
    the lock depends on its registry resolution), but if a future cargo
    version ever resolves something else too, this must fail loudly rather
    than let an unrelated lockfile change slip into a release commit.
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
        raise ReleaseError(
            "`cargo update -p quadraui --offline` made no changes to Cargo.lock"
        )
    bad_lines = [line for line in changed_lines if not LOCK_VERSION_LINE_RE.match(line)]
    if bad_lines:
        raise ReleaseError(
            "`cargo update -p quadraui --offline` changed more than quadraui's "
            "own version line in Cargo.lock:\n" + "\n".join(bad_lines)
        )


def cmd_prepare(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    cargo_toml_path = repo_dir / "quadraui" / "Cargo.toml"
    changelog_path = repo_dir / "CHANGELOG.md"
    readme_path = repo_dir / "README.md"
    lib_rs_path = repo_dir / "quadraui" / "src" / "lib.rs"

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

        changelog_text = changelog_path.read_text(encoding="utf-8")
        if not unreleased_body(changelog_text).strip():
            raise ReleaseError(
                "CHANGELOG.md's '## [Unreleased]' section is empty; nothing to release"
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

        refresh_lock(repo_dir)
        verify_lock_diff_is_quadraui_version_only(repo_dir)

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
        changelog_path.write_text(
            retitle_changelog(changelog_text, args.version, date), encoding="utf-8"
        )

        run_git(
            [
                "add",
                "quadraui/Cargo.toml",
                "Cargo.lock",
                "README.md",
                "quadraui/src/lib.rs",
                "CHANGELOG.md",
            ],
            repo_dir,
        )
        run_git(
            [
                "commit",
                "-m",
                f"Release v{args.version} (version bump + CHANGELOG)",
            ],
            repo_dir,
        )
    except ReleaseError as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as exc:
        print(f"::error::`{' '.join(exc.cmd)}` failed:\n{exc.stderr}", file=sys.stderr)
        return 1

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
    return 0


def cmd_promote(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    changelog_path = repo_dir / "CHANGELOG.md"

    try:
        changelog_text = changelog_path.read_text(encoding="utf-8")
        version = latest_release_version(changelog_text)
        body = release_section_body(changelog_text, version)
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
