"""Cross-backend file manager: a resizable two-pane app built from one set of
`quadraui` descriptors that run unchanged on TUI, GTK, macOS, and Windows --
only the final run call differs per platform.

Layout (the `render` callback returns a *layout tree*, new in the bindings):

    split (resizable divider, draggable)
    ├─ first  : a FOLDERS-ONLY tree (navigation)
    └─ second : the details area -- tabs, one per opened folder, each tab a
                data_table of that folder's contents (files + subfolders)

The UI is *live*: a plain Python `state` dict holds what changes (the tree
cursor, the split ratio, which folders are open as tabs, the active tab), the
event handler mutates it, and `render()` rebuilds the whole layout tree from it
every frame. Neither callback touches `QuiApp` -- the handler only mutates
`state`, the render callback only reads it and returns descriptors -- the
`render(state)` seam the bindings use to avoid a reentrant borrow.

Keys: Up/Down move the folder cursor; Enter opens the cursored folder as a
details tab (or focuses it if already open); drag the divider to resize; click
a tab to switch. `q`/Escape quit the real run loops.

    python quadraui-py/examples/file_manager.py [PATH]            # headless demo
    python quadraui-py/examples/file_manager.py [PATH] --tui      # real terminal
    python quadraui-py/examples/file_manager.py [PATH] --native   # OS window
    python quadraui-py/examples/file_manager.py [PATH] --dump      # descriptor dump
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
    chevron state.
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
    """The scanned tree. `root` feeds both panes: the folders-only tree walks
    its directory nodes; the details tabs list a chosen folder's children."""

    root: Node = field(default=None)  # type: ignore[assignment]


def build_model(root_path: Path) -> Model:
    return Model(root=scan(root_path))


# --- model -> folders-only tree descriptor ----------------------------------
#
# The tree pane shows *only directories* -- files live in the details tabs. We
# walk the model depth-first in display order, emitting a row per directory and
# tagging it with its `path`: the route from the root as a list of child
# indices among *folders* (a `TreePath`, read on the Rust side as `Vec<u16>`).
# A folder's path stays a valid prefix of its subfolders' paths, so nesting and
# selection still line up even though files are omitted. `indent` is folder
# depth.

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
    """One `StyledText` with a single uncolored span -- the shape tree `text`,
    data-table cells, and text_display lines all use; the theme picks colors."""
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _flatten_folders(
    node: Node,
    path: list[int],
    indent: int,
    rows: list[dict],
    nodes: list[Node],
) -> None:
    """Append `node` (a directory) as one tree row, then recurse into its
    subdirectories only. `rows` and `nodes` stay index-aligned; `path` numbers
    by folder-sibling index so it addresses the folders-only tree."""
    rows.append(
        {
            "path": path,
            "indent": indent,
            "icon": FOLDER_ICON,
            "text": _styled_text(node.name),
            "badge": None,
            # Every row is a folder, so every row is a branch.
            "is_expanded": node.expanded,
        }
    )
    nodes.append(node)
    subdirs = [c for c in (node.children or ()) if c.is_dir]
    for i, child in enumerate(subdirs):
        _flatten_folders(child, path + [i], indent + 1, rows, nodes)


def folder_rows(model: Model) -> tuple[list[dict], list[Node], dict[tuple[int, ...], Node]]:
    """Walk the model once into index-aligned `(rows, nodes)` plus a
    `path -> Node` map, directories only. The snapshot is static, so callers
    flatten once and reuse these across frames, varying only `selected_path`."""
    rows: list[dict] = []
    nodes: list[Node] = []
    _flatten_folders(model.root, path=[], indent=0, rows=rows, nodes=nodes)
    by_path = {tuple(r["path"]): n for r, n in zip(rows, nodes)}
    return rows, nodes, by_path


def tree_leaf(rows: list[dict], *, selected_path: list[int]) -> dict:
    """A layout-tree *leaf* wrapping the folders-only tree descriptor. A leaf is
    `{"id", "<kind>": <descriptor>}`; the id is unique across the whole tree."""
    return {
        "id": "fm:tree",
        "tree": {
            "id": "fm:tree",
            "rows": rows,
            "selection_mode": "Single",
            "selected_path": selected_path,
            "scroll_offset": 0,
            "has_focus": True,
            "style": TREE_STYLE,
        },
    }


