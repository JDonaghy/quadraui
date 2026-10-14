#!/usr/bin/env python3
"""Decision logic for `.github/workflows/release.yml`'s release gate (#1368).

The workflow runs on every push to `main`. Most of those pushes don't bump
`quadraui/Cargo.toml`'s version, so most runs must be a no-op — this module
is the part of that decision worth testing outside a real GitHub Actions run:

  - tag `v<version>` already exists on the remote -> no-op (nothing to do;
    this merge didn't bump the version, or it was already released)
  - tag missing and `CHANGELOG.md` has a non-empty `## [<version>]` section
    -> publish (the workflow then runs `cargo publish`, pushes the tag, and
    creates the GitHub release)
  - tag missing and the changelog section is missing/empty -> fail, before
    any tag is pushed or anything is published

Four subcommands, used from `release.yml` and `ci.yml`:

    python3 scripts/release_decide.py decide \\
        --version 0.1.2 --remote origin --repo-dir .

    python3 scripts/release_decide.py notes \\
        --changelog CHANGELOG.md --version 0.1.2 --out notes.md

    python3 scripts/release_decide.py check-lock --repo-dir .

    python3 scripts/release_decide.py version --repo-dir .

`decide` exits 0 and prints `decision=noop` or `decision=publish` (also
appended to the file named by `$GITHUB_OUTPUT`, if set) and touches the
network only via `git ls-remote` against the real remote tags, never the
shallow local checkout. `notes` extracts the `## [<version>]` CHANGELOG.md
section and exits 1, writing nothing, if that section is missing or empty —
`ci.yml`'s `changelog-version-check` job runs this on the `develop` -> `main`
promotion PR itself, against the version `version` below reads, so a
CHANGELOG still titled `## [Unreleased]` fails the PR before merge instead of
only `release.yml`'s own, identical check catching it after `main` has
already moved.

`version` prints `version=<X.Y.Z>` (also appended to `$GITHUB_OUTPUT`),
reading `quadraui/Cargo.toml`'s `[package]` version directly — no `cargo`
invocation, so it works in a checkout-only job with no toolchain installed,
the same reason `check-lock` reads both its files directly instead of
shelling out to `cargo metadata`.

`check-lock` guards against a version bump that edited `quadraui/Cargo.toml`
directly instead of going through `scripts/release.py prepare`, which also
refreshes `Cargo.lock`. It compares the `version =` field of
`quadraui/Cargo.toml` against the `version =` of `Cargo.lock`'s
`[[package]] name = "quadraui"` entry and fails if they disagree, which is
exactly the mismatch `cargo publish --locked` would reject later. It runs
before `decide` or `notes`, and never rewrites the lockfile — the point is
to report the mismatch, not to paper over it.

`check-lock` deliberately reads both files directly instead of shelling out
to `cargo update -p quadraui --offline`: that call needs a populated
registry index for every one of quadraui's dependencies, which a fresh
CI checkout does not have, so it fails on a perfectly fresh lockfile. Plain
file reads make this check hermetic, toolchain-free and identical on every
host.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path


def changelog_section(text: str, version: str) -> str:
    """Return the body of CHANGELOG.md's `## [<version>]` section.

    Everything after the matching `## [<version>]` header line, up to (not
    including) the next `## [` header. Returns "" if no such header exists.
    """
    header = f"## [{version}]"
    lines = text.splitlines()
    body: list[str] = []
    in_section = False
    for line in lines:
        if line.startswith(header):
            in_section = True
            continue
        if in_section and line.startswith("## ["):
            break
        if in_section:
            body.append(line)
    return "\n".join(body)


def tag_exists(remote: str, tag: str, repo_dir: Path) -> bool:
    """Whether `tag` exists on `remote`, checked against the remote directly.

    Deliberately `git ls-remote`, not a local `git tag`/`git rev-parse`
    check: a shallow `actions/checkout` has no tag refs at all, so a local
    check would always say "missing" and every version bump would try to
    re-create a tag that already exists upstream.
    """
    result = subprocess.run(
        ["git", "ls-remote", "--exit-code", "--tags", remote, f"refs/tags/{tag}"],
        cwd=repo_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        return True
    if result.returncode == 2:
        return False
    raise RuntimeError(
        f"git ls-remote failed (exit {result.returncode}): {result.stderr.strip()}"
    )


def _write_github_output(name: str, value: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if not path:
        return
    with open(path, "a", encoding="utf-8") as f:
        f.write(f"{name}={value}\n")


def cmd_decide(args: argparse.Namespace) -> int:
    tag = f"v{args.version}"
    try:
        exists = tag_exists(args.remote, tag, Path(args.repo_dir))
    except RuntimeError as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 1
    decision = "noop" if exists else "publish"
    print(f"decision={decision}")
    _write_github_output("decision", decision)
    _write_github_output("tag", tag)
    return 0


MANIFEST_VERSION_RE = re.compile(r'(?m)^version\s*=\s*"([^"]+)"')


def manifest_version(cargo_toml_text: str) -> str | None:
    """Return `quadraui/Cargo.toml`'s `[package]` version, or None.

    The first top-level `version = "..."` line in the manifest, which is
    the one `[package]` owns: every later `version =` sits inside a
    `[dependencies.*]`-style table.
    """
    match = MANIFEST_VERSION_RE.search(cargo_toml_text)
    return match.group(1) if match else None


def locked_version(cargo_lock_text: str, package: str) -> str | None:
    """Return `package`'s version from a `Cargo.lock`, or None if absent.

    `Cargo.lock` is a sequence of `[[package]]` tables, each with its own
    `name`/`version` pair, so this walks the lines and returns the
    `version` that follows the matching `name`.
    """
    found = False
    for line in cargo_lock_text.splitlines():
        stripped = line.strip()
        if stripped == f'name = "{package}"':
            found = True
        elif found:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', stripped)
            if match:
                return match.group(1)
            if stripped.startswith("[["):
                return None
    return None


def cmd_check_lock(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    manifest_path = repo_dir / "quadraui" / "Cargo.toml"
    lock_path = repo_dir / "Cargo.lock"

    for path in (manifest_path, lock_path):
        if not path.is_file():
            print(f"::error::{path} does not exist", file=sys.stderr)
            return 1

    declared = manifest_version(manifest_path.read_text(encoding="utf-8"))
    if declared is None:
        print(
            f'::error::no `version = "..."` line in {manifest_path}',
            file=sys.stderr,
        )
        return 1

    locked = locked_version(lock_path.read_text(encoding="utf-8"), "quadraui")
    if locked is None:
        print(
            f'::error::{lock_path} has no `[[package]] name = "quadraui"` entry — '
            "regenerate it with `cargo update -p quadraui --offline` and commit "
            "the result.",
            file=sys.stderr,
        )
        return 1

    if locked != declared:
        print(
            f"::error::Cargo.lock is stale: quadraui/Cargo.toml declares version "
            f"{declared} but Cargo.lock pins {locked}. `cargo publish --locked` "
            "rejects that. Bump the version with `scripts/release.py prepare "
            "X.Y.Z`, which refreshes Cargo.lock in the same commit, or run "
            "`cargo update -p quadraui --offline` and commit the result.",
            file=sys.stderr,
        )
        return 1

    print(f"Cargo.lock matches quadraui/Cargo.toml's version ({declared}).")
    return 0


VERSION_FORMAT_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def cmd_version(args: argparse.Namespace) -> int:
    repo_dir = Path(args.repo_dir)
    manifest_path = repo_dir / "quadraui" / "Cargo.toml"

    if not manifest_path.is_file():
        print(f"::error::{manifest_path} does not exist", file=sys.stderr)
        return 1

    version = manifest_version(manifest_path.read_text(encoding="utf-8"))
    if version is None or not VERSION_FORMAT_RE.match(version):
        print(
            f'::error::{manifest_path} has no valid `version = "X.Y.Z"` '
            f"`[package]` line (found {version!r})",
            file=sys.stderr,
        )
        return 1

    print(f"version={version}")
    _write_github_output("version", version)
    return 0


def cmd_notes(args: argparse.Namespace) -> int:
    changelog_path = Path(args.changelog)
    text = changelog_path.read_text(encoding="utf-8")
    section = changelog_section(text, args.version)
    if not section.strip():
        print(
            f"::error::{changelog_path} has no non-empty '## [{args.version}]' section",
            file=sys.stderr,
        )
        return 1
    Path(args.out).write_text(section, encoding="utf-8")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    decide = sub.add_parser("decide", help="tag-exists-on-remote check")
    decide.add_argument("--version", required=True)
    decide.add_argument("--remote", default="origin")
    decide.add_argument("--repo-dir", default=".")
    decide.set_defaults(func=cmd_decide)

    notes = sub.add_parser("notes", help="extract CHANGELOG.md release notes")
    notes.add_argument("--changelog", required=True)
    notes.add_argument("--version", required=True)
    notes.add_argument("--out", required=True)
    notes.set_defaults(func=cmd_notes)

    check_lock = sub.add_parser(
        "check-lock", help="fail if Cargo.lock is stale relative to quadraui's version"
    )
    check_lock.add_argument("--repo-dir", default=".")
    check_lock.set_defaults(func=cmd_check_lock)

    version = sub.add_parser(
        "version", help="read quadraui/Cargo.toml's [package] version"
    )
    version.add_argument("--repo-dir", default=".")
    version.set_defaults(func=cmd_version)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
