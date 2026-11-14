#!/usr/bin/env python3
"""Tests for `scripts/release.py`'s `prepare`/`promote` commands (#1501).

Stdlib-only (`unittest`), matching `scripts/test_release_decide.py`'s own
rationale for avoiding a pytest dependency. Run with:

    python3 scripts/test_release.py

Each test builds a tiny throwaway cargo workspace (a `quadraui` member crate
with no external dependencies, so `cargo update -p quadraui --offline`
never needs network access) plus a `CHANGELOG.md`, inside a temp git repo,
and runs `release.py` against it as a subprocess — exercising the real
`git`/`cargo` calls `prepare` makes, not a mocked version of them.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release  # noqa: E402

RELEASE_PY = str(Path(release.__file__))

CHANGELOG_TEMPLATE = """\
# Changelog

## [Unreleased]

{unreleased_body}

## [0.1.0] - 2026-09-01

### Added

- the first thing
"""


def _hermetic_git_env() -> dict[str, str]:
    """See `scripts/test_release_decide.py`'s `_hermetic_git_env` for why."""
    env = dict(os.environ)
    env["GIT_CONFIG_GLOBAL"] = "/dev/null"
    env["GIT_CONFIG_SYSTEM"] = "/dev/null"
    return env


def run_git(args: list[str], cwd: Path, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=check,
        env=_hermetic_git_env(),
    )


