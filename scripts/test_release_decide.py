#!/usr/bin/env python3
"""Tests for `scripts/release_decide.py`'s three decision branches (#1368).

Stdlib-only (`unittest`), matching `tools/test_example_coverage.py`'s own
rationale for avoiding a pytest dependency. Run with:

    python3 scripts/test_release_decide.py

Covers the three branches `.github/workflows/release.yml` relies on:

  - tag already exists on the remote -> no-op (`decide` prints
    `decision=noop`)
  - tag missing + a non-empty `## [X.Y.Z]` CHANGELOG.md section -> publish
    (`decide` prints `decision=publish`, `notes` extracts the section)
  - tag missing + no/empty CHANGELOG.md section -> `notes` fails (exit 1),
    before any tag is created or anything published
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_decide  # noqa: E402


def run_git(args: list[str], cwd: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
    )


class TempRemoteRepo:
    """A bare "remote" repo plus a clone that pushes tags to it.

    `git ls-remote` needs a real remote to query — a bare repo on local disk
    is the cheapest stand-in that still exercises the actual network-facing
    code path (`release_decide.tag_exists` never special-cases "local" vs.
    "remote", it always shells out to `git ls-remote`).
    """

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.remote = self.root / "remote.git"
        self.clone = self.root / "clone"
        run_git(["init", "-q", "--bare", "-b", "main", str(self.remote)], self.root)
        run_git(["clone", "-q", str(self.remote), str(self.clone)], self.root)
        run_git(["config", "user.email", "test@example.com"], self.clone)
        run_git(["config", "user.name", "Test"], self.clone)
        (self.clone / "README.md").write_text("placeholder\n")
        run_git(["add", "-A"], self.clone)
        run_git(["commit", "-q", "-m", "init"], self.clone)
        run_git(["push", "-q", "origin", "main"], self.clone)

    def __enter__(self) -> "TempRemoteRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def push_tag(self, tag: str) -> None:
        run_git(["tag", tag], self.clone)
        run_git(["push", "-q", "origin", tag], self.clone)


class TagExistsTests(unittest.TestCase):
    def test_tag_missing_on_remote(self):
        with TempRemoteRepo() as repo:
            self.assertFalse(release_decide.tag_exists("origin", "v9.9.9", repo.clone))

    def test_tag_present_on_remote(self):
        with TempRemoteRepo() as repo:
            repo.push_tag("v0.1.1")
            self.assertTrue(release_decide.tag_exists("origin", "v0.1.1", repo.clone))


class DecideCommandTests(unittest.TestCase):
    def test_decide_noop_when_tag_exists(self):
        with TempRemoteRepo() as repo:
            repo.push_tag("v0.1.1")
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(release_decide.__file__)),
                    "decide",
                    "--version",
                    "0.1.1",
                    "--repo-dir",
                    str(repo.clone),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("decision=noop", result.stdout)

    def test_decide_publish_when_tag_missing(self):
        with TempRemoteRepo() as repo:
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(release_decide.__file__)),
                    "decide",
                    "--version",
                    "0.2.0",
                    "--repo-dir",
                    str(repo.clone),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("decision=publish", result.stdout)


class ChangelogSectionTests(unittest.TestCase):
    CHANGELOG = """\
# Changelog

## [Unreleased]

### Added
- nothing yet

## [0.2.0] - 2026-10-08

### Added
- a thing

## [0.1.1] - 2026-09-01

### Fixed
- another thing
"""

    def test_extracts_matching_section_only(self):
        section = release_decide.changelog_section(self.CHANGELOG, "0.2.0")
        self.assertIn("a thing", section)
        self.assertNotIn("another thing", section)
        self.assertNotIn("nothing yet", section)

    def test_missing_section_is_empty(self):
        section = release_decide.changelog_section(self.CHANGELOG, "9.9.9")
        self.assertEqual(section.strip(), "")


class NotesCommandTests(unittest.TestCase):
    def _write_changelog(self, tmp: Path, text: str) -> Path:
        path = tmp / "CHANGELOG.md"
        path.write_text(text)
        return path

    def test_notes_extracts_non_empty_section(self):
        with tempfile.TemporaryDirectory() as tmp_str:
            tmp = Path(tmp_str)
            changelog = self._write_changelog(
                tmp,
                "## [0.2.0] - 2026-10-08\n\n### Added\n- a thing\n\n## [0.1.1]\n",
            )
            out = tmp / "notes.md"
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(release_decide.__file__)),
                    "notes",
                    "--changelog",
                    str(changelog),
                    "--version",
                    "0.2.0",
                    "--out",
                    str(out),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("a thing", out.read_text())

    def test_notes_fails_on_missing_section(self):
        with tempfile.TemporaryDirectory() as tmp_str:
            tmp = Path(tmp_str)
            changelog = self._write_changelog(tmp, "## [0.1.1]\n\nsomething\n")
            out = tmp / "notes.md"
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(release_decide.__file__)),
                    "notes",
                    "--changelog",
                    str(changelog),
                    "--version",
                    "0.2.0",
                    "--out",
                    str(out),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 1)
            self.assertFalse(out.exists())

    def test_notes_fails_on_empty_section(self):
        with tempfile.TemporaryDirectory() as tmp_str:
            tmp = Path(tmp_str)
            changelog = self._write_changelog(
                tmp, "## [0.2.0]\n\n\n## [0.1.1]\n\nsomething\n"
            )
            out = tmp / "notes.md"
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(release_decide.__file__)),
                    "notes",
                    "--changelog",
                    str(changelog),
                    "--version",
                    "0.2.0",
                    "--out",
                    str(out),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 1)
            self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
