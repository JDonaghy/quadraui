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

    def __init__(self, unreleased_body: str = "### Added\n\n- a new thing"):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "quadraui" / "src").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["quadraui"]\nresolver = "2"\n'
        )
        (self.root / "quadraui" / "Cargo.toml").write_text(
            '[package]\nname = "quadraui"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.root / "quadraui" / "src" / "lib.rs").write_text("pub fn hello() {}\n")
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


if __name__ == "__main__":
    unittest.main()
