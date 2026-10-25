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
commands, with the PR body pre-filled with that section. Never runs them.
`--merge` is load-bearing in that last command: a squash or rebase merge of
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
        changelog_path.write_text(
            retitle_changelog(changelog_text, args.version, date), encoding="utf-8"
        )

        paths_to_add = [
            "quadraui/Cargo.toml",
            "Cargo.lock",
            "README.md",
            "quadraui/src/lib.rs",
            "CHANGELOG.md",
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