def run_release(
    args: list[str], cwd: Path, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess:
    full_env = _hermetic_git_env()
    if env:
        full_env.update(env)
    return subprocess.run(
        [sys.executable, RELEASE_PY, *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        env=full_env,
    )


class ReleaseFixtureRepo:
    """A git repo with a minimal cargo workspace, mirroring quadraui's shape.

    `quadraui/Cargo.toml` + a generated `Cargo.lock` + `CHANGELOG.md` at the
    root, matching the real repo's layout closely enough for `release.py`'s
    path assumptions (`quadraui/Cargo.toml`, root `Cargo.lock` and
    `CHANGELOG.md`) without pulling in any real dependency.
    """

    def __init__(
        self,
        unreleased_body: str = "### Added\n\n- a new thing",
        *,
        with_py: bool = False,
        py_version: str = "0.1.0",
    ):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.with_py = with_py
        (self.root / "quadraui" / "src").mkdir(parents=True)
        members = '"quadraui"' + (', "quadraui-py"' if with_py else "")
        (self.root / "Cargo.toml").write_text(
            f'[workspace]\nmembers = [{members}]\nresolver = "2"\n'
        )
        (self.root / "quadraui" / "Cargo.toml").write_text(
            '[package]\nname = "quadraui"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        if with_py:
            (self.root / "quadraui-py" / "src").mkdir(parents=True)
            (self.root / "quadraui-py" / "Cargo.toml").write_text(
                "[package]\n"
                '# a comment with version = "9.9.9" that must not be matched\n'
                'name = "quadraui-py"\n'
                f'version = "{py_version}"\n'
                'edition = "2021"\n'
            )
            (self.root / "quadraui-py" / "src" / "lib.rs").write_text("pub fn p() {}\n")
            (self.root / "quadraui-py" / "pyproject.toml").write_text(
                "[build-system]\n"
                'requires = ["maturin>=1.5,<2.0"]\n'
                'build-backend = "maturin"\n'
                "\n"
                "[project]\n"
                'name = "quadraui"\n'
                f'version = "{py_version}"\n'
            )
        (self.root / "quadraui" / "src" / "lib.rs").write_text(
            "//! ## Status\n"
            "//!\n"
            "//! `0.1.x` — pre-1.0, published on crates.io.\n"
            "pub fn hello() {}\n"
        )
        (self.root / "README.md").write_text(
            "# quadraui\n"
            "\n"
            "## Status\n"
            "\n"
            "`0.1.x` — pre-1.0, published on crates.io.\n"
            "\n"
            "## Getting started\n"
            "\n"
            "```toml\n"
            'quadraui = { version = "0.1", features = ["tui", "gtk"] }\n'
            "```\n"
        )
        (self.root / "CHANGELOG.md").write_text(
            CHANGELOG_TEMPLATE.format(unreleased_body=unreleased_body)
        )

        run_git(["init", "-q", "-b", "develop"], self.root)
        run_git(["config", "user.email", "test@example.com"], self.root)
        run_git(["config", "user.name", "Test"], self.root)
        subprocess.run(
            ["cargo", "generate-lockfile", "--offline"],
            cwd=self.root,
            check=True,
            capture_output=True,
            text=True,
        )
        run_git(["add", "-A"], self.root)
        run_git(["commit", "-q", "-m", "init"], self.root)

    def __enter__(self) -> "ReleaseFixtureRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def changelog_text(self) -> str:
        return (self.root / "CHANGELOG.md").read_text(encoding="utf-8")

    def cargo_toml_text(self) -> str:
        return (self.root / "quadraui" / "Cargo.toml").read_text(encoding="utf-8")

    def lock_text(self) -> str:
        return (self.root / "Cargo.lock").read_text(encoding="utf-8")

    def readme_text(self) -> str:
        return (self.root / "README.md").read_text(encoding="utf-8")

    def lib_rs_text(self) -> str:
        return (self.root / "quadraui" / "src" / "lib.rs").read_text(encoding="utf-8")

    def py_pyproject_text(self) -> str:
        return (self.root / "quadraui-py" / "pyproject.toml").read_text(encoding="utf-8")

    def py_cargo_toml_text(self) -> str:
        return (self.root / "quadraui-py" / "Cargo.toml").read_text(encoding="utf-8")

    def current_branch(self) -> str:
        return run_git(["branch", "--show-current"], self.root).stdout.strip()

    def write_fragment(self, name: str, body: str) -> None:
        fragment_dir = self.root / "changelog.d"
        fragment_dir.mkdir(exist_ok=True)
        (fragment_dir / name).write_text(body, encoding="utf-8")

    def commit_fragment(self, name: str, body: str) -> None:
        """`write_fragment` plus the commit `prepare` expects -- a fragment
        is already tracked and committed on `develop` by the time a release
        branch runs; `prepare`'s own first check refuses a dirty tree."""
        self.write_fragment(name, body)
        run_git(["add", "-A"], self.root)
        run_git(["commit", "-q", "-m", f"add changelog.d/{name}"], self.root)

    def fragment_names(self) -> list[str]:
        fragment_dir = self.root / "changelog.d"
        if not fragment_dir.is_dir():
            return []
        return sorted(p.name for p in fragment_dir.glob("*.md"))


class PureFunctionTests(unittest.TestCase):
    def test_parse_version_accepts_x_y_z(self):
        self.assertEqual(release.parse_version("0.1.5"), (0, 1, 5))

    def test_parse_version_rejects_malformed(self):
        with self.assertRaises(release.ReleaseError):
            release.parse_version("0.1")
        with self.assertRaises(release.ReleaseError):
            release.parse_version("v0.1.5")

    def test_read_package_version(self):
        text = '[package]\nname = "quadraui"\nversion = "0.1.4"\nedition = "2021"\n'
        self.assertEqual(release.read_package_version(text), "0.1.4")

    def test_set_package_version_replaces_only_version_line(self):
        text = '[package]\nname = "quadraui"\nversion = "0.1.4"\nedition = "2021"\n'
        new_text = release.set_package_version(text, "0.2.0")
        self.assertIn('version = "0.2.0"', new_text)
        self.assertIn('name = "quadraui"', new_text)
        self.assertNotIn('"0.1.4"', new_text)

    def test_unreleased_body_extracts_only_that_section(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        body = release.unreleased_body(text)
        self.assertIn("a new thing", body)
        self.assertNotIn("the first thing", body)

    def test_unreleased_body_empty_section_is_blank(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="")
        self.assertEqual(release.unreleased_body(text).strip(), "")

    def test_retitle_changelog_inserts_fresh_unreleased(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        new_text = release.retitle_changelog(text, "0.2.0", "2026-10-11")
        self.assertIn("## [Unreleased]\n\n## [0.2.0] - 2026-10-11", new_text)
        # The old Unreleased content now lives under the new [0.2.0] header.
        idx_0_2_0 = new_text.index("## [0.2.0]")
        idx_a_new_thing = new_text.index("a new thing")
        idx_0_1_0 = new_text.index("## [0.1.0]")
        self.assertTrue(idx_0_2_0 < idx_a_new_thing < idx_0_1_0)
        # The fresh [Unreleased] section (before [0.2.0]) is empty.
        fresh_section = new_text[: new_text.index("## [0.2.0]")]
        self.assertEqual(release.unreleased_body(fresh_section + "\n## [0.2.0]\n").strip(), "")

    def test_retitle_changelog_with_fragments_assembles_into_new_version(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        fragments = [(100, "added", "- a fragment-based addition.\n")]
        new_text = release.retitle_changelog(text, "0.2.0", "2026-10-11", fragments=fragments)

        self.assertIn("## [Unreleased]\n\n## [0.2.0] - 2026-10-11", new_text)
        idx_0_2_0 = new_text.index("## [0.2.0]")
        idx_a_new_thing = new_text.index("a new thing")
        idx_fragment = new_text.index("a fragment-based addition")
        idx_0_1_0 = new_text.index("## [0.1.0]")
        # Existing bullet, then the fragment's, both inside [0.2.0], before [0.1.0].
        self.assertTrue(idx_0_2_0 < idx_a_new_thing < idx_fragment < idx_0_1_0)
        # The fresh [Unreleased] section (before [0.2.0]) is still empty.
        fresh_section = new_text[: new_text.index("## [0.2.0]")]
        self.assertEqual(release.unreleased_body(fresh_section + "\n## [0.2.0]\n").strip(), "")

    def test_retitle_changelog_without_fragments_is_byte_for_byte_unchanged(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        self.assertEqual(
            release.retitle_changelog(text, "0.2.0", "2026-10-11"),
            release.retitle_changelog(text, "0.2.0", "2026-10-11", fragments=[]),
        )

    def test_parse_fragment_filename_accepts_known_kinds(self):
        for kind in release.FRAGMENT_KINDS:
            self.assertEqual(
                release.parse_fragment_filename(f"2419.{kind}.md"), (2419, kind)
            )

    def test_parse_fragment_filename_rejects_malformed(self):
        self.assertIsNone(release.parse_fragment_filename("not-a-number.added.md"))
        self.assertIsNone(release.parse_fragment_filename("2419.security.md"))
        self.assertIsNone(release.parse_fragment_filename("README.md"))
        self.assertIsNone(release.parse_fragment_filename("2419.added.txt"))

    def test_parse_unreleased_sections_merges_duplicate_headings(self):
        # Mirrors #2409/#2416's hand-merge-conflict shape: the same heading
        # appearing twice must merge into one entry, not stay split.
        body = (
            "\n### Added\n- first added bullet.\n\n"
            "### Fixed\n- a fixed bullet.\n\n"
            "### Fixed\n- a second, separately-appended fixed bullet.\n\n"
        )
        preamble, sections = release.parse_unreleased_sections(body)
        self.assertEqual(preamble, "")
        self.assertEqual(list(sections.keys()), ["Added", "Fixed"])
        self.assertIn("a fixed bullet.", sections["Fixed"])
        self.assertIn("a second, separately-appended fixed bullet.", sections["Fixed"])

    def test_render_unreleased_body_empty_sections_is_blank(self):
        self.assertEqual(release.render_unreleased_body([]), "\n\n")

    def test_assemble_unreleased_body_creates_missing_heading_in_canonical_order(self):
        old_body = "\n### Added\n- an existing added bullet.\n\n"
        fragments = [
            (10, "fixed", "- a new fixed bullet.\n"),
            (20, "added", "- a new added bullet.\n"),
        ]
        assembled = release.assemble_unreleased_body(old_body, fragments)
        idx_added_heading = assembled.index("### Added")
        idx_existing = assembled.index("an existing added bullet")
        idx_new_added = assembled.index("a new added bullet")
        idx_fixed_heading = assembled.index("### Fixed")
        idx_new_fixed = assembled.index("a new fixed bullet")
        # Added's own bullets stay together, existing first; Fixed (newly
        # created, since old_body had none) comes after Added, canonically.
        self.assertTrue(
            idx_added_heading < idx_existing < idx_new_added < idx_fixed_heading < idx_new_fixed
        )

    def test_assemble_unreleased_body_sorts_fragments_by_issue_within_heading(self):
        fragments = [
            (200, "added", "- entry two hundred.\n"),
            (50, "added", "- entry fifty.\n"),
        ]
        assembled = release.assemble_unreleased_body("", fragments)
        self.assertLess(assembled.index("entry fifty"), assembled.index("entry two hundred"))

    def test_assemble_unreleased_body_no_fragments_reproduces_old_body(self):
        old_body = "\n### Added\n- an existing bullet.\n\n"
        self.assertEqual(release.assemble_unreleased_body(old_body, []), old_body)

    def test_discover_fragment_paths_excludes_readme_and_sorts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fragment_dir = root / "changelog.d"
            fragment_dir.mkdir()
            (fragment_dir / "README.md").write_text("# changelog.d\n")
            (fragment_dir / "200.added.md").write_text("- b.\n")
            (fragment_dir / "10.fixed.md").write_text("- a.\n")

            names = [p.name for p in release.discover_fragment_paths(root)]
            self.assertEqual(names, ["10.fixed.md", "200.added.md"])

    def test_discover_fragment_paths_missing_dir_is_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(release.discover_fragment_paths(Path(tmp)), [])

    def test_read_fragments_parses_issue_kind_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fragment_dir = root / "changelog.d"
            fragment_dir.mkdir()
            (fragment_dir / "2419.changed.md").write_text("- a changed fragment.\n")

            fragments = release.read_fragments(root)
            self.assertEqual(len(fragments), 1)
            issue, kind, content, path = fragments[0]
            self.assertEqual((issue, kind), (2419, "changed"))
            self.assertIn("a changed fragment.", content)
            self.assertEqual(path.name, "2419.changed.md")

    def test_read_fragments_raises_on_malformed_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fragment_dir = root / "changelog.d"
            fragment_dir.mkdir()
            (fragment_dir / "not-an-issue.added.md").write_text("- content.\n")

            with self.assertRaises(release.ReleaseError):
                release.read_fragments(root)

    def test_read_fragments_raises_on_empty_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fragment_dir = root / "changelog.d"
            fragment_dir.mkdir()
            (fragment_dir / "2419.added.md").write_text("   \n")

            with self.assertRaises(release.ReleaseError):
                release.read_fragments(root)

    def test_latest_release_version_skips_unreleased(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        self.assertEqual(release.latest_release_version(text), "0.1.0")

    def test_latest_release_version_fails_when_only_unreleased(self):
        text = "## [Unreleased]\n\n### Added\n\n- a new thing\n"
        with self.assertRaises(release.ReleaseError):
            release.latest_release_version(text)

    def test_release_section_body(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        body = release.release_section_body(text, "0.1.0")
        self.assertIn("the first thing", body)
        self.assertNotIn("a new thing", body)

    def test_promotion_pr_body_includes_full_section_when_short(self):
        text = CHANGELOG_TEMPLATE.format(unreleased_body="### Added\n\n- a new thing")
        body = release.promotion_pr_body(text, "0.1.0")
        self.assertIn("the first thing", body)
        self.assertIn("v0.1.0", body)
        self.assertLessEqual(len(body), release.PR_BODY_MAX_LEN)

    def test_promotion_pr_body_truncates_an_oversized_section(self):
        # A release section far larger than GitHub's PR body limit.
        huge_entries = "\n".join(f"- entry number {i}" for i in range(20_000))
        text = CHANGELOG_TEMPLATE.replace(
            "### Added\n\n- the first thing", f"### Added\n\n{huge_entries}"
        ).format(unreleased_body="")
        full_section = release.release_section_body(text, "0.1.0")
        self.assertGreater(len(full_section), release.PR_BODY_MAX_LEN)

        body = release.promotion_pr_body(text, "0.1.0")
        self.assertLessEqual(len(body), release.PR_BODY_MAX_LEN)
        self.assertIn("entry number 0", body)
        self.assertIn("truncated", body)
        self.assertNotIn(f"entry number {19_999}", body)

    def test_promotion_pr_body_respects_a_custom_max_len(self):
        # A section bigger than the custom 300-char budget but comfortably
        # bigger than the fixed header + truncation marker text too, so the
        # truncation path (not just a pass-through) is what's exercised.
        many_entries = "\n".join(f"- entry number {i}" for i in range(50))
        text = CHANGELOG_TEMPLATE.replace(
            "### Added\n\n- the first thing", f"### Added\n\n{many_entries}"
        ).format(unreleased_body="")
        self.assertGreater(len(release.release_section_body(text, "0.1.0")), 300)

        body = release.promotion_pr_body(text, "0.1.0", max_len=300)
        self.assertLessEqual(len(body), 300)
        self.assertIn("truncated", body)

    def test_promotion_pr_body_rejects_a_max_len_too_small_for_its_own_header(self):
        # A max_len smaller than the fixed header + truncation marker text
        # can never be honored — raise rather than silently return a body
        # longer than max_len.
        text = CHANGELOG_TEMPLATE.format(unreleased_body="")
        with self.assertRaises(release.ReleaseError):
            release.promotion_pr_body(text, "0.1.0", max_len=10)

    def test_pre_1_0_status_string(self):
        self.assertEqual(release.pre_1_0_status_string("0.3.1"), "0.3.x")

    def test_published_dependency_version(self):
        self.assertEqual(release.published_dependency_version("0.3.1"), "0.3")

    def test_set_pre_1_0_status_string_replaces_marker(self):
        text = "`0.1.x` — pre-1.0, published on crates.io.\n"
        new_text = release.set_pre_1_0_status_string(text, "0.2.0", label="test")
        self.assertIn("`0.2.x`", new_text)
        self.assertNotIn("`0.1.x`", new_text)

    def test_set_pre_1_0_status_string_raises_when_marker_missing(self):
        with self.assertRaises(release.ReleaseError):
            release.set_pre_1_0_status_string("no marker here", "0.2.0", label="test")

    def test_set_dependency_snippet_version_replaces_version(self):
        text = 'quadraui = { version = "0.1", features = ["tui"] }\n'
        new_text = release.set_dependency_snippet_version(text, "0.2.0", label="test")
        self.assertIn('quadraui = { version = "0.2"', new_text)
        self.assertNotIn('"0.1"', new_text)

    def test_set_dependency_snippet_version_raises_when_snippet_missing(self):
        with self.assertRaises(release.ReleaseError):
            release.set_dependency_snippet_version("no snippet here", "0.2.0", label="test")


class PrepareCommandTests(unittest.TestCase):
    def test_prepare_happy_path(self):
        with ReleaseFixtureRepo() as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--date", "2026-10-11"],
                repo.root,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            self.assertEqual(repo.current_branch(), "release-0.2.0")
            self.assertIn('version = "0.2.0"', repo.cargo_toml_text())

            lock_text = repo.lock_text()
            self.assertIn('name = "quadraui"', lock_text)
            self.assertIn('version = "0.2.0"', lock_text)

            changelog = repo.changelog_text()
            self.assertIn("## [Unreleased]\n\n## [0.2.0] - 2026-10-11", changelog)
            self.assertIn("a new thing", changelog)

            readme = repo.readme_text()
            self.assertIn("`0.2.x`", readme)
            self.assertNotIn("`0.1.x`", readme)
            self.assertIn('quadraui = { version = "0.2"', readme)
            self.assertNotIn('version = "0.1"', readme)

            lib_rs = repo.lib_rs_text()
            self.assertIn("`0.2.x`", lib_rs)
            self.assertNotIn("`0.1.x`", lib_rs)

            # A commit was made on the release branch.
            log = run_git(["log", "-1", "--format=%s"], repo.root).stdout
            self.assertIn("0.2.0", log)

            self.assertIn("git push -u origin release-0.2.0", result.stdout)
            self.assertIn("gh pr create --base develop", result.stdout)

    def test_prepare_refuses_dirty_tree(self):
        with ReleaseFixtureRepo() as repo:
            (repo.root / "quadraui" / "src" / "lib.rs").write_text("// dirty\n")
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("dirty", result.stderr)
            # No branch was created.
            self.assertEqual(repo.current_branch(), "develop")

    def test_prepare_refuses_version_not_greater(self):
        with ReleaseFixtureRepo() as repo:
            result = run_release(
                ["prepare", "0.1.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("not greater", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")

    def test_prepare_refuses_lesser_version(self):
        with ReleaseFixtureRepo() as repo:
            result = run_release(
                ["prepare", "0.0.9", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("not greater", result.stderr)

    def test_prepare_refuses_empty_unreleased(self):
        with ReleaseFixtureRepo(unreleased_body="") as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("empty", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")
            self.assertIn('version = "0.1.0"', repo.cargo_toml_text())

    def test_prepare_refuses_whitespace_only_unreleased(self):
        with ReleaseFixtureRepo(unreleased_body="\n\n") as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("empty", result.stderr)

    def test_prepare_assembles_fragments_of_mixed_kinds_and_deletes_them(self):
        with ReleaseFixtureRepo() as repo:
            repo.commit_fragment("200.added.md", "- a fragment-added thing.\n")
            repo.commit_fragment("50.fixed.md", "- a fragment-fixed thing.\n")

            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--date", "2026-10-11"],
                repo.root,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            changelog = repo.changelog_text()
            idx_0_2_0 = changelog.index("## [0.2.0]")
            idx_existing = changelog.index("a new thing")  # pre-existing [Unreleased] bullet
            idx_added_fragment = changelog.index("a fragment-added thing")
            idx_fixed_heading = changelog.index("### Fixed")
            idx_fixed_fragment = changelog.index("a fragment-fixed thing")
            idx_0_1_0 = changelog.index("## [0.1.0]")
            self.assertTrue(
                idx_0_2_0 < idx_existing < idx_added_fragment
                < idx_fixed_heading < idx_fixed_fragment < idx_0_1_0
            )

            # Fragments are deleted and the deletion is part of the commit.
            self.assertEqual(repo.fragment_names(), [])
            log_files = run_git(
                ["show", "--stat", "--format=", "HEAD"], repo.root
            ).stdout
            self.assertIn("200.added.md", log_files)
            self.assertIn("50.fixed.md", log_files)

    def test_prepare_with_only_fragments_and_empty_unreleased_succeeds(self):
        with ReleaseFixtureRepo(unreleased_body="") as repo:
            repo.commit_fragment("300.added.md", "- the only thing to release.\n")

            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--date", "2026-10-11"],
                repo.root,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            changelog = repo.changelog_text()
            self.assertIn("the only thing to release.", changelog)
            self.assertEqual(repo.fragment_names(), [])

    def test_prepare_refuses_when_unreleased_and_fragments_are_both_empty(self):
        with ReleaseFixtureRepo(unreleased_body="") as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("empty", result.stderr)
            self.assertIn("changelog.d", result.stderr)

    def test_prepare_refuses_malformed_fragment_name(self):
        with ReleaseFixtureRepo() as repo:
            repo.commit_fragment("not-an-issue.added.md", "- content.\n")

            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("not-an-issue.added.md", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")

    def test_prepare_refuses_empty_fragment(self):
        with ReleaseFixtureRepo() as repo:
            repo.commit_fragment("200.added.md", "   \n")

            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("200.added.md", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")

    def test_prepare_reuses_existing_branch_already_bumped(self):
        """A re-run from develop after the release branch already exists
        (e.g. a `release_full.py` run that timed out past this point)
        resumes by checking the existing branch out, instead of failing on
        `checkout -b`."""
        with ReleaseFixtureRepo() as repo:
            first = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--date", "2026-10-11"],
                repo.root,
            )
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertEqual(repo.current_branch(), "release-0.2.0")
            first_sha = run_git(["rev-parse", "HEAD"], repo.root).stdout.strip()

            run_git(["checkout", "develop"], repo.root)
            self.assertEqual(repo.current_branch(), "develop")

            second = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertIn("already exists", second.stdout)
            self.assertEqual(repo.current_branch(), "release-0.2.0")

            second_sha = run_git(["rev-parse", "HEAD"], repo.root).stdout.strip()
            self.assertEqual(first_sha, second_sha, "should not re-commit the bump")

    def test_prepare_refuses_existing_branch_with_mismatched_bump(self):
        with ReleaseFixtureRepo() as repo:
            run_git(["checkout", "-b", "release-0.2.0"], repo.root)
            (repo.root / "quadraui" / "Cargo.toml").write_text(
                '[package]\nname = "quadraui"\nversion = "0.3.0"\nedition = "2021"\n'
            )
            run_git(["commit", "-a", "-m", "stray bump"], repo.root)
            run_git(["checkout", "develop"], repo.root)

            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root)], repo.root
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("already exists", result.stderr)
            self.assertIn("0.3.0", result.stderr)


class PrepareWithPyTests(unittest.TestCase):
    def test_with_py_bumps_both_crate_and_wheel(self):
        with ReleaseFixtureRepo(with_py=True) as repo:
            result = run_release(
                [
                    "prepare", "0.2.0",
                    "--repo-dir", str(repo.root),
                    "--date", "2026-10-11",
                    "--with-py",
                ],
                repo.root,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            # Crate bumped as usual.
            self.assertIn('version = "0.2.0"', repo.cargo_toml_text())
            # Both wheel version fields bumped.
            self.assertIn('version = "0.2.0"', repo.py_pyproject_text())
            self.assertIn('version = "0.2.0"', repo.py_cargo_toml_text())
            # The comment line's fake version is untouched (regex skips comments).
            self.assertIn('version = "9.9.9"', repo.py_cargo_toml_text())

            # Lock carries both crates at the new version.
            lock = repo.lock_text()
            self.assertIn('name = "quadraui-py"', lock)
            # Two `version = "0.2.0"` lines (quadraui + quadraui-py).
            self.assertGreaterEqual(lock.count('version = "0.2.0"'), 2)

            # Commit names both; output points at the py tag follow-up.
            log = run_git(["log", "-1", "--format=%s"], repo.root).stdout
            self.assertIn("quadraui-py", log)
            self.assertIn("py-v0.2.0", result.stdout)

    def test_with_py_refuses_wheel_downgrade(self):
        # Crate at 0.1.0, wheel already ahead at 0.5.0: a 0.2.0 crate release
        # must not drag the wheel backwards (PyPI is append-only).
        with ReleaseFixtureRepo(with_py=True, py_version="0.5.0") as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--with-py"],
                repo.root,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("not greater", result.stderr)
            self.assertIn("quadraui-py", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")
            self.assertIn('version = "0.5.0"', repo.py_pyproject_text())

    def test_without_flag_leaves_wheel_untouched(self):
        with ReleaseFixtureRepo(with_py=True) as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--date", "2026-10-11"],
                repo.root,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('version = "0.2.0"', repo.cargo_toml_text())
            # Wheel stays at 0.1.0 — independent by default.
            self.assertIn('version = "0.1.0"', repo.py_pyproject_text())
            self.assertIn('version = "0.1.0"', repo.py_cargo_toml_text())
            self.assertNotIn("py-v", result.stdout)

    def test_with_py_refuses_when_wheel_missing(self):
        with ReleaseFixtureRepo(with_py=False) as repo:
            result = run_release(
                ["prepare", "0.2.0", "--repo-dir", str(repo.root), "--with-py"],
                repo.root,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("quadraui-py/pyproject.toml", result.stderr)
            self.assertEqual(repo.current_branch(), "develop")


class PromoteCommandTests(unittest.TestCase):
    def test_promote_prints_pr_commands_with_changelog_body(self):
        with ReleaseFixtureRepo() as repo:
            result = run_release(["promote", "--repo-dir", str(repo.root)], repo.root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("gh pr create --base main --head develop", result.stdout)
            self.assertIn("gh pr merge --merge", result.stdout)
            self.assertIn("0.1.0", result.stdout)
            self.assertIn("the first thing", result.stdout)

    def test_promote_refuses_when_no_release_section_exists(self):
        with ReleaseFixtureRepo() as repo:
            (repo.root / "CHANGELOG.md").write_text(
                "## [Unreleased]\n\n### Added\n\n- a new thing\n"
            )
            result = run_release(["promote", "--repo-dir", str(repo.root)], repo.root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("no released", result.stderr)

    def test_promote_prints_py_tag_when_wheel_untagged(self):
        with ReleaseFixtureRepo(with_py=True, py_version="0.2.0") as repo:
            result = run_release(["promote", "--repo-dir", str(repo.root)], repo.root)
            self.assertEqual(result.returncode, 0, result.stderr)
            # Crate promotion commands still printed.
            self.assertIn("gh pr create --base main --head develop", result.stdout)
            # Plus the wheel tag step, since no py-v0.2.0 tag exists yet.
            self.assertIn("git tag py-v0.2.0 origin/main", result.stdout)
            self.assertIn("git push origin py-v0.2.0", result.stdout)

    def test_promote_omits_py_tag_when_wheel_already_tagged(self):
        with ReleaseFixtureRepo(with_py=True, py_version="0.2.0") as repo:
            run_git(["tag", "py-v0.2.0"], repo.root)
            result = run_release(["promote", "--repo-dir", str(repo.root)], repo.root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("gh pr create --base main --head develop", result.stdout)
            # Wheel already released at this version — no tag step.
            self.assertNotIn("py-v0.2.0", result.stdout)

    def test_promote_no_py_tag_without_wheel(self):
        with ReleaseFixtureRepo(with_py=False) as repo:
            result = run_release(["promote", "--repo-dir", str(repo.root)], repo.root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn("py-v", result.stdout)


if __name__ == "__main__":
    unittest.main()
