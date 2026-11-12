#!/usr/bin/env python3
"""Tests for `tools/changelog_check.py`'s `changelog.d/` fragment
enforcement (#1500, #2419).

Stdlib-only (`unittest`), mirroring `tools/test_example_coverage.py`'s
`TempGitRepo` pattern: a throwaway git repo seeded with `quadraui/src/` and
`CHANGELOG.md`, so each test commits two revisions and diffs them directly
rather than touching this repo's real history.

Run with:

    python3 tools/test_changelog_check.py
"""
from __future__ import annotations

import contextlib
import io
import os
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

BASE_MANIFEST = """\
[package]
name = "quadraui"
version = "0.1.0"
edition = "2021"
"""

# The shape `scripts/release.py prepare` leaves a develop -> main
# promotion PR in: `[Unreleased]` empty, its former contents moved into a
# `## [X.Y.Z]` section matching the bumped `quadraui/Cargo.toml` version.
PROMOTION_CHANGELOG = """\
# Changelog

## [Unreleased]

## [0.2.0] - 2026-02-02

### Added

- placeholder entry so the section isn't empty at base.

## [0.1.0] - 2026-01-01

### Added

- initial release.
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
        self.write_manifest(BASE_MANIFEST)

    def __enter__(self) -> "TempGitRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def commit(self, message: str) -> str:
        run_git(["add", "-A"], self.root)
        run_git(["commit", "-q", "-m", message], self.root)
        return run_git(["rev-parse", "HEAD"], self.root).stdout.strip()

    def write_changelog(self, body: str) -> None:
        (self.root / "CHANGELOG.md").write_text(body, encoding="utf-8")

    def write_src(self, name: str, body: str) -> None:
        (self.src_dir / name).write_text(body, encoding="utf-8")

    def write_manifest(self, body: str) -> None:
        (self.root / "quadraui" / "Cargo.toml").write_text(body, encoding="utf-8")

    def write_fragment(self, name: str, body: str) -> None:
        fragment_dir = self.root / "changelog.d"
        fragment_dir.mkdir(exist_ok=True)
        (fragment_dir / name).write_text(body, encoding="utf-8")


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


class UnreleasedSectionEditedTest(unittest.TestCase):
    """`unreleased_section_edited`: detects a direct edit to `[Unreleased]`,
    which `main()` now forbids (#2419) rather than treats as satisfying the
    changelog-entry requirement."""

    def test_entry_added_inside_unreleased_is_detected(self):
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
            head = repo.commit("add farewell() with a direct CHANGELOG.md edit")

            self.assertTrue(
                changelog_check.unreleased_section_edited(base, head, repo.root)
            )

    def test_entry_added_outside_unreleased_is_not_detected(self):
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

            self.assertFalse(
                changelog_check.unreleased_section_edited(base, head, repo.root)
            )

    def test_no_entry_at_all_is_not_detected(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog touch at all")

            self.assertFalse(
                changelog_check.unreleased_section_edited(base, head, repo.root)
            )


class NewFragmentPathsTest(unittest.TestCase):
    def test_added_fragment_is_found(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_fragment("2419.changed.md", "- a fragment-based entry.\n")
            head = repo.commit("add a fragment")

            self.assertEqual(
                changelog_check.new_fragment_paths(base, head, repo.root),
                ["changelog.d/2419.changed.md"],
            )

    def test_fragment_readme_is_excluded(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_fragment("README.md", "# changelog.d\n")
            head = repo.commit("add changelog.d/README.md")

            self.assertEqual(
                changelog_check.new_fragment_paths(base, head, repo.root), []
            )

    def test_modified_fragment_is_not_newly_added(self):
        with TempGitRepo() as repo:
            repo.write_fragment("2419.changed.md", "- original text.\n")
            base = repo.commit("base with an existing fragment")
            repo.write_fragment("2419.changed.md", "- edited text.\n")
            head = repo.commit("edit, not add, the fragment")

            self.assertEqual(
                changelog_check.new_fragment_paths(base, head, repo.root), []
            )

    def test_no_fragment_added_is_empty(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("ordinary src change, no fragment")

            self.assertEqual(
                changelog_check.new_fragment_paths(base, head, repo.root), []
            )


class ValidateFragmentTest(unittest.TestCase):
    def test_well_formed_fragment_is_valid(self):
        error = changelog_check.validate_fragment(
            "changelog.d/2419.changed.md", "- a valid fragment.\n"
        )
        self.assertIsNone(error)

    def test_malformed_name_is_invalid(self):
        error = changelog_check.validate_fragment(
            "changelog.d/not-an-issue-number.changed.md", "- content.\n"
        )
        self.assertIsNotNone(error)
        self.assertIn("changelog.d/not-an-issue-number.changed.md", error)

    def test_unknown_kind_is_invalid(self):
        error = changelog_check.validate_fragment(
            "changelog.d/2419.security.md", "- content.\n"
        )
        self.assertIsNotNone(error)

    def test_empty_content_is_invalid(self):
        error = changelog_check.validate_fragment("changelog.d/2419.changed.md", "   \n")
        self.assertIsNotNone(error)

    def test_missing_content_is_invalid(self):
        error = changelog_check.validate_fragment("changelog.d/2419.changed.md", None)
        self.assertIsNotNone(error)


class ReleasePromotionVersionTest(unittest.TestCase):
    """A develop -> main promotion PR (or a release-X.Y.Z -> develop bump
    PR) legitimately leaves `[Unreleased]` empty, so it must not need a
    `[Unreleased]` entry -- but only when its own diff actually bumped the
    version."""

    def test_promotion_shape_returns_the_bumped_version(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            head = repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

            self.assertEqual(
                changelog_check.release_promotion_version(base, head, repo.root),
                "0.2.0",
            )

    def test_unchanged_version_with_empty_unreleased_is_not_exempt(self):
        """`[Unreleased]` empty but the manifest version didn't change --
        e.g. left empty by mistake on an ordinary PR -- must not match the
        *previous* release's still-present, non-empty section."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- placeholder entry so the section isn't empty at base.\n\n", ""
                )
            )
            head = repo.commit("empty [Unreleased], but no version bump happened")

            self.assertIsNone(
                changelog_check.release_promotion_version(base, head, repo.root)
            )

    def test_non_promotion_shape_with_empty_unreleased_is_not_exempt(self):
        """Version bumped, `[Unreleased]` empty, but no versioned section
        matches the new version -- e.g. a typo'd heading -- must not be
        mistaken for a promotion."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                PROMOTION_CHANGELOG.replace("## [0.2.0]", "## [9.9.9]")
            )
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            head = repo.commit("empty [Unreleased], but no matching version section")

            self.assertIsNone(
                changelog_check.release_promotion_version(base, head, repo.root)
            )

    def test_unreleased_still_has_content_is_not_exempt(self):
        """A promotion PR that also picked up a genuine `[Unreleased]`
        addition afterwards still needs the ordinary entry check."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                PROMOTION_CHANGELOG.replace(
                    "## [Unreleased]\n",
                    "## [Unreleased]\n\n- a late, unrelated addition.\n",
                )
            )
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            head = repo.commit("promotion shape, but [Unreleased] isn't empty")

            self.assertIsNone(
                changelog_check.release_promotion_version(base, head, repo.root)
            )

    def test_stale_branch_forked_after_a_bump_is_not_exempt(self):
        """A feature branch forked right after a promotion commit never
        touches the manifest or `CHANGELOG.md` itself, so it inherits an
        empty `[Unreleased]` and a non-empty `## [X.Y.Z]` section for the
        version it was forked at. If `develop` bumps the version again
        while that branch is still open, `base`'s version now differs
        from `head`'s -- but `head`'s own diff never bumped anything, so
        this must not match the promotion shape."""
        with TempGitRepo() as repo:
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

            run_git(["checkout", "-b", "feature"], repo.root)
            repo.write_src(
                "lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n'
            )
            head = repo.commit(
                "stale branch: ordinary src change, no manifest/changelog touch"
            )

            run_git(["checkout", "main"], repo.root)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.3.0"'))
            repo.write_changelog(PROMOTION_CHANGELOG.replace("## [0.2.0]", "## [0.3.0]"))
            base = repo.commit("Release v0.3.0 (version bump + CHANGELOG)")

            self.assertIsNone(
                changelog_check.release_promotion_version(base, head, repo.root)
            )

    def test_downgraded_version_is_not_exempt(self):
        """A manifest edit that lowers the version -- e.g. a reverted bump
        -- must not match just because the versions differ and the rest
        of the promotion shape happens to line up."""
        with TempGitRepo() as repo:
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.3.0"'))
            repo.write_changelog(PROMOTION_CHANGELOG.replace("## [0.2.0]", "## [0.3.0]"))
            base = repo.commit("base, at 0.3.0")

            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            head = repo.commit("revert the bump back down to 0.2.0")

            self.assertIsNone(
                changelog_check.release_promotion_version(base, head, repo.root)
            )


