"""Black-box acceptance test for `examples/file_manager.py`: a live,
cross-backend file manager whose grid pane follows the tree selection via the
`render` callback. Drives the real `event -> handle -> render` path on the
headless backend -- push a key, `step()`, then assert against `screen()` /
`screen_contains()` and the rebuilt descriptors, never hardcoded coordinates.

Built against a deterministic temp directory (not the repo), so the expected
names and ordering are fixed:

    root/
      alpha/        a1.txt  a2.txt
      beta/         b1.txt
      readme.md

With `MAX_DEPTH = 2` the scan pre-expands `alpha/` and `beta/`, so the flattened
tree rows -- and the selection cursor indices -- are:

    0 root   1 alpha   2 a1.txt   3 a2.txt   4 beta   5 b1.txt   6 readme.md

Run against a built wheel/extension, e.g.:

    maturin develop --features macos
    pytest quadraui-py/tests/test_file_manager.py
"""

import importlib.util
import pathlib
import sys

import pytest

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "file_manager.py"
)

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "meta": False}


def _load_file_manager():
    spec = importlib.util.spec_from_file_location("file_manager", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    # `Node` is a dataclass with `from __future__ import annotations`, so the
    # dataclasses machinery resolves its string annotations via
    # `sys.modules[module.__name__]`; register the module before executing it
    # or that lookup returns None and raises.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def fmmod():
    """Load `file_manager.py` once -- it has no mutable top-level state."""
    return _load_file_manager()


@pytest.fixture
def tree_root(tmp_path):
    """Build the fixed directory tree the docstring describes and return its
    root path."""
    alpha = tmp_path / "alpha"
    alpha.mkdir()
    (alpha / "a1.txt").write_text("one")
    (alpha / "a2.txt").write_text("two")
    beta = tmp_path / "beta"
    beta.mkdir()
    (beta / "b1.txt").write_text("bee")
    (tmp_path / "readme.md").write_text("# readme")
    return tmp_path


def _manager(fmmod, root):
    return fmmod.FileManager(root, width=80, height=30)


def _press_down(fmmod, fm):
    fm.app.push_event(
        {"KeyPressed": {"key": fmmod.DOWN_KEY, "modifiers": _NO_MODS, "repeat": False}}
    )
    fm.app.step()


def _grid(fm):
    """Extract the grid pane's (names, kinds) from the live render output."""
    table = fm._render()["data_table"]
    names = [r["cells"][0]["spans"][0]["text"] for r in table["rows"]]
    kinds = [r["cells"][2]["spans"][0]["text"] for r in table["rows"]]
    return names, kinds


def test_initial_grid_lists_root_children(fmmod, tree_root):
    """At the initial cursor (root selected), the grid lists the root folder's
    immediate children -- dirs first, then files, each name sorted."""
    fm = _manager(fmmod, tree_root)

    names, kinds = _grid(fm)
    assert names == ["alpha", "beta", "readme.md"]
    assert kinds == ["folder", "folder", "file"]

    # The grid header's column titles are painted and are grid-only vocabulary
    # (the tree pane never prints them), so finding them proves the grid is
    # actually on screen.
    for title in ("Name", "Size", "Kind"):
        assert fm.app.screen_contains(title)


def test_selecting_a_folder_updates_the_grid(fmmod, tree_root):
    """Moving the selection onto `alpha/` makes the grid list *alpha's*
    children -- the whole point of the `render` callback: state changed in the
    handler, the grid followed on the next frame."""
    fm = _manager(fmmod, tree_root)

    _press_down(fmmod, fm)  # cursor 0 -> 1 == alpha

    assert fm.state["cursor"] == 1
    assert fm._selected_node().name == "alpha"

    names, kinds = _grid(fm)
    assert names == ["a1.txt", "a2.txt"]
    assert kinds == ["file", "file"]
    # beta's child belongs to a different folder and must have dropped out.
    assert "b1.txt" not in names


def test_selecting_a_file_shows_its_containing_folder(fmmod, tree_root):
    """A file has no children to list, so selecting one shows the contents of
    the folder that contains it -- here, selecting `a1.txt` keeps the grid on
    `alpha/`'s children."""
    fm = _manager(fmmod, tree_root)

    _press_down(fmmod, fm)  # -> alpha
    _press_down(fmmod, fm)  # -> a1.txt (a file inside alpha)

    assert fm._selected_node().name == "a1.txt"
    names, _kinds = _grid(fm)
    assert names == ["a1.txt", "a2.txt"]


def test_screen_changes_end_to_end_when_selection_moves(fmmod, tree_root):
    """End-to-end through the painted screen (no descriptor peeking): the root
    grid lists folders, so the grid-only word "folder" is on screen; once the
    selection moves to `alpha/` (whose children are all files) that word is
    gone and the whole screen differs -- proving the repaint happened through
    the real step/render path."""
    fm = _manager(fmmod, tree_root)

    root_screen = fm.app.screen()
    assert fm.app.screen_contains("folder")  # alpha + beta listed as folders

    _press_down(fmmod, fm)  # -> alpha

    alpha_screen = fm.app.screen()
    assert alpha_screen != root_screen
    # alpha's children are both files, so no grid row carries kind "folder".
    assert not fm.app.screen_contains("folder")
