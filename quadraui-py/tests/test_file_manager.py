"""Black-box acceptance test for `examples/file_manager.py`: an interactive,
cross-backend, resizable two-pane file manager -- a lazily-loaded folders-only
tree on the left, and a details area on the right that opens one closable tab
per folder (each tab a data_table of that folder's contents), all built from
one layout tree the `render` callback returns. Drives the real
`event -> handle -> render` path on the headless backend -- push an event,
`step()`, then assert against the rebuilt descriptors and `screen()` /
`screen_contains()`, never hardcoded coordinates.

Built against a deterministic temp directory (not the repo), so names and
ordering are fixed:

    root/
      alpha/   sub/ (deep.txt)   a1.txt  a2.txt
      beta/    b1.txt
      readme.md

The tree pane is FOLDERS ONLY and loads one level at a time. Initially the root
is expanded and its subfolders are collapsed + unloaded, so (depth-first, dirs
only) the visible folder rows are:

    0 root   1 alpha   2 beta

alpha's own children (including the `sub/` subfolder) are not read until alpha
is expanded or opened. Files (readme.md, a1.txt, ...) never appear in the tree;
they show in a folder's details table once it is opened.

Run against a built wheel/extension, e.g.:

    maturin develop
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
    sub = alpha / "sub"
    sub.mkdir()
    (sub / "deep.txt").write_text("deep")
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


def _send(fm, event):
    fm.app.push_event(event)
    fm.app.step()


def _body_split(fm):
    """The resizable tree/details split. The frame is an outer vertical split
    (chrome+body over a status bar); the body lives in the first pane, inside
    the `sidebar_panel` that carries the toolbar."""
    outer = fm._render()["split"]
    return outer["first"]["sidebar_panel"]["content"]["split"]


def _status_bar(fm):
    return fm._render()["split"]["second"]["status_bar"]


def _active_table(fm):
    """The data_table descriptor behind the active tab, or None if the details
    pane is still the placeholder (no folder opened yet)."""
    second = _body_split(fm)["second"]
    if "tabs" not in second:
        return None
    tabs = second["tabs"]
    content = tabs["tabs"][tabs["selected"]]["content"]
    return content["data_table"]


def _names(table):
    return [r["cells"][0]["spans"][0]["text"] for r in table["rows"]]


def _folder_names(fmmod, fm):
    rows, _nodes = fmmod.folder_rows(fm.root)
    return [r["text"]["spans"][0]["text"] for r in rows]


def _segment_text(segments):
    return "".join(s["text"] for s in segments)


# --- the tree: folders-only, lazily loaded -----------------------------------


def test_tree_starts_folders_only_and_lazily_loaded(fmmod, tree_root):
    """The tree lists directories only, one level deep: the root is expanded
    onto its subfolders, which are themselves collapsed and unread."""
    fm = _manager(fmmod, tree_root)

    assert _folder_names(fmmod, fm) == [tree_root.name, "alpha", "beta"]
    # alpha's contents (and its `sub/` subfolder) are not read yet.
    alpha = fm.root.children[0]
    assert alpha.name == "alpha"
    assert alpha.children is None
    # The top-level dict is the outer vertical split; the body beneath is the
    # horizontal tree/details divider.
    assert set(fm._render()) == {"split"}
    assert fm._render()["split"]["orientation"] == "vertical"
    assert _body_split(fm)["orientation"] == "horizontal"


def test_right_expands_and_loads_on_demand(fmmod, tree_root):
    """Right on a collapsed folder reads it and reveals its subfolders; the
    tree grows without any up-front deep scan."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # cursor 0 -> alpha
    assert fm.cursor.name == "alpha"
    _press(fmmod, fm, fmmod.RIGHT_KEY)  # expand alpha (loads it)

    assert fm.cursor.children is not None          # now loaded
    assert _folder_names(fmmod, fm) == [tree_root.name, "alpha", "sub", "beta"]
    assert fm.app.screen_contains("sub")