class IsReleaseBumpPrTest(unittest.TestCase):
    """`is_release_bump_pr`: ci.yml's `bump-shape` job uses this to
    decide whether a PR can skip the full build/test/clippy matrix."""

    def _bump_commit(self, repo: TempGitRepo) -> str:
        """Commit the exact file set + shape `scripts/release.py prepare`
        leaves on a `release-X.Y.Z` branch."""
        repo.write_changelog(PROMOTION_CHANGELOG)
        repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
        (repo.root / "README.md").write_text("# quadraui\n\n`0.2.x`\n", encoding="utf-8")
        return repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

    def test_pure_bump_commit_is_bump_shaped(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            head = self._bump_commit(repo)

            self.assertTrue(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_bump_plus_an_unrelated_file_is_not_bump_shaped(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            # `other.rs` is a real src file outside BUMP_FILE_SET -- unlike
            # `lib.rs`, which `scripts/release.py prepare` itself touches
            # (its README/lib.rs version-string rewrite) and so is *in* the
            # bump set.
            repo.write_src("other.rs", "pub fn other() -> i32 {\n    1\n}\n")
            head = repo.commit("bump plus a real, unrelated src change")

            self.assertFalse(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_readme_only_edit_with_no_version_bump_is_not_bump_shaped(self):
        """Touches only files in the bump set, but never actually bumps the
        version -- e.g. a plain README typo fix -- must still get the full
        matrix, not be mistaken for a release bump."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            (repo.root / "README.md").write_text("# quadraui\n\ntypo fix\n", encoding="utf-8")
            head = repo.commit("README typo fix, no version bump")

            self.assertFalse(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_no_diff_is_not_bump_shaped(self):
        """`base == head` -- no changed files at all -- must not match."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            self.assertFalse(
                changelog_check.is_release_bump_pr(base, base, repo.root)
            )

    def test_bump_with_comment_only_lib_rs_doc_edit_is_still_bump_shaped(self):
        """`release.py prepare`'s own `lib.rs` edit -- rewriting the `//!`
        Status doc line -- must not disqualify the bump shape."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            repo.write_src("lib.rs", BASE_SRC.replace("Example module.", "Example module, v0.2.x."))
            head = repo.commit("bump plus lib.rs doc-comment rewrite")

            self.assertTrue(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_bump_with_a_real_lib_rs_code_change_is_not_bump_shaped(self):
        """`lib.rs` is in `BUMP_FILE_SET`, but a real `pub fn`/`pub mod`
        change there -- not just `release.py prepare`'s own doc-comment
        rewrite -- must still get the full matrix, even though every
        changed file is still nominally within the bump set."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("bump plus a real lib.rs code change")

            self.assertFalse(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_bump_that_also_deletes_fragments_is_still_bump_shaped(self):
        """`release.py prepare` (#2419) assembles every `changelog.d/*.md`
        fragment into `[Unreleased]` and deletes them in the same commit --
        those deletions must not disqualify the bump shape even though
        they're not in `BUMP_FILE_SET`."""
        with TempGitRepo() as repo:
            repo.write_fragment("2419.changed.md", "- a fragment entry.\n")
            repo.write_fragment("2420.fixed.md", "- another fragment entry.\n")
            base = repo.commit("base with two pending fragments")

            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            (repo.root / "README.md").write_text("# quadraui\n\n`0.2.x`\n", encoding="utf-8")
            (repo.root / "changelog.d" / "2419.changed.md").unlink()
            (repo.root / "changelog.d" / "2420.fixed.md").unlink()
            head = repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

            self.assertTrue(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_bump_that_adds_a_fragment_instead_of_deleting_is_not_bump_shaped(self):
        """A fragment *added* (not deleted) alongside a bump commit is not
        the shape `prepare` produces -- must still get the full matrix."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            repo.write_fragment("2421.added.md", "- a fragment added alongside a bump.\n")
            head = repo.commit("bump plus an added (not deleted) fragment")

            self.assertFalse(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )

    def test_bump_that_modifies_a_fragment_instead_of_deleting_is_not_bump_shaped(self):
        """A fragment *modified* (not deleted) alongside a bump commit is
        not the shape `prepare` produces either."""
        with TempGitRepo() as repo:
            repo.write_fragment("2419.changed.md", "- original text.\n")
            base = repo.commit("base with an existing fragment")

            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            repo.write_fragment("2419.changed.md", "- edited, not deleted, text.\n")
            head = repo.commit("bump plus a modified (not deleted) fragment")

            self.assertFalse(
                changelog_check.is_release_bump_pr(base, head, repo.root)
            )


class MainCliTest(unittest.TestCase):
    """End-to-end `main()` exercises, matching the issue's five fixtures."""

    def test_fragment_added_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_fragment("1500.added.md", "- `greet::farewell()`.\n")
            head = repo.commit("add farewell() with a changelog.d/ fragment")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 0)

    def test_src_change_without_entry_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            head = repo.commit("add farewell(), no changelog entry")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_malformed_fragment_name_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_fragment("farewell.added.md", "- `greet::farewell()`.\n")
            head = repo.commit("add farewell() with a malformed fragment name")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_empty_fragment_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_fragment("1500.added.md", "   \n")
            head = repo.commit("add farewell() with an empty fragment")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_direct_unreleased_edit_fails(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- placeholder entry so the section isn't empty at base.",
                    "- placeholder entry so the section isn't empty at base.\n"
                    "- `greet::farewell()` (issue #1500) -- direct edit, not a fragment.",
                )
            )
            head = repo.commit("add farewell() with a direct [Unreleased] edit")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_direct_unreleased_edit_fails_even_with_a_valid_fragment(self):
        """A fragment doesn't buy back a direct edit elsewhere in the same
        diff -- both are checked, and the direct edit alone is enough to
        fail."""
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_fragment("1500.added.md", "- `greet::farewell()`.\n")
            repo.write_changelog(
                BASE_CHANGELOG.replace(
                    "- placeholder entry so the section isn't empty at base.",
                    "- placeholder entry so the section isn't empty at base.\n"
                    "- a stray direct edit alongside the fragment.",
                )
            )
            head = repo.commit("fragment plus a stray direct edit")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 1)

    def test_comment_only_change_passes(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + "// just a trailing comment\n")
            head = repo.commit("comment-only change, no changelog entry")

            exit_code = self._run_main(repo.root, base, head, labels="")
            self.assertEqual(exit_code, 0)

    def test_release_promotion_shape_passes_without_label_or_entry(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src("lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n')
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"'))
            head = repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

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


class PrintBumpShapeCliTest(unittest.TestCase):
    """`--print-bump-shape`: the mode `ci.yml`'s `bump-shape` job
    invokes, instead of the changelog-entry check."""

    def _run(self, repo_root: Path, base: str, head: str) -> tuple[int, str]:
        real_root = changelog_check.REPO_ROOT
        changelog_check.REPO_ROOT = repo_root
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                exit_code = changelog_check.main(
                    ["--print-bump-shape", base, head]
                )
        finally:
            changelog_check.REPO_ROOT = real_root
        return exit_code, buf.getvalue()

    def test_bump_shaped_diff_prints_true(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(
                BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"')
            )
            head = repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

            exit_code, output = self._run(repo.root, base, head)
            self.assertEqual(exit_code, 0)
            self.assertIn("bump_shape=true", output)

    def test_ordinary_diff_prints_false(self):
        with TempGitRepo() as repo:
            base = repo.commit("base")
            repo.write_src(
                "lib.rs", BASE_SRC + '\npub fn farewell() -> &\'static str {\n    "bye"\n}\n'
            )
            head = repo.commit("add farewell(), no version bump")

            exit_code, output = self._run(repo.root, base, head)
            self.assertEqual(exit_code, 0)
            self.assertIn("bump_shape=false", output)

    def test_writes_to_github_output_when_set(self):
        with TempGitRepo() as repo, tempfile.TemporaryDirectory() as out_dir:
            base = repo.commit("base")
            repo.write_changelog(PROMOTION_CHANGELOG)
            repo.write_manifest(
                BASE_MANIFEST.replace('version = "0.1.0"', 'version = "0.2.0"')
            )
            head = repo.commit("Release v0.2.0 (version bump + CHANGELOG)")

            output_path = Path(out_dir) / "github_output"
            real_github_output = os.environ.get("GITHUB_OUTPUT")
            os.environ["GITHUB_OUTPUT"] = str(output_path)
            try:
                self._run(repo.root, base, head)
            finally:
                if real_github_output is None:
                    os.environ.pop("GITHUB_OUTPUT", None)
                else:
                    os.environ["GITHUB_OUTPUT"] = real_github_output

            self.assertEqual(output_path.read_text(encoding="utf-8"), "bump_shape=true\n")


if __name__ == "__main__":
    unittest.main()
