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

# Mirrors the real shape of `quadraui/src/gtk/backend.rs` /
# `quadraui/src/tui/backend.rs` that #1500's review flagged: an early,
# lone `#[cfg(test)]`-gated `use` import, then real production code, then
# the file's actual trailing `#[cfg(test)] mod tests { ... }` block. A
# "first #[cfg(test)] occurrence" boundary would wrongly treat everything
# from the early import onward -- including the production code -- as
# exempt test code; the correct boundary is the trailing block only.
EARLY_CFG_TEST_IMPORT_SRC = """\
//! Example module mirroring gtk/backend.rs's shape: an early
//! `#[cfg(test)]`-gated import, followed by real production code,
//! followed by the file's true trailing `#[cfg(test)] mod tests { ... }`.

#[cfg(test)]
use std::cell::Cell;

pub fn greet() -> &'static str {
    "hi"
}

pub fn farewell() -> &'static str {
    "bye"
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
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


class TestModuleBoundaryTest(unittest.TestCase):
    """Regression coverage for #1500's review: the boundary must be the
    file's trailing, EOF-spanning `#[cfg(test)] mod ... {` block, not the
    first `#[cfg(test)]` occurrence anywhere in the file."""

    def test_boundary_is_the_trailing_block_not_the_early_import(self):
        source = EARLY_CFG_TEST_IMPORT_SRC
        lines = source.splitlines()
        boundary = changelog_check.test_module_boundary(source)

        self.assertIsNotNone(boundary)
        # 1-indexed boundary line must be the `#[cfg(test)]` immediately
        # above `mod tests {`, not the earlier lone `use` import's attribute.
        self.assertEqual(lines[boundary - 1], "#[cfg(test)]")
        self.assertEqual(lines[boundary], "mod tests {")

    def test_lone_cfg_test_import_with_no_trailing_mod_is_not_a_boundary(self):
        source = "#[cfg(test)]\nuse std::cell::Cell;\n\npub fn greet() {}\n"
        self.assertIsNone(changelog_check.test_module_boundary(source))

    def test_nested_cfg_test_mod_that_closes_before_eof_is_not_a_boundary(self):
        # A `#[cfg(test)]` test submodule nested inside another `mod` (so
        # indented, and closing well before EOF) must never be mistaken
        # for the trailing boundary -- mirrors win/run.rs's shape.
        source = (
            "mod win32 {\n"
            "    #[cfg(test)]\n"
            "    mod live_window_nchittest_tests {\n"
            "        #[test]\n"
            "        fn it_works() {}\n"
            "    }\n"
            "\n"
            "    pub fn run() {}\n"
            "}\n"
            "\n"
            "pub fn run_with() {}\n"
        )
        self.assertIsNone(changelog_check.test_module_boundary(source))


class TestModuleOnlyChangeShapeTest(unittest.TestCase):
    """`requires_changelog_entry` on the `gtk/backend.rs`-shaped fixture:
    an early `#[cfg(test)]` import must not exempt real production code
    that follows it, only the file's actual trailing test module."""

    def test_change_after_early_cfg_test_import_requires_entry(self):
        with TempGitRepo() as repo:
            repo.write_src("lib.rs", EARLY_CFG_TEST_IMPORT_SRC)
            base = repo.commit("base mirroring gtk/backend.rs's shape")

            updated = EARLY_CFG_TEST_IMPORT_SRC.replace('"bye"', '"goodbye"')
            repo.write_src("lib.rs", updated)
            head = repo.commit(
                "change production code between the early import and the "
                "trailing mod tests"
            )

            required = changelog_check.requires_changelog_entry(base, head, repo.root)
            self.assertEqual(required, ["quadraui/src/lib.rs"])

    def test_change_inside_trailing_mod_tests_still_exempt(self):
        with TempGitRepo() as repo:
            repo.write_src("lib.rs", EARLY_CFG_TEST_IMPORT_SRC)
            base = repo.commit("base mirroring gtk/backend.rs's shape")

            updated = EARLY_CFG_TEST_IMPORT_SRC.replace(
                "assert_eq!(2 + 2, 4);", "assert_eq!(2 + 2, 4); // extra"
            )
            repo.write_src("lib.rs", updated)
            head = repo.commit("edit only inside the trailing mod tests")

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