def test_left_collapses_then_climbs_to_parent(fmmod, tree_root):
    """Left collapses an expanded folder; a second Left (now collapsed) jumps
    the cursor to the parent."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)    # -> alpha
    _press(fmmod, fm, fmmod.RIGHT_KEY)   # expand alpha
    _press(fmmod, fm, fmmod.DOWN_KEY)    # -> sub (collapsed leaf dir)
    _press(fmmod, fm, fmmod.LEFT_KEY)    # sub collapsed -> climb to parent alpha
    assert fm.cursor.name == "alpha"
    assert fm.cursor.expanded  # alpha still expanded

    _press(fmmod, fm, fmmod.LEFT_KEY)    # alpha expanded -> collapse it
    assert fm.cursor.name == "alpha"
    assert not fm.cursor.expanded
    assert _folder_names(fmmod, fm) == [tree_root.name, "alpha", "beta"]  # sub hidden

    _press(fmmod, fm, fmmod.LEFT_KEY)    # alpha collapsed -> climb to root
    assert fm.cursor.name == tree_root.name


def test_chevron_click_toggles_expansion_without_moving_the_cursor(fmmod, tree_root):
    """A `Tree(RowToggleExpand)` event -- what a chevron click resolves to --
    expands/collapses that row in place; unlike a row-body click, it leaves
    the cursor (still on the root) untouched."""
    fm = _manager(fmmod, tree_root)

    _send(fm, {"Tree": ["fm:tree", {"RowToggleExpand": {"path": [0]}}]})  # alpha
    assert fm.cursor.name == tree_root.name  # cursor never moved
    assert _folder_names(fmmod, fm) == [tree_root.name, "alpha", "sub", "beta"]

    _send(fm, {"Tree": ["fm:tree", {"RowToggleExpand": {"path": [0]}}]})  # alpha again
    assert fm.cursor.name == tree_root.name
    assert _folder_names(fmmod, fm) == [tree_root.name, "alpha", "beta"]  # sub hidden again


# --- opening folders as detail tabs ------------------------------------------


def test_details_is_placeholder_until_a_folder_is_opened(fmmod, tree_root):
    """With nothing opened the right pane is a placeholder, not a table."""
    fm = _manager(fmmod, tree_root)

    assert _active_table(fm) is None
    assert fm.app.screen_contains("Select a folder")
    assert not fm.app.screen_contains("Kind")  # no table header yet


def test_enter_opens_the_cursored_folder_as_a_tab(fmmod, tree_root):
    """Down onto alpha, Enter: a tab titled 'alpha' opens listing alpha's
    contents, subfolders first with a trailing slash."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)

    assert [n.name for n in fm.open] == ["alpha"]
    assert fm.active_tab == 0
    assert _names(_active_table(fm)) == ["sub/", "a1.txt", "a2.txt"]
    assert fm.app.screen_contains("alpha")
    for title in ("Name", "Size", "Kind"):
        assert fm.app.screen_contains(title)


def test_reopening_an_open_folder_focuses_its_tab(fmmod, tree_root):
    """Enter on an already-open folder focuses its tab instead of duplicating."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (tab 0)
    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> beta
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (tab 1, active)
    assert fm.active_tab == 1

    _press(fmmod, fm, fmmod.UP_KEY)     # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)
    assert [n.name for n in fm.open] == ["alpha", "beta"]
    assert fm.active_tab == 0


# --- drilling in from the details table --------------------------------------


def test_activating_a_subfolder_row_drills_into_it(fmmod, tree_root):
    """Activating (clicking) a subfolder row in the details table opens that
    subfolder as a new, active tab and moves the tree cursor onto it."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (row 0 is sub/)
    assert _names(_active_table(fm))[0] == "sub/"

    _send(fm, {"DataTable": ["fm:grid:0", {"RowActivated": {"idx": 0}}]})

    assert [n.name for n in fm.open] == ["alpha", "sub"]
    assert fm.active_tab == 1
    assert fm.cursor.name == "sub"
    assert _names(_active_table(fm)) == ["deep.txt"]
    assert fm.app.screen_contains("deep.txt")


