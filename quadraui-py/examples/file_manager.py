"""Cross-backend file manager: a folder tree over a contents grid, built from
one set of `quadraui` descriptors that run unchanged on TUI, GTK, macOS, and
Windows -- only the final run call differs per platform.

The UI is *live* via the `render` callback: a plain Python `state` dict holds
what changes (the selection cursor), the event handler mutates it on Up/Down,
and `render()` rebuilds the `tree` + `data_table` descriptors from it every
frame. Neither callback ever touches `QuiApp` -- the handler only mutates
`state`, the render callback only reads it and returns descriptors -- which is
how the binding avoids the reentrant borrow that a mid-loop `insert_*` would
hit (the `render(state)` seam from the bindings, data flowing *out* of Python).

The folder tree is still pre-expanded at build time down to `MAX_DEPTH`;
expand-on-demand (re-scan when a collapsed folder is opened) is a later step.

    python quadraui-py/examples/file_manager.py [PATH]            # headless demo
    python quadraui-py/examples/file_manager.py [PATH] --tui      # real terminal
    python quadraui-py/examples/file_manager.py [PATH] --native   # OS window
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path

from quadraui import QuiApp

# Root is depth 0. Directories shallower than this are opened and their
# children included in the model; a directory at exactly this depth is kept as
# a collapsed branch (we know it is a folder but do not load its children).
MAX_DEPTH = 2


@dataclass
class Node:
    """One filesystem entry in the scanned model.

    `children is None` means "not loaded" -- either a file, or a directory we
    chose not to open (one sitting at `MAX_DEPTH`). `expanded` is True only for
    directories we actually recursed into, so it lines up with the tree's
    chevron state later.
    """

    name: str
    path: Path
    is_dir: bool
    size_bytes: int | None = None
    children: list["Node"] | None = None
    expanded: bool = False


def _sort_key(node: Node) -> tuple[bool, str]:
    """Directories first, then case-insensitive name."""
    return (not node.is_dir, node.name.lower())


def scan(path: Path, depth: int = 0) -> Node:
    """Scan `path` into a `Node`, recursing into directories until `MAX_DEPTH`.

    Dotfiles are skipped. A directory at `MAX_DEPTH` (or any unreadable one)
    is returned as a collapsed branch with `children=None`.
    """
    is_dir = path.is_dir()
    size = None if is_dir else _safe_size(path)
    node = Node(name=path.name or str(path), path=path, is_dir=is_dir, size_bytes=size)

    if is_dir and depth < MAX_DEPTH:
        try:
            entries = [child for child in path.iterdir() if not child.name.startswith(".")]
        except OSError:
            # Permission denied / vanished: leave it a collapsed branch.
            return node
        kids = [scan(child, depth + 1) for child in entries]
        kids.sort(key=_sort_key)
        node.children = kids
        node.expanded = True

    return node


def _safe_size(path: Path) -> int | None:
    try:
        return path.stat().st_size
    except OSError:
        return None


@dataclass
class Model:
    """The scanned tree. `root.children` is the top folder's contents, which
    the grid pane will list; the whole `root` feeds the tree pane."""

    root: Node = field(default=None)  # type: ignore[assignment]


def build_model(root_path: Path) -> Model:
    return Model(root=scan(root_path))


# --- Step 2: model -> tree descriptor ---------------------------------------
#
# `insert_tree` wants a *flat* list of rows, not our recursive `Node`, so we
# walk the model depth-first in display order and tag each row with its `path`
# -- the route from the root as a list of child indices (a `TreePath`, which
# the Rust side reads as `Vec<u16>`). The backend uses `path` to know which row
# a click/selection landed on and to nest rows under their parent; `indent` is
# the matching visual depth. Only directories we actually recursed into carry
# branches (folders) carry `is_expanded=Some(bool)`; files are leaves and carry
# `is_expanded=None`, so only folders draw a chevron.

TREE_STYLE = {
    "indent": 2,  # columns of indent per depth level
    "show_chevrons": True,  # draw the twisty on directories
    "chevron_expanded": "▾",  # down triangle
    "chevron_collapsed": "▸",  # right triangle
    "row_height": 22,  # px; TUI ignores it, native backends honour it
}

# `icon` is an `Icon`: a Nerd Font `glyph` plus an ASCII/basic-Unicode
# `fallback` the backend uses when the glyph can't render (so the TUI stays
# legible without a Nerd Font). `color: None` lets the row's theme foreground
# through.
FOLDER_ICON = {"glyph": "\U0001f4c1", "fallback": "/", "color": None}  # folder
FILE_ICON = {"glyph": "\U0001f4c4", "fallback": " ", "color": None}  # page


def _styled_text(text: str) -> dict:
    """One `StyledText` with a single uncolored span -- the shape both tree
    `text` and data-table cells use; the backend theme picks the colors."""
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _flatten(
    node: Node,
    path: list[int],
    indent: int,
    rows: list[dict],
    nodes: list[Node],
) -> None:
    """Append `node` as one tree row (and the `Node` behind it), then recurse
    into its loaded children. `rows` and `nodes` stay index-aligned."""
    rows.append(
        {
            "path": path,
            "indent": indent,
            "icon": FOLDER_ICON if node.is_dir else FILE_ICON,
            "text": _styled_text(node.name),
            "badge": None,  # reserved: child count / size could land here
            # Folders are branches (Some(expanded)); files are leaves (None).
            "is_expanded": node.expanded if node.is_dir else None,
        }
    )
    nodes.append(node)
    for i, child in enumerate(node.children or ()):
        _flatten(child, path + [i], indent + 1, rows, nodes)


def flatten(model: Model) -> tuple[list[dict], list[Node], dict[tuple[int, ...], Node]]:
    """Walk the model once into index-aligned `(rows, nodes)` plus a
    `path -> Node` map. The snapshot is static, so callers flatten once and
    reuse these across every frame, only varying `selected_path`."""
    rows: list[dict] = []
    nodes: list[Node] = []
    _flatten(model.root, path=[], indent=0, rows=rows, nodes=nodes)
    by_path = {tuple(r["path"]): n for r, n in zip(rows, nodes)}
    return rows, nodes, by_path


def tree_from_rows(rows: list[dict], *, selected_path: list[int] | None = None) -> dict:
    """Wrap pre-flattened `rows` into an `insert_tree` descriptor, varying only
    the selection -- the live render path's hot loop."""
    return {
        "id": "fm:tree",
        "rows": rows,
        "selection_mode": "Single",
        "selected_path": selected_path if selected_path is not None else [],
        "scroll_offset": 0,
        "has_focus": True,
        "style": TREE_STYLE,
    }


