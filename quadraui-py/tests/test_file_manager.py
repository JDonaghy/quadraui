"""Black-box acceptance test for `examples/file_manager.py`: a live,
cross-backend, resizable two-pane file manager -- a folders-only tree on the
left, and a details area on the right that opens one tab per folder (each tab a
data_table of that folder's contents), all built from one layout tree the
`render` callback returns. Drives the real `event -> handle -> render` path on
the headless backend -- push an event, `step()`, then assert against the
rebuilt descriptors and `screen()` / `screen_contains()`, never hardcoded
coordinates.

Built against a deterministic temp directory (not the repo), so names and
ordering are fixed:

    root/
      alpha/        a1.txt  a2.txt
      beta/         b1.txt
      readme.md

The tree pane is FOLDERS ONLY, so (depth-first, dirs only) the folder rows --
and the selection cursor indices -- are:

    0 root   1 alpha   2 beta

Files (readme.md, a1.txt, ...) never appear in the tree; they show in the
details table of a folder's tab once it is opened with Enter.

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


def _press(fmmod, fm, key):
    fm.app.push_event({"KeyPressed": {"key": key, "modifiers": _NO_MODS, "repeat": False}})
    fm.app.step()


def _active_table(fm):
    """The data_table descriptor behind the active tab, or None if the details
    pane is still the placeholder (no folder opened yet)."""
    second = fm._render()["split"]["second"]
    if "tabs" not in second:
        return None
    tabs = second["tabs"]
    content = tabs["tabs"][tabs["selected"]]["content"]
    return content["data_table"]


def _names(table):
    return [r["cells"][0]["spans"][0]["text"] for r in table["rows"]]


def test_tree_pane_is_folders_only(fmmod, tree_root):
    """The tree lists directories only; files never become tree rows."""
    fm = _manager(fmmod, tree_root)

    row_names = [r["text"]["spans"][0]["text"] for r in fm.rows]
    assert row_names == [tree_root.name, "alpha", "beta"]
    assert "readme.md" not in row_names
    assert "a1.txt" not in row_names
    # The top-level dict is a layout tree rooted at a resizable split.
    assert set(fm._render()) == {"split"}
    assert fm._render()["split"]["orientation"] == "horizontal"


def test_details_is_placeholder_until_a_folder_is_opened(fmmod, tree_root):
    """With nothing opened the right pane is a placeholder, not a table -- so
    the grid header vocabulary is absent until the first Enter."""
    fm = _manager(fmmod, tree_root)

    assert _active_table(fm) is None
    assert fm.app.screen_contains("Select a folder")
    assert not fm.app.screen_contains("Kind")  # no table header yet


def test_enter_opens_the_cursored_folder_as_a_tab(fmmod, tree_root):
    """Down onto alpha, Enter: a tab titled 'alpha' opens and its table lists
    alpha's files -- state changed in the handler, the layout followed."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)  # cursor 0 -> 1 == alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)

    assert [n.name for n in fm.state["open"]] == ["alpha"]
    assert fm.state["active_tab"] == 0
    assert _names(_active_table(fm)) == ["a1.txt", "a2.txt"]
    # The tab title (folder name) and the grid header are both on screen now.
    assert fm.app.screen_contains("alpha")
    for title in ("Name", "Size", "Kind"):
        assert fm.app.screen_contains(title)


def test_opening_a_second_folder_adds_a_tab_and_activates_it(fmmod, tree_root):
    """Opening beta after alpha appends a second tab and makes it active, so
    the details table now shows beta's child, not alpha's."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (tab 0)
    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> beta
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (tab 1, active)

    assert [n.name for n in fm.state["open"]] == ["alpha", "beta"]
    assert fm.state["active_tab"] == 1
    assert _names(_active_table(fm)) == ["b1.txt"]
    assert fm.app.screen_contains("b1.txt")
    assert not fm.app.screen_contains("a1.txt")  # alpha's tab is inactive


def test_reopening_an_open_folder_focuses_its_tab(fmmod, tree_root):
    """Enter on an already-open folder focuses its tab instead of duplicating."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha
    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> beta
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (active 1)
    assert fm.state["active_tab"] == 1

    # Go back up to alpha and Enter again: no new tab, alpha refocused.
    _press(fmmod, fm, fmmod.UP_KEY)     # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)
    assert [n.name for n in fm.state["open"]] == ["alpha", "beta"]
    assert fm.state["active_tab"] == 0
    assert _names(_active_table(fm)) == ["a1.txt", "a2.txt"]


def test_tab_activated_event_switches_the_shown_tab(fmmod, tree_root):
    """A TabBar/TabActivated event (what a tab click delivers) switches which
    tab's content paints on the next frame."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (tab 0)
    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (tab 1, active)
    assert fm.state["active_tab"] == 1

    fm.app.push_event({"TabBar": ["fm:tabs", {"TabActivated": {"index": 0}}]})
    fm.app.step()

    assert fm.state["active_tab"] == 0
    assert _names(_active_table(fm)) == ["a1.txt", "a2.txt"]
    assert fm.app.screen_contains("a1.txt")


def test_split_divider_drag_event_updates_the_ratio(fmmod, tree_root):
    """A SplitDividerDragged event (what dragging the divider delivers) writes
    the new ratio into state, which the next render reads back."""
    fm = _manager(fmmod, tree_root)

    assert fm._render()["split"]["ratio"] == pytest.approx(0.3)

    fm.app.push_event(
        {"SplitDividerDragged": {"tree": "fm:split", "split_index": 0, "new_ratio": 0.6}}
    )
    fm.app.step()

    assert fm.state["ratio"] == pytest.approx(0.6)
    assert fm._render()["split"]["ratio"] == pytest.approx(0.6)
