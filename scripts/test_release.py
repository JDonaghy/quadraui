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
