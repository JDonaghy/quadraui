"""Black-box acceptance test for `examples/file_manager_app.py`: an interactive,
cross-backend, resizable two-pane file manager -- a lazily-loaded folders-only
tree on the left, and a details area on the right that opens one closable tab
per folder (each tab a data_table of that folder's contents), all built from
one `quadraui.app` node tree the `view` callback returns.

Written entirely against `quadraui.testing` -- the app-author test harness --
so every line below reads the way a user describes the app: find the tree,
click the row labelled "alpha", press Enter, assert the frame shows
"a1.txt". There is no raw wire event, no hand-built key dict and no
hardcoded coordinate anywhere in this file; the harness synthesises the real
`UiEvent` for each action and drives the real
`event -> handle -> render` path on the headless backend.

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
is expanded or opened -- which is exactly why "sub" is absent from the tree
below until something expands alpha. Files (readme.md, a1.txt, ...) never
appear in the tree; they show in a folder's details table once it is opened.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_file_manager_app.py
"""

import importlib.util
import pathlib
import sys

import pytest

from quadraui.testing import harness

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "file_manager_app.py"
)


def _load_file_manager():
    spec = importlib.util.spec_from_file_location("file_manager_app", _EXAMPLE_PATH)
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
    """Load `file_manager_app.py` once -- it has no mutable top-level state."""
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


@pytest.fixture
def ui(fmmod, tree_root):
    """The file manager over the fixture tree, wrapped in the harness."""
    return harness(fmmod.FileManager(tree_root, width=80, height=30).app)


def _deep_root(tmp_path, n):
    """A root with `n` subfolders -- enough to overflow a short frame so the
    tree must scroll."""
    for i in range(n):
        (tmp_path / f"folder_{i:02d}").mkdir()
    return tmp_path


def _tree(ui):
    return ui.find(role="tree")


def _cursor(ui):
    """The folder the tree renders as selected -- the app's own cursor, read
    back off the frame rather than out of the app object."""
    return _tree(ui).selected_row.text


def _folder_names(ui):
    return _tree(ui).row_texts


def _open_titles(ui):
    """The title of every open details tab, left to right."""
    tabs = ui.query(role="tabs")
    return [] if tabs is None else [tab.title for tab in tabs.tabs]


def _details_names(ui):
    """The Name column of the details table behind the active tab, or `None`
    when the details pane is still the placeholder."""
    table = ui.query(role="data_table")
    return None if table is None else [row.cells[0] for row in table.rows]


def _status(ui):
    return ui.find(role="status_bar")


# --- the tree: folders-only, lazily loaded -----------------------------------


def test_tree_starts_folders_only_and_lazily_loaded(ui, tree_root):
    """The tree lists directories only, one level deep: the root is expanded
    onto its subfolders, which are themselves collapsed and unread -- so
    alpha's own `sub/` is nowhere on the frame yet."""
    assert _folder_names(ui) == [tree_root.name, "alpha", "beta"]
    ui.assert_not_text("sub")
    # No file ever reaches the tree, at any depth.
    ui.assert_not_text("readme.md", "a1.txt", "deep.txt")
    # The frame is an outer vertical split (chrome + body over a status bar)
    # whose body is the horizontal tree/details divider.
    assert ui.root.role == "split"
    assert ui.root["orientation"] == "vertical"
    assert ui.find(id="fm:split")["orientation"] == "horizontal"


def test_right_expands_and_loads_on_demand(ui, tree_root):
    """Right on a collapsed folder reads it and reveals its subfolders; the
    tree grows without any up-front deep scan."""
    ui.press("Down")  # cursor root -> alpha
    assert _cursor(ui) == "alpha"
    ui.press("Right")  # expand alpha (loads it)

    assert _tree(ui).row("alpha").expanded
    assert _folder_names(ui) == [tree_root.name, "alpha", "sub", "beta"]
    ui.assert_text("sub")


def test_left_collapses_then_climbs_to_parent(ui, tree_root):
    """Left collapses an expanded folder; a second Left (now collapsed) jumps
    the cursor to the parent."""
    ui.press("Down")   # -> alpha
    ui.press("Right")  # expand alpha
    ui.press("Down")   # -> sub (collapsed leaf dir)
    ui.press("Left")   # sub collapsed -> climb to parent alpha
    assert _cursor(ui) == "alpha"
    assert _tree(ui).row("alpha").expanded

    ui.press("Left")   # alpha expanded -> collapse it
    assert _cursor(ui) == "alpha"
    assert not _tree(ui).row("alpha").expanded
    assert _folder_names(ui) == [tree_root.name, "alpha", "beta"]  # sub hidden

    ui.press("Left")   # alpha collapsed -> climb to root
    assert _cursor(ui) == tree_root.name


