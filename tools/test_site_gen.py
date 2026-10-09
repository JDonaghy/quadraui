#!/usr/bin/env python3
"""Tests for `tools/site_gen.py` (#1349).

Stdlib-only (`unittest`), same reasoning as `test_example_coverage.py` and
`test_comment_history_lint.py`: no pytest dependency just to cover one
script. Run with:

    python3 tools/test_site_gen.py
"""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import site_gen  # noqa: E402


FIXTURE_REGISTRY = [
    {
        "demo": "Toast",
        "group": "Overlays",
        "variants": ["default"],
        "source": "pub struct ToastDemo;\nimpl ToastDemo {}\n",
    },
    {
        "demo": "Activity Bar",
        "group": "Chrome",
        "variants": ["Accent line", "Row fill (VS Code)"],
        "source": "pub struct ActivityBarDemo;\n",
    },
    {
        "demo": "Markdown",
        "group": "Content",
        "variants": ["Popup"],
        # Mirrors `quadraui-gallery/src/demos/markdown.rs`'s `DOC`
        # constant: its own `source` region embeds a column-0 ```rust
        # fence, which must not be able to close the page's wrapping
        # fence early.
        "source": 'const DOC: &str = "\\\n# Heading\n```rust\nfn main() {}\n```";\n',
    },
]


def write_fixture_capture(capture_dir: Path, image_name: str = "tui__overlays__toast__default.svg") -> None:
    capture_dir.mkdir(parents=True, exist_ok=True)
    (capture_dir / image_name).write_text("<svg></svg>")
    manifest = [
        {
            "demo": "Toast",
            "group": "Overlays",
            "variant": "default",
            "backend": "tui",
            "path": image_name,
        },
        {
            "demo": "Toast",
            "group": "Overlays",
            "variant": "default",
            "backend": "gtk",
            "status": "capture-pending",
            "note": "no headless GTK capture path exists yet",
        },
        {
            "demo": "Toast",
            "group": "Overlays",
            "variant": "default",
            "backend": "macos",
            "status": "unsupported",
            "note": "quadraui-gallery was built without the macos feature",
        },
        {
            "demo": "Toast",
            "group": "Overlays",
            "variant": "default",
            "backend": "win",
            "status": "capture-pending",
            "note": "no headless Windows gallery driver exists yet",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Accent line",
            "backend": "tui",
            "status": "error",
            "note": "label not found at capture size",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Accent line",
            "backend": "gtk",
            "status": "capture-pending",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Accent line",
            "backend": "macos",
            "status": "unsupported",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Accent line",
            "backend": "win",
            "status": "capture-pending",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Row fill (VS Code)",
            "backend": "tui",
            "status": "capture-pending",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Row fill (VS Code)",
            "backend": "gtk",
            "status": "capture-pending",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Row fill (VS Code)",
            "backend": "macos",
            "status": "unsupported",
        },
        {
            "demo": "Activity Bar",
            "group": "Chrome",
            "variant": "Row fill (VS Code)",
            "backend": "win",
            "status": "capture-pending",
        },
    ]
    (capture_dir / "manifest.json").write_text(json.dumps(manifest))


class SlugTest(unittest.TestCase):
    def test_lowercases_and_collapses_separators(self):
        self.assertEqual(site_gen.slug("Activity Bar"), "activity-bar")
        self.assertEqual(site_gen.slug("Row fill (VS Code)"), "row-fill-vs-code")

    def test_empty_falls_back_to_demo(self):
        self.assertEqual(site_gen.slug("---"), "demo")
        self.assertEqual(site_gen.slug(""), "demo")


class CodeFenceTest(unittest.TestCase):
    def test_plain_source_gets_the_minimum_three_backtick_fence(self):
        self.assertEqual(site_gen.code_fence("pub struct Foo;\n"), "```")

    def test_source_with_a_nested_triple_backtick_fence_gets_a_longer_one(self):
        # Mirrors `quadraui-gallery/src/demos/markdown.rs`'s `DOC`
        # constant: a demo's own source can embed a column-0 ```rust
        # fence (e.g. a markdown-adapter sample), which must not be able
        # to close our wrapping fence early.
        source = 'const DOC: &str = "\\\n```rust\nfn main() {}\n```";\n'
        fence = site_gen.code_fence(source)
        self.assertGreater(len(fence), 3)
        self.assertTrue(set(fence) == {"`"})

    def test_source_with_a_longer_nested_fence_still_gets_a_strictly_longer_one(self):
        source = "`````rust\ncode\n`````"
        fence = site_gen.code_fence(source)
        self.assertGreater(len(fence), 5)