# --- folder -> details data_table --------------------------------------------
#
# Each details tab lists the immediate children (files + subfolders) of one
# opened folder. `width` is `{"Flex": w}` -- unitless proportional weights that
# lay out the same on every backend, unlike `{"Fixed": n}` (surface-native
# units: cells on TUI, *pixels* on native).

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


def details_table(folder: Node, index: int) -> dict:
    """The data_table descriptor for one opened folder's contents, as the
    content subtree of tab `index`. Unique id per tab so each instance is
    independently addressable."""
    table_id = f"fm:grid:{index}"
    columns = [
        {"title": "Name", "width": {"Flex": 2.0}, "align": "Left"},
        {"title": "Size", "width": {"Flex": 1.0}, "align": "Right"},
        {"title": "Kind", "width": {"Flex": 1.0}, "align": "Left"},
    ]

    children = folder.children
    if children is None:
        # A folder beyond the scan depth: we know it is a folder but never
        # loaded its contents (expand-on-demand is a later step).
        rows = [{"cells": [_styled_text("(not scanned -- beyond depth limit)"),
                           _styled_text(""), _styled_text("")]}]
    else:
        rows = []
        for child in children:
            kind = "folder" if child.is_dir else "file"
            size = "" if child.is_dir else _human_size(child.size_bytes)
            rows.append({"cells": [_styled_text(child.name),
                                   _styled_text(size), _styled_text(kind)]})

    return {"id": table_id, "columns": columns, "rows": rows}


# --- the live QuiApp ---------------------------------------------------------
#
# event -> handler mutates plain Python `state` -> the runner repaints ->
# `render()` reads `state` and returns the whole frame's layout tree. The
# things that change: the tree cursor, the split ratio, the list of opened
# folders (tabs), and the active tab.

UP_KEY = {"Named": "Up"}
DOWN_KEY = {"Named": "Down"}
ENTER_KEY = {"Named": "Enter"}


class FileManager:
    """Holds the model, the flattened folders-only tree, and the `state` dict
    the `render` and handler callbacks share. Build one, then call `run_*` for
    a real backend or drive it headlessly with `push_event`/`step`."""

    def __init__(self, root_path: Path, width: int = 100, height: int = 30):
        self.model = build_model(root_path)
        self.rows, self.nodes, self.by_path = folder_rows(self.model)
        self.state = {
            "cursor": 0,       # index into self.rows / self.nodes (folders)
            "ratio": 0.3,      # tree pane width fraction (first pane of the split)
            "open": [],        # list of folder Nodes shown as tabs, in tab order
            "active_tab": 0,   # index into state["open"]
        }
        self.app = QuiApp(width, height, render=self._render)
        self.app.set_handler(self._handle)

    def _cursored_folder(self) -> Node:
        return self.nodes[self.state["cursor"]]

    def _open_folder(self, node: Node) -> None:
        """Open `node` as a details tab, or focus its tab if already open."""
        for i, existing in enumerate(self.state["open"]):
            if existing.path == node.path:
                self.state["active_tab"] = i
                return
        self.state["open"].append(node)
        self.state["active_tab"] = len(self.state["open"]) - 1

    def _details_pane(self) -> dict:
        """The split's second pane: a placeholder until a folder is opened,
        then a tabs container with one data_table tab per opened folder."""
        open_folders = self.state["open"]
        if not open_folders:
            return {
                "id": "fm:empty",
                "text_display": {
                    "id": "fm:empty",
                    "lines": [_styled_text("Select a folder and press Enter to open it.")],
                },
            }
        return {
            "id": "fm:tabs",
            "tabs": {
                "id": "fm:tabs",
                "selected": self.state["active_tab"],
                "tabs": [
                    {
                        "id": f"tab:{i}",
                        "title": folder.name,
                        "content": {
                            "id": f"fm:grid:{i}",
                            "data_table": details_table(folder, i),
                        },
                    }
                    for i, folder in enumerate(open_folders)
                ],
            },
        }

    def _render(self) -> dict:
        """Per-frame: rebuild the whole split from `state`. Reads only; never
        calls back into `QuiApp`."""
        selected_path = self.rows[self.state["cursor"]]["path"]
        return {
            "split": {
                "id": "fm:split",
                "orientation": "horizontal",  # panes side by side, vertical divider
                "ratio": self.state["ratio"],
                "first": tree_leaf(self.rows, selected_path=selected_path),
                "second": self._details_pane(),
            }
        }

    def _handle(self, event) -> None:
        """Per-event: move the cursor, open folders, resize, switch tabs.
        Mutates `state` only; never calls back into `QuiApp`."""
        if not isinstance(event, dict):
            return
        if "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            last = len(self.rows) - 1
            if key == DOWN_KEY:
                self.state["cursor"] = min(last, self.state["cursor"] + 1)
            elif key == UP_KEY:
                self.state["cursor"] = max(0, self.state["cursor"] - 1)
            elif key == ENTER_KEY:
                self._open_folder(self._cursored_folder())
        elif "SplitDividerDragged" in event:
            self.state["ratio"] = event["SplitDividerDragged"]["new_ratio"]
        elif "TabBar" in event:
            activation = event["TabBar"][1]
            if "TabActivated" in activation:
                self.state["active_tab"] = activation["TabActivated"]["index"]