def test_chevron_click_toggles_expansion_without_moving_the_cursor(ui, tree_root):
    """Clicking a row's expand chevron expands/collapses that row in place;
    unlike a row-body click, it leaves the cursor (still on the root)
    untouched."""
    _tree(ui).row("alpha").toggle()
    assert _cursor(ui) == tree_root.name  # cursor never moved
    assert _folder_names(ui) == [tree_root.name, "alpha", "sub", "beta"]

    _tree(ui).row("alpha").toggle()
    assert _cursor(ui) == tree_root.name
    assert _folder_names(ui) == [tree_root.name, "alpha", "beta"]  # sub hidden again


def test_row_body_click_moves_the_cursor(ui):
    """A click on the row body (not its chevron) moves the cursor onto that
    folder without expanding it."""
    _tree(ui).row("beta").click()

    assert _cursor(ui) == "beta"
    assert _folder_names(ui)[-1] == "beta"  # nothing expanded


# --- opening folders as detail tabs ------------------------------------------


def test_details_is_placeholder_until_a_folder_is_opened(ui):
    """With nothing opened the right pane is a placeholder, not a table."""
    assert _details_names(ui) is None
    ui.assert_text("Select a folder")
    ui.assert_not_text("Kind")  # no table header yet


def test_enter_opens_the_cursored_folder_as_a_tab(ui):
    """Down onto alpha, Enter: a tab titled 'alpha' opens listing alpha's
    contents, subfolders first with a trailing slash."""
    ui.press("Down")   # -> alpha
    ui.press("Enter")

    assert _open_titles(ui) == ["alpha"]
    assert ui.find(role="tabs").active_index == 0
    assert _details_names(ui) == ["sub/", "a1.txt", "a2.txt"]
    ui.assert_text("alpha", "Name", "Size", "Kind")


def test_reopening_an_open_folder_focuses_its_tab(ui):
    """Enter on an already-open folder focuses its tab instead of duplicating."""
    ui.press("Down")   # -> alpha
    ui.press("Enter")  # open alpha (tab 0)
    ui.press("Down")   # -> beta
    ui.press("Enter")  # open beta (tab 1, active)
    assert ui.find(role="tabs").active_index == 1

    ui.press("Up")     # -> alpha
    ui.press("Enter")
    assert _open_titles(ui) == ["alpha", "beta"]
    assert ui.find(role="tabs").active_index == 0


# --- drilling in from the details table --------------------------------------


def test_activating_a_subfolder_row_drills_into_it(ui):
    """Double-clicking a subfolder row in the details table opens that
    subfolder as a new, active tab and moves the tree cursor onto it. A
    single click must not: it only selects, never opens."""
    ui.press("Down")   # -> alpha
    ui.press("Enter")  # open alpha (row 0 is sub/)
    assert _details_names(ui)[0] == "sub/"

    ui.find(role="data_table").row("sub/").click()
    assert _open_titles(ui) == ["alpha"], "a single click must not drill in"

    ui.find(role="data_table").row("sub/").double_click()

    assert _open_titles(ui) == ["alpha", "sub"]
    assert ui.find(role="tabs").active_index == 1
    assert _cursor(ui) == "sub"
    assert _details_names(ui) == ["deep.txt"]
    ui.assert_text("deep.txt")


def test_activating_a_file_row_reports_it_without_opening_a_tab(ui):
    """Double-clicking a file row is not navigation: it reports the file in
    the status bar and opens no tab."""
    ui.press("Down")   # -> alpha
    ui.press("Enter")  # open alpha
    ui.find(role="data_table").row("a1.txt").double_click()

    assert _open_titles(ui) == ["alpha"]  # no new tab
    assert "a1.txt" in _status(ui).text_of("left_segments")


# --- tabs: click to switch, close to remove ----------------------------------


def test_clicking_a_tab_switches_the_shown_tab(ui):
    """Clicking a tab header switches which tab's content paints on the next
    frame -- and the inactive tab's table stops being on screen at all."""
    ui.press("Down")
    ui.press("Enter")  # open alpha (tab 0)
    ui.press("Down")
    ui.press("Enter")  # open beta (tab 1, active)
    assert ui.find(role="tabs").active_index == 1

    ui.find(role="tabs").tabs[0].click()

    assert ui.find(role="tabs").active_index == 0
    assert _details_names(ui)[0] == "sub/"  # alpha's contents
    # Both tables are in the tree; only the active tab's is visible.
    assert len(ui.find_all(role="data_table")) == 1
    assert len(ui.find_all(role="data_table", include_hidden=True)) == 2


def test_closing_a_tab_removes_it(ui):
    """A tab's close affordance drops that tab and keeps the active index in
    range."""
    ui.press("Down")
    ui.press("Enter")  # open alpha (tab 0)
    ui.press("Down")
    ui.press("Enter")  # open beta (tab 1, active)

    ui.find(role="tabs").tabs[0].close()  # close alpha

    assert _open_titles(ui) == ["beta"]
    assert ui.find(role="tabs").active_index == 0
    assert _details_names(ui) == ["b1.txt"]


# --- the split divider --------------------------------------------------------


