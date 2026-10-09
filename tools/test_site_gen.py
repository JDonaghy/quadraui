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
        self.assertIn("Unsupported", page)
        self.assertIn("Error: label not found at capture size", page)

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


if __name__ == "__main__":
    unittest.main()