# --- inspection / headless helpers -------------------------------------------


def _dump(node: Node, indent: int = 0) -> None:
    """Human-readable dump of the model."""
    pad = "  " * indent
    if node.is_dir:
        chevron = "v" if node.expanded else ">"
        print(f"{pad}{chevron} {node.name}/")
        for child in node.children or ():
            _dump(child, indent + 1)
    else:
        size = "" if node.size_bytes is None else f"  ({node.size_bytes} B)"
        print(f"{pad}  {node.name}{size}")


def _dump_folder_rows(rows: list[dict]) -> None:
    """Human-readable dump of the folders-only tree rows."""
    for row in rows:
        pad = "  " * row["indent"]
        chevron = "▾ " if row["is_expanded"] else "▸ "
        glyph = row["icon"]["glyph"]
        text = row["text"]["spans"][0]["text"]
        path = ",".join(str(i) for i in row["path"]) or "-"
        print(f"  path=[{path:<8}] {pad}{chevron}{glyph} {text}")


def _dump_grid(descriptor: dict) -> None:
    """Human-readable dump of a details table."""
    titles = [c["title"] for c in descriptor["columns"]]
    widths = [20, 10, 8]

    def _row_text(cells_text: list[str]) -> str:
        return "  ".join(f"{t:<{w}}" for t, w in zip(cells_text, widths))

    print("  " + _row_text(titles))
    for row in descriptor["rows"]:
        cells_text = ["".join(s["text"] for s in cell["spans"]) for cell in row["cells"]]
        print("  " + _row_text(cells_text))


def _dump_steps(model: Model) -> None:
    print("# model")
    _dump(model.root)
    print("\n# folders-only tree rows")
    rows, _nodes, _by_path = folder_rows(model)
    _dump_folder_rows(rows)
    print("\n# details table for the root folder")
    _dump_grid(details_table(model.root, 0))


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


def _key_event(key: dict) -> dict:
    return {
        "KeyPressed": {
            "key": key,
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "meta": False},
            "repeat": False,
        }
    }


def _run_headless(fm: FileManager) -> None:
    """Drive the app with Python owning the loop: walk down to a folder, open
    it as a tab, open a second one, then print the final screen -- runnable
    with no feature build."""
    script = [DOWN_KEY, ENTER_KEY, DOWN_KEY, ENTER_KEY]
    for key in script:
        fm.app.push_event(_key_event(key))
        fm.app.step()
    print(fm.app.screen())
    open_names = [n.name for n in fm.state["open"]]
    print(f"\ncursor={fm.state['cursor']}  open_tabs={open_names}  "
          f"active_tab={fm.state['active_tab']}  ratio={fm.state['ratio']}")


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
