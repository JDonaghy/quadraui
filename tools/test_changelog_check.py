#!/usr/bin/env python3
"""Tests for `tools/changelog_check.py`'s [Unreleased]-entry enforcement (#1500).

Stdlib-only (`unittest`), mirroring `tools/test_example_coverage.py`'s
`TempGitRepo` pattern: a throwaway git repo seeded with `quadraui/src/` and
`CHANGELOG.md`, so each test commits two revisions and diffs them directly
rather than touching this repo's real history.

Run with:

    python3 tools/test_changelog_check.py
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import changelog_check  # noqa: E402


def run_git(args: list[str], cwd: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=True
    )


BASE_CHANGELOG = """\
# Changelog

## [Unreleased]

### Added

- placeholder entry so the section isn't empty at base.

## [0.1.0] - 2026-01-01

### Added

- initial release.
"""

BASE_SRC = """\
//! Example module.

pub fn greet() -> &'static str {
    "hi"
}
"""


class TempGitRepo:
    """A throwaway git repo with `quadraui/src/` + `CHANGELOG.md`, for diff tests."""

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.src_dir = self.root / "quadraui" / "src"
        self.src_dir.mkdir(parents=True)
        run_git(["init", "-q", "-b", "main"], self.root)
        run_git(["config", "user.email", "test@example.com"], self.root)
        run_git(["config", "user.name", "Test"], self.root)
        self.write_changelog(BASE_CHANGELOG)
        self.write_src("lib.rs", BASE_SRC)

    def __enter__(self) -> "TempGitRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def commit(self, message: str) -> str:
        run_git(["add", "-A"], self.root)
        run_git(["commit", "-q", "-m", message], self.root)
        return run_git(["rev-parse", "HEAD"], self.root).stdout.strip()

    def write_changelog(self, body: str) -> None:
        (self.root / "CHANGELOG.md").write_text(body)

    def write_src(self, name: str, body: str) -> None:
        (self.src_dir / name).write_text(body)


class RequiresChangelogEntryTest(unittest.TestCase):
    def test_src_change_without_entry_is_required(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog entry")

            required = changelog_check.requires_changelog_entry(base, head, repo.root)
            self.assertEqual(required, ["quadraui/src/lib.rs"])

    def test_comment_only_src_change_is_exempt(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + "// just a trailing comment\n")
            head = repo.commit("comment-only change")

            required = changelog_check.requires_changelog_entry(base, head, repo.root)
            self.assertEqual(required, [])

    def test_test_module_only_change_is_exempt(self):
        with TempGitRepo() as repo:
            src_with_tests = BASE_SRC + (
                "\n#[cfg(test)]\nmod tests {\n"
                "    #[test]\n    fn it_works() {\n        assert_eq!(2 + 2, 4);\n    }\n}\n"
            )
            repo.write_src("lib.rs", src_with_tests)
            base = repo.commit("base with a tests module")

            updated = src_with_tests.replace(
                "assert_eq!(2 + 2, 4);", "assert_eq!(2 + 2, 4); // extra assertion"
            ).replace(
                "fn it_works() {",
                "fn it_works() {\n        assert!(true);",
            )
            repo.write_src("lib.rs", updated)
            head = repo.commit("edit only inside #[cfg(test)] mod tests")

            required = changelog_check.requires_changelog_entry(base, head, repo.root)
            self.assertEqual(required, [])

    def test_exempt_path_is_skipped(self):
        with TempGitRepo() as repo:
            examples_dir = repo.root / "quadraui" / "examples"
            examples_dir.mkdir(parents=True)
            base = repo.commit("base")
            (examples_dir / "tui_demo.rs").write_text("fn main() {}\n")
            # quadraui/examples/ is not under quadraui/src/, so this alone
            # wouldn't match anyway -- also exercise an explicit src-rooted
            # exempt path to prove the glob mechanism itself works.
            tests_dir = repo.src_dir / "tests"
            tests_dir.mkdir()
            (tests_dir / "helpers.rs").write_text("pub fn helper() {}\n")
            head = repo.commit("add files under exempt paths")

            required = changelog_check.requires_changelog_entry(base, head, repo.root)
            self.assertEqual(required, [])


class HasUnreleasedEntryTest(unittest.TestCase):
    def test_entry_added_inside_unreleased_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- placeholder entry so the section isn't empty at base.",
                    "- placeholder entry so the section isn't empty at base.\n"
                    "- `greet::farewell()` (issue #1500) -- adds a farewell helper.",
                )
            )
            head = repo.commit("add farewell() with a changelog entry")

            self.assertTrue(changelog_check.has_unreleased_entry(base, head, repo.root))

    def test_entry_added_outside_unreleased_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- initial release.",
                    "- initial release.\n"
                    "- `greet::farewell()` (issue #1500) -- misplaced entry.",
                )
            )
            head = repo.commit("add farewell() with entry in the WRONG section")

            self.assertFalse(changelog_check.has_unreleased_entry(base, head, repo.root))

    def test_no_entry_at_all_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog touch at all")

            self.assertFalse(changelog_check.has_unreleased_entry(base, head, repo.root))


class MainCliTest(unittest.TestCase):
    """End-to-end `main()` exercises, matching the issue's five fixtures."""

    def test_entry_added_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- placeholder entry so the section isn't empty at base.",
                    "- placeholder entry so the section isn't empty at base.\n"
                    "- `greet::farewell()` (issue #1500).",
                )
            )
            head = repo.commit("add farewell() with a changelog entry")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 0)

    def test_src_change_without_entry_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog entry")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_entry_outside_unreleased_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- initial release.",
                    "- initial release.\n- `greet::farewell()` (issue #1500) -- misplaced.",
                )
            )
            head = repo.commit("add farewell() with entry in the WRONG section")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_comment_only_change_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + "// just a trailing comment\n")
            head = repo.commit("comment-only change, no changelog entry")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 0)

    def test_no_changelog_label_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog entry, but labeled")

            exit_code = self._run_main(repo.root, base, head, labels="no-changelog")
            self.assertEqual(exit_code, 0)

    @staticmethod
    def _run_main(repo_root: Path, base: str, head: str, labels: str) -> int:
        """Invoke `changelog_check.main()` against `repo_root` by pointing
        `REPO_ROOT` at the temp repo for the duration of the call."""
        real_root = changelog_check.REPO_ROOT
        changelog_check.REPO_ROOT = repo_root
        try:
            return changelog_check.main(["--labels", labels, base, head])
        finally:
            changelog_check.REPO_ROOT = real_root


if __name__ == "__main__":
    unittest.main()