def test_activating_a_file_row_reports_it_without_opening_a_tab(fmmod, tree_root):
    """Activating a file row is not navigation: it reports the file in the
    status bar and opens no tab."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha
    _send(fm, {"DataTable": ["fm:grid:0", {"RowActivated": {"idx": 1}}]})  # a1.txt

    assert [n.name for n in fm.open] == ["alpha"]  # no new tab
    assert "a1.txt" in _segment_text(_status_bar(fm)["left_segments"])


# --- tabs: click to switch, close to remove ----------------------------------


def test_tab_activated_event_switches_the_shown_tab(fmmod, tree_root):
    """A TabBar/TabActivated event (what a tab click delivers) switches which
    tab's content paints on the next frame."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (tab 0)
    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (tab 1, active)
    assert fm.active_tab == 1

    _send(fm, {"TabBar": ["fm:tabs", {"TabActivated": {"index": 0}}]})

    assert fm.active_tab == 0
    assert _names(_active_table(fm))[0] == "sub/"  # alpha's contents


def test_tab_closed_event_removes_the_tab(fmmod, tree_root):
    """A TabBar/TabClosed event (what the tab's × delivers) drops that tab and
    keeps the active index in range."""
    fm = _manager(fmmod, tree_root)

    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha (tab 0)
    _press(fmmod, fm, fmmod.DOWN_KEY)
    _press(fmmod, fm, fmmod.ENTER_KEY)  # open beta (tab 1, active)

    _send(fm, {"TabBar": ["fm:tabs", {"TabClosed": {"index": 0}}]})  # close alpha

    assert [n.name for n in fm.open] == ["beta"]
    assert fm.active_tab == 0
    assert _names(_active_table(fm)) == ["b1.txt"]


# --- the split divider --------------------------------------------------------


def test_body_divider_drag_updates_the_ratio(fmmod, tree_root):
    """A SplitDividerDragged on the body split writes the new ratio into state,
    which the next render reads back."""
    fm = _manager(fmmod, tree_root)

    assert _body_split(fm)["ratio"] == pytest.approx(0.3)
    _send(fm, {"SplitDividerDragged": {"tree": "fm:split", "split_index": 0, "new_ratio": 0.6}})

    assert fm.ratio == pytest.approx(0.6)
    assert _body_split(fm)["ratio"] == pytest.approx(0.6)


def test_outer_divider_drag_is_ignored(fmmod, tree_root):
    """Dragging the outer chrome/status divider must not resize the body."""
    fm = _manager(fmmod, tree_root)

    _send(fm, {"SplitDividerDragged": {"tree": "fm:outer", "split_index": 0, "new_ratio": 0.9}})
    assert fm.ratio == pytest.approx(0.3)
    assert _body_split(fm)["ratio"] == pytest.approx(0.3)


# --- chrome: toolbar legend + live status bar --------------------------------


def test_toolbar_band_is_present_and_rendered(fmmod, tree_root):
    """A sidebar_panel wraps the body and carries the toolbar legend in its
    reserved top slot; its action labels paint on screen."""
    fm = _manager(fmmod, tree_root)

    chrome = fm._render()["split"]["first"]["sidebar_panel"]
    assert chrome["id"] == "fm:chrome"
    assert chrome["toolbar"]["id"] == "fm:toolbar"
    labels = [b["Action"]["label"] for b in chrome["toolbar"]["buttons"]]
    assert any("Open" in label for label in labels)
    for word in ("Move", "Expand", "Collapse", "Open", "Refresh"):
        assert fm.app.screen_contains(word)