class TableCellTextTest(unittest.TestCase):
    def test_collapses_newlines_and_escapes_pipes(self):
        self.assertEqual(
            site_gen.table_cell_text("line one\nline two | still one cell"),
            "line one line two \\| still one cell",
        )


class MergeManifestsTest(unittest.TestCase):
    def test_path_row_wins_over_status_only_row_for_same_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            tui_dir = Path(tmp) / "tui-capture"
            macos_dir = Path(tmp) / "macos-capture"
            write_fixture_capture(tui_dir)
            # A second capture dir that *does* have a real macOS image for
            # the same (demo, group, variant, backend) key the first
            # capture marked "unsupported".
            macos_dir.mkdir()
            (macos_dir / "macos__overlays__toast__default.png").write_text("fake-png")
            macos_manifest = [
                {
                    "demo": "Toast",
                    "group": "Overlays",
                    "variant": "default",
                    "backend": "macos",
                    "path": "macos__overlays__toast__default.png",
                }
            ]
            (macos_dir / "manifest.json").write_text(json.dumps(macos_manifest))

            merged = site_gen.merge_manifests([tui_dir, macos_dir])
            row = merged[("Toast", "Overlays", "default", "macos")]
            self.assertEqual(row["path"], "macos__overlays__toast__default.png")


class GenerateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

        self.registry_path = self.root / "registry.json"
        self.registry_path.write_text(json.dumps(FIXTURE_REGISTRY))

        self.capture_dir = self.root / "capture"
        write_fixture_capture(self.capture_dir)

        self.out_dir = self.root / "site-src"
        site_gen.generate(self.registry_path, [self.capture_dir], self.out_dir)

    def test_one_page_per_demo(self):
        for entry in FIXTURE_REGISTRY:
            page = self.out_dir / "gallery" / f"{site_gen.slug(entry['demo'])}.md"
            self.assertTrue(page.exists(), f"missing page for {entry['demo']}: {page}")

    def test_code_region_is_present_verbatim(self):
        for entry in FIXTURE_REGISTRY:
            page = self.out_dir / "gallery" / f"{site_gen.slug(entry['demo'])}.md"
            text = page.read_text()
            self.assertIn(entry["source"], text)

    def test_pending_and_unsupported_cells_are_labelled(self):
        page = (self.out_dir / "gallery" / "activity-bar.md").read_text()
        self.assertIn("Pending", page)
        self.assertIn("Not captured in this build", page)
        self.assertIn("Error: label not found at capture size", page)

    def test_markdown_demo_fence_is_longer_than_its_own_nested_fence(self):
        # The regression this guards: a demo (like the real
        # `quadraui-gallery` `Markdown` demo) whose own `source` embeds a
        # 3-backtick fence must get a wrapping fence *longer* than 3, or
        # that nested fence (or its own close) could terminate the page's
        # code block early and reflow the rest of the source as markdown
        # prose instead of code.
        page = (self.out_dir / "gallery" / "markdown.md").read_text()
        markdown_entry = next(e for e in FIXTURE_REGISTRY if e["demo"] == "Markdown")
        lines = page.splitlines()
        open_idx = next(i for i, line in enumerate(lines) if line.startswith("`"))
        self.assertEqual(lines[open_idx], "````rust,noplayground")
        close_idx = lines.index("````", open_idx + 1)
        body = "\n".join(lines[open_idx + 1 : close_idx])
        self.assertEqual(body, markdown_entry["source"])

    def test_captured_image_is_embedded_and_copied(self):
        page = (self.out_dir / "gallery" / "toast.md").read_text()
        self.assertIn("images/tui__overlays__toast__default.svg", page)
        copied = self.out_dir / "gallery" / "images" / "tui__overlays__toast__default.svg"
        self.assertTrue(copied.exists())

    def test_gallery_index_links_every_demo(self):
        index = (self.out_dir / "gallery" / "index.md").read_text()
        for entry in FIXTURE_REGISTRY:
            self.assertIn(f"{site_gen.slug(entry['demo'])}.md", index)

    def test_summary_lists_every_demo_under_its_group(self):
        summary = (self.out_dir / "SUMMARY.md").read_text()
        self.assertIn("[Introduction](introduction.md)", summary)
        self.assertIn("[Getting Started](getting-started.md)", summary)
        self.assertIn("# Chrome", summary)
        self.assertIn("# Overlays", summary)
        self.assertIn("[Toast](gallery/toast.md)", summary)
        self.assertIn("[Activity Bar](gallery/activity-bar.md)", summary)

    def test_rerunning_generate_cleans_up_stale_pages(self):
        # A demo removed from the registry between two runs must not
        # leave behind a stale page from the previous run.
        trimmed_registry = [FIXTURE_REGISTRY[0]]
        trimmed_path = self.root / "registry-trimmed.json"
        trimmed_path.write_text(json.dumps(trimmed_registry))

        site_gen.generate(trimmed_path, [self.capture_dir], self.out_dir)

        self.assertTrue((self.out_dir / "gallery" / "toast.md").exists())
        self.assertFalse((self.out_dir / "gallery" / "activity-bar.md").exists())

    def test_error_note_with_a_pipe_and_newline_does_not_corrupt_the_table(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            registry_path = root / "registry.json"
            registry_path.write_text(
                json.dumps(
                    [
                        {
                            "demo": "Toast",
                            "group": "Overlays",
                            "variants": ["default"],
                            "source": "pub struct ToastDemo;\n",
                        }
                    ]
                )
            )
            capture_dir = root / "capture"
            capture_dir.mkdir()
            (capture_dir / "manifest.json").write_text(
                json.dumps(
                    [
                        {
                            "demo": "Toast",
                            "group": "Overlays",
                            "variant": "default",
                            "backend": "tui",
                            "status": "error",
                            "note": "panic at row 1 | col 2\nsecond line",
                        }
                    ]
                )
            )
            out_dir = root / "site-src"
            site_gen.generate(registry_path, [capture_dir], out_dir)

            page = (out_dir / "gallery" / "toast.md").read_text()
            row_line = next(line for line in page.splitlines() if line.startswith("| default"))
            # Exactly 6 unescaped `|`s: the table's own 5 column
            # separators (leading, 4 internal, trailing) — none
            # contributed by the note's own `\|` or its collapsed
            # newline.
            self.assertEqual(row_line.count("|") - row_line.count("\\|"), 6)
            self.assertIn("panic at row 1 \\| col 2 second line", row_line)


class SlugCollisionTest(unittest.TestCase):
    def test_two_demo_names_sharing_a_slug_raise_instead_of_overwriting(self):
        registry = [
            {
                "demo": "Foo Bar",
                "group": "Chrome",
                "variants": ["default"],
                "source": "a\n",
            },
            {
                "demo": "foo-bar",
                "group": "Chrome",
                "variants": ["default"],
                "source": "b\n",
            },
        ]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            registry_path = root / "registry.json"
            registry_path.write_text(json.dumps(registry))
            capture_dir = root / "capture"
            capture_dir.mkdir()
            (capture_dir / "manifest.json").write_text("[]")

            with self.assertRaises(ValueError):
                site_gen.generate(registry_path, [capture_dir], root / "site-src")


class ReadJsonArrayTest(unittest.TestCase):
    def test_missing_file_raises_a_message_naming_the_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "does-not-exist.json"
            with self.assertRaises(FileNotFoundError) as ctx:
                site_gen.read_json_array(missing, "registry.json from --dump-registry")
            self.assertIn(str(missing), str(ctx.exception))

    def test_non_array_raises_a_message_naming_the_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "registry.json"
            path.write_text(json.dumps({"not": "a list"}))
            with self.assertRaises(ValueError) as ctx:
                site_gen.read_json_array(path, "registry.json from --dump-registry")
            self.assertIn(str(path), str(ctx.exception))


class CopyImagesTest(unittest.TestCase):
    def test_path_escaping_the_capture_dir_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            capture_dir = root / "capture"
            capture_dir.mkdir()
            outside = root / "outside.svg"
            outside.write_text("<svg></svg>")

            merged = {
                ("Toast", "Overlays", "default", "tui"): {
                    "path": "../outside.svg",
                    "_capture_dir": capture_dir,
                }
            }
            with self.assertRaises(ValueError):
                site_gen.copy_images(merged, root / "images-out")


if __name__ == "__main__":
    unittest.main()