def tree_descriptor(model: Model, *, selected_path: list[int] | None = None) -> dict:
    """Flatten `model` and build its tree descriptor in one call (used by the
    step-by-step dumps; the live app flattens once and calls `tree_from_rows`)."""
    rows, _nodes, _by_path = flatten(model)
    return tree_from_rows(rows, selected_path=selected_path)


# --- Step 3: model -> data_table descriptor ---------------------------------
#
# The grid lists the immediate children of ONE folder (the scanned root, in
# this static snapshot). `insert_data_table` wants `columns` + `rows`, where a
# row's `cells` are `StyledText` -- the same `{"spans":[{"text","fg","bg"}]}`
# shape the list descriptor uses. `width` is an externally-tagged enum
# (`{"Flex": 1.0}` / `{"Fixed": 12.0}` / `{"Content": {"min","max"}}`) and
# `align` is one of "Left" / "Center" / "Right".

_SIZE_UNITS = ("B", "KB", "MB", "GB", "TB")


def _human_size(n: int | None) -> str:
    """Human-readable byte count; "" for directories (size_bytes is None)."""
    if n is None:
        return ""
    size = float(n)
    i = 0
    while size >= 1024 and i < len(_SIZE_UNITS) - 1:
        size /= 1024
        i += 1
    return f"{int(size)} {_SIZE_UNITS[i]}" if i == 0 else f"{size:.1f} {_SIZE_UNITS[i]}"