def test_status_bar_reflects_cursor_and_open_tabs(fmmod, tree_root):
    """The bottom status bar is live: it names the cursored folder, counts its
    items once loaded, and summarizes the open tabs."""
    fm = _manager(fmmod, tree_root)

    status = _status_bar(fm)
    assert status["id"] == "fm:status"
    # The root is loaded (alpha, beta, readme.md == 3), no tabs yet.
    assert tree_root.name in _segment_text(status["left_segments"])
    assert "3 items" in _segment_text(status["left_segments"])
    assert "no tabs" in _segment_text(status["right_segments"])

    _press(fmmod, fm, fmmod.DOWN_KEY)   # -> alpha (still unopened)
    assert "unopened" in _segment_text(_status_bar(fm)["left_segments"])

    _press(fmmod, fm, fmmod.ENTER_KEY)  # open alpha: sub/, a1.txt, a2.txt == 3
    status = _status_bar(fm)
    assert "alpha" in _segment_text(status["left_segments"])
    assert "3 items" in _segment_text(status["left_segments"])
    assert "tab 1/1" in _segment_text(status["right_segments"])
    assert fm.app.screen_contains("tab 1/1")


# --- the real-terminal key path ----------------------------------------------


# --- scrolling: the view follows the cursor, the scrollbar paints ------------


def _deep_root(tmp_path, n):
    """A root with `n` subfolders -- enough to overflow a short frame so the
    tree must scroll."""
    for i in range(n):
        (tmp_path / f"folder_{i:02d}").mkdir()
    return tmp_path


def test_short_tree_paints_no_scrollbar(fmmod, tree_root):
    """When every folder row fits the pane, no scrollbar track/thumb paints --
    `Backend::tree_vscrollbar` returns None and nothing is drawn over it."""
    fm = fmmod.FileManager(tree_root, width=80, height=30)  # 3 rows, 30 tall
    assert not fm.app.screen_contains("░")  # track glyph
    assert not fm.app.screen_contains("█")  # thumb glyph


def test_overflowing_tree_paints_a_scrollbar_on_tui(fmmod, tmp_path):
    """More folder rows than fit makes the headless (TUI) backend paint the
    vertical scrollbar the tree rasteriser itself doesn't draw -- the binding
    composes it from `tree_vscrollbar` geometry, so a declarative tree gets a
    scrollbar with no per-app code."""
    fm = fmmod.FileManager(_deep_root(tmp_path, 20), width=60, height=12)
    fm.app.step()
    assert fm.app.screen_contains("░")  # track
    assert fm.app.screen_contains("█")  # thumb


def test_scroll_follows_cursor_down_and_clamps(fmmod, tmp_path):
    """Arrowing past the visible window advances `scroll` so the cursor stays
    painted, and the last page clamps without scrolling past the end."""
    fm = fmmod.FileManager(_deep_root(tmp_path, 20), width=60, height=12)

    assert fm.scroll == 0
    for _ in range(15):
        _press(fmmod, fm, fmmod.DOWN_KEY)

    # The cursor advanced past the first window, so the view scrolled...
    assert fm.scroll > 0
    # ...and the cursored folder is actually on screen (not off the bottom).
    assert fm.app.screen_contains(fm.cursor.name)

    # Arrowing back to the top pulls the window back to the start.
    for _ in range(20):
        _press(fmmod, fm, fmmod.UP_KEY)
    assert fm.scroll == 0
    assert fm.cursor.name == tmp_path.name


def test_focus_scoped_key_event_moves_the_cursor(fmmod, tree_root):
    """On the real backends a focused tree's keys arrive wrapped as
    Tree/KeyPressed, not a plain KeyPressed; the handler must honor both so the
    keyboard works in an actual terminal."""
    fm = _manager(fmmod, tree_root)

    assert fm.cursor.name == tree_root.name
    # A focused tree stringifies the key ("Down"), unlike a plain KeyPressed's
    # tagged {"Named": "Down"}. Deliver that shape straight to the handler (it
    # is what the real backend hands Python after focus routing).
    fm._handle({"Tree": ["fm:tree", {"KeyPressed": {"key": "Down", "modifiers": _NO_MODS}}]})
    assert fm.cursor.name == "alpha"

    # And the plain top-level KeyPressed shape still works.
    _press(fmmod, fm, fmmod.DOWN_KEY)
    assert fm.cursor.name == "beta"