def test_body_divider_drag_updates_the_ratio(ui, fmmod):
    """Dragging the body split's divider persists the new ratio in the
    engine's own widget-state store -- with no app code tracking it at all:
    the app keeps re-declaring the same starting literal
    (`BODY_SPLIT_RATIO`) before and after the drag, and the store alone is
    what the next real render actually paints."""
    split = ui.find(id="fm:split")
    assert split.ratio == pytest.approx(fmmod.BODY_SPLIT_RATIO)

    split.drag_divider(0.6)

    split = ui.find(id="fm:split")
    assert split.ratio == pytest.approx(0.6)
    assert split["ratio"] == pytest.approx(fmmod.BODY_SPLIT_RATIO)


def test_outer_divider_drag_persists_but_never_touches_the_body_split(ui, fmmod):
    """The outer chrome/status divider resolves exactly the same uncontrolled
    way the body split does -- it is not a fixed divider, so a drag on it
    persists its own ratio in the store. What it must never do is move the
    body split's own ratio: the two ids are tracked independently."""
    ui.find(id="fm:outer").drag_divider(0.9)

    assert ui.find(id="fm:outer").ratio == pytest.approx(0.9)
    assert ui.find(id="fm:split").ratio == pytest.approx(fmmod.BODY_SPLIT_RATIO)


# --- chrome: toolbar legend + live status bar --------------------------------


def test_toolbar_band_is_present_and_rendered(ui):
    """A sidebar_panel wraps the body and carries the toolbar legend in its
    reserved top slot; its action labels paint on screen."""
    chrome = ui.find(role="sidebar_panel")
    assert chrome.id == "fm:chrome"

    toolbar = chrome.find(role="toolbar")
    assert toolbar.id == "fm:toolbar"
    assert "Open" in toolbar.button("Open").label
    ui.assert_text("Move", "Expand", "Collapse", "Open", "Refresh")


def test_status_bar_reflects_cursor_and_open_tabs(ui, tree_root):
    """The bottom status bar is live: it names the cursored folder, counts its
    items once loaded, and summarizes the open tabs."""
    status = _status(ui)
    assert status.id == "fm:status"
    # The root is loaded (alpha, beta, readme.md == 3), no tabs yet.
    assert tree_root.name in status.text_of("left_segments")
    assert "3 items" in status.text_of("left_segments")
    assert "no tabs" in status.text_of("right_segments")

    ui.press("Down")  # -> alpha (still unopened)
    assert "unopened" in _status(ui).text_of("left_segments")

    ui.press("Enter")  # open alpha: sub/, a1.txt, a2.txt == 3
    status = _status(ui)
    assert "alpha" in status.text_of("left_segments")
    assert "3 items" in status.text_of("left_segments")
    assert "1 tab" in status.text_of("right_segments")
    ui.assert_text("1 tab")


# --- scrolling: the view follows the cursor, the scrollbar paints ------------


def test_short_tree_paints_no_scrollbar(ui):
    """When every folder row fits the pane, no scrollbar track/thumb paints --
    `Backend::tree_vscrollbar` returns None and nothing is drawn over it."""
    ui.assert_not_text("░", "█")  # track glyph, thumb glyph


def test_overflowing_tree_paints_a_scrollbar_on_tui(fmmod, tmp_path):
    """More folder rows than fit makes the headless (TUI) backend paint the
    vertical scrollbar the tree rasteriser itself doesn't draw -- the binding
    composes it from `tree_vscrollbar` geometry, so a declarative tree gets a
    scrollbar with no per-app code."""
    ui = harness(fmmod.FileManager(_deep_root(tmp_path, 20), width=60, height=12).app)

    ui.assert_text("░", "█")  # track, thumb


def test_scroll_follows_cursor_down_and_clamps(fmmod, tmp_path):
    """Arrowing past the visible window advances the engine's own stored
    scroll offset so the cursor stays painted, and the last page clamps
    without scrolling past the end -- with no `scroll`/keep-visible code in
    the app itself."""
    ui = harness(fmmod.FileManager(_deep_root(tmp_path, 20), width=60, height=12).app)

    assert _tree(ui).scroll_offset == 0
    for _ in range(15):
        ui.press("Down")

    # The cursor advanced past the first window, so the view scrolled...
    assert _tree(ui).scroll_offset > 0
    # ...and the cursored folder is actually on screen (not off the bottom).
    ui.assert_text(_cursor(ui))

    # Arrowing back to the top pulls the window back to the start.
    for _ in range(20):
        ui.press("Up")
    assert _tree(ui).scroll_offset == 0
    assert _cursor(ui) == tmp_path.name


def test_focus_scoped_key_event_moves_the_cursor(ui, tree_root):
    """On the real backends a focused tree's keys arrive scoped to that
    widget, not as a plain top-level key; the handler must honor both so the
    keyboard works in an actual terminal."""
    assert _cursor(ui) == tree_root.name

    _tree(ui).press("Down")  # the shape a focused tree receives
    assert _cursor(ui) == "alpha"

    ui.press("Down")  # the plain top-level shape
    assert _cursor(ui) == "beta"