def _cell(text: str) -> dict:
    """One `StyledText` table cell -- same shape as a tree row's `text`."""
    return _styled_text(text)


def grid_descriptor(model: Model, folder: Node | None = None) -> dict:
    """Build the `insert_data_table` descriptor for one folder's contents."""
    folder = folder or model.root  # static snapshot: the root folder's contents
    children = folder.children or ()  # already dirs-first, name-sorted by `scan`

    # `Flex` weights share the width proportionally and are unitless, so they
    # lay out the same on every backend. `Fixed` would NOT: it is in
    # surface-native units (cells on TUI, *pixels* on native), so a `Fixed(12)`
    # that is 12 cells wide on the terminal is only 12 pixels -- a couple of
    # characters -- in a native window. Name takes the lion's share; Size and
    # Kind each get a quarter.
    columns = [
        {"title": "Name", "width": {"Flex": 2.0}, "align": "Left"},
        {"title": "Size", "width": {"Flex": 1.0}, "align": "Right"},
        {"title": "Kind", "width": {"Flex": 1.0}, "align": "Left"},
    ]

    rows = []
    for child in children:
        kind = "folder" if child.is_dir else "file"
        size = "" if child.is_dir else _human_size(child.size_bytes)
        rows.append({"cells": [_cell(child.name), _cell(size), _cell(kind)]})

    return {"id": "fm:grid", "columns": columns, "rows": rows}


# --- Step 4: assemble the live QuiApp ---------------------------------------
#
# The `render` callback is the seam that makes the UI live without the handler
# ever calling back into `QuiApp` (which would be a reentrant borrow). The flow
# is: event -> handler mutates plain Python `state` -> the runner repaints ->
# `render()` reads `state` and returns the whole frame's descriptors. Here the
# only thing that changes is a selection cursor into the flattened tree rows;
# the grid follows it, showing the selected folder's contents (or, when a file
# is selected, the folder that contains it).

UP_KEY = {"Named": "Up"}
DOWN_KEY = {"Named": "Down"}


class FileManager:
    """Holds the model, the flattened tree, and the `state` dict the `render`
    and handler callbacks share. Build one, then call `run_*` for a real
    backend or drive it headlessly with `push_event`/`step`."""

    def __init__(self, root_path: Path, width: int = 100, height: int = 30):
        self.model = build_model(root_path)
        self.rows, self.nodes, self.by_path = flatten(self.model)
        self.state = {"cursor": 0}  # index into self.rows / self.nodes
        self.app = QuiApp(width, height, render=self._render)
        self.app.set_handler(self._handle)

    def _selected_node(self) -> Node:
        return self.nodes[self.state["cursor"]]

    def _grid_folder(self) -> Node:
        """Which folder the grid lists: the selected node if it is a folder,
        else the folder that contains the selected file."""
        node = self._selected_node()
        if node.is_dir:
            return node
        parent_path = tuple(self.rows[self.state["cursor"]]["path"][:-1])
        return self.by_path.get(parent_path, self.model.root)

    def _render(self) -> dict:
        """Per-frame: rebuild both panes from `state`. Reads only; never calls
        back into `QuiApp`."""
        selected_path = self.rows[self.state["cursor"]]["path"]
        return {
            "tree": tree_from_rows(self.rows, selected_path=selected_path),
            "data_table": grid_descriptor(self.model, folder=self._grid_folder()),
        }

    def _handle(self, event) -> None:
        """Per-event: move the cursor on Up/Down. Mutates `state` only; never
        calls back into `QuiApp`."""
        if not (isinstance(event, dict) and "KeyPressed" in event):
            return
        key = event["KeyPressed"]["key"]
        last = len(self.rows) - 1
        if key == DOWN_KEY:
            self.state["cursor"] = min(last, self.state["cursor"] + 1)
        elif key == UP_KEY:
            self.state["cursor"] = max(0, self.state["cursor"] - 1)


