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

Two subcommands, used from `release.yml`:

    python3 scripts/release_decide.py decide \\
        --version 0.1.2 --remote origin --repo-dir .

    python3 scripts/release_decide.py notes \\
        --changelog CHANGELOG.md --version 0.1.2 --out notes.md

`decide` exits 0 and prints `decision=noop` or `decision=publish` (also
appended to the file named by `$GITHUB_OUTPUT`, if set) and touches the
network only via `git ls-remote` against the real remote tags, never the
shallow local checkout. `notes` extracts the `## [<version>]` CHANGELOG.md
section and exits 1, writing nothing, if that section is missing or empty.
"""
from __future__ import annotations

import argparse
import os
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

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