def _dump(node: Node, indent: int = 0) -> None:
    """Human-readable dump of the model, for Step 1 verification."""
    pad = "  " * indent
    if node.is_dir:
        chevron = "v" if node.expanded else ">"
        print(f"{pad}{chevron} {node.name}/")
        for child in node.children or ():
            _dump(child, indent + 1)
    else:
        size = "" if node.size_bytes is None else f"  ({node.size_bytes} B)"
        print(f"{pad}  {node.name}{size}")


def _dump_tree_rows(descriptor: dict) -> None:
    """Human-readable dump of the flattened tree rows, for Step 2 verification."""
    for row in descriptor["rows"]:
        pad = "  " * row["indent"]
        expanded = row["is_expanded"]  # None=leaf, True/False=branch
        chevron = "" if expanded is None else ("▾ " if expanded else "▸ ")
        glyph = row["icon"]["glyph"]
        text = row["text"]["spans"][0]["text"]
        path = ",".join(str(i) for i in row["path"]) or "-"
        print(f"  path=[{path:<10}] {pad}{chevron}{glyph} {text}")


def _dump_grid(descriptor: dict) -> None:
    """Human-readable dump of the grid rows, for Step 3 verification."""
    titles = [c["title"] for c in descriptor["columns"]]
    widths = [16, 10, 8]

    def _row_text(cells_text: list[str]) -> str:
        return "  ".join(f"{t:<{w}}" for t, w in zip(cells_text, widths))

    print("  " + _row_text(titles))
    for row in descriptor["rows"]:
        cells_text = ["".join(s["text"] for s in cell["spans"]) for cell in row["cells"]]
        print("  " + _row_text(cells_text))


class _Quit(Exception):
    """Raised from the handler to unwind out of `run_tui()`/`run_native()` --
    a terminal has no window-manager close event, so a quit keystroke is the
    only way to ask `run_tui()` to return."""


def _wrap_quit(fm: FileManager) -> None:
    """Wrap the handler so `q`/Escape exits the real run loops, leaving the
    headless demo (which never sees those keys) untouched."""
    inner = fm._handle

    def handler(event) -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                raise _Quit
        inner(event)

    fm.app.set_handler(handler)


def _run_headless(fm: FileManager, presses: int = 3) -> None:
    """Drive the app with Python owning the loop: press Down `presses` times,
    then print the final screen -- runnable with no feature build."""
    down = {"KeyPressed": {"key": DOWN_KEY, "modifiers": {"shift": False, "ctrl": False, "alt": False, "meta": False}, "repeat": False}}
    for _ in range(presses):
        fm.app.push_event(down)
        fm.app.step()
    print(fm.app.screen())
    print(f"\ncursor={fm.state['cursor']}  selected={fm._selected_node().name!r}")


def _dump_steps(model: Model) -> None:
    """The step-by-step descriptor dumps (Steps 1-3), kept for inspection."""
    print("# model (Step 1)")
    _dump(model.root)
    print("\n# tree rows (Step 2)")
    _dump_tree_rows(tree_descriptor(model))
    print("\n# grid rows (Step 3)")
    _dump_grid(grid_descriptor(model))


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    root = Path(args[0]) if args else Path.cwd()

    if "--dump" in flags:
        _dump_steps(build_model(root.resolve()))
        sys.exit(0)

    fm = FileManager(root.resolve())

    # --- the only platform-specific line is the run call below --------------
    if "--tui" in flags:
        _wrap_quit(fm)  # real terminal: raw mode, live redraw on key events
        try:
            fm.app.run_tui()
        except _Quit:
            pass
    elif "--native" in flags:
        _wrap_quit(fm)  # OS window: GTK / AppKit / Win32 per the built feature
        try:
            fm.app.run_native(title="quadraui file manager")
        except _Quit:
            pass
    else:
        _run_headless(fm)  # Python owns the loop; prints the final screen
