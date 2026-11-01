"""Cross-backend file manager: an interactive two-pane browser built from one
set of `quadraui` descriptors that run unchanged on TUI, GTK, macOS, and
Windows -- only the final run call differs per platform.

Layout (the `render` callback returns a *layout tree*):

    split (vertical: chrome + body stacked over a status bar)
    ├─ first  : sidebar_panel
    │            ├─ toolbar : a key-legend affordance strip (reserved top slot)
    │            └─ content : split (resizable divider, draggable)
    │                          ├─ first  : a FOLDERS-ONLY tree (navigation)
    │                          └─ second : the details area -- tabs, one per
    │                                      opened folder, each tab a data_table
    │                                      of that folder's contents
    └─ second : a live status bar (cursored folder, item count, open tabs)

The tree loads lazily: only one directory level is read at a time, so you can
browse an arbitrarily deep filesystem without scanning it up front. Folders
expand and collapse; a folder's children are read the first time it is opened.

The UI is *live*: plain Python attributes hold what changes (the cursor node,
the split ratio, which folders are open as tabs, the active tab, a transient
status hint), the event handler mutates them, and `render()` rebuilds the whole
layout tree from them every frame. Neither callback calls back into `QuiApp`.

Interaction (keyboard drives, mouse works too):
  Up / Down    move the folder cursor
  Right        expand the cursored folder (load on demand); if already
               expanded, descend into its first subfolder
  Left         collapse the cursored folder; if already collapsed, jump to
               its parent
  Enter / o    open the cursored folder as a details tab (and expand it)
  r            reload the cursored folder from disk
  click a tree row         select it and toggle its expansion
  activate a details row   drill into that subfolder (or report a file's size)
  click a tab / its ×      switch to / close that tab
  q / Escape   quit the real run loops

    python quadraui-py/examples/file_manager.py [PATH]            # headless demo
    python quadraui-py/examples/file_manager.py [PATH] --tui      # real terminal
    python quadraui-py/examples/file_manager.py [PATH] --native   # OS window
    python quadraui-py/examples/file_manager.py [PATH] --dump     # descriptor dump
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path

from quadraui import QuiApp


@dataclass
class Node:
    """One filesystem entry in the lazily-scanned model.

    `children is None` means "not loaded yet" -- a directory whose contents
    have never been read. After loading, `children` is a (possibly empty)
    list. Files always keep `children is None` and are never loaded.
    `expanded` is True only for directories the user has opened in the tree.
    """

    name: str
    path: Path
    is_dir: bool
    size_bytes: int | None = None
    parent: "Node | None" = field(default=None, repr=False)
    children: list["Node"] | None = None
    expanded: bool = False
    load_error: bool = False

    def subdirs(self) -> list["Node"]:
        """Loaded child directories in display order (empty if not loaded)."""
        return [c for c in (self.children or ()) if c.is_dir]


def _safe_size(path: Path) -> int | None:
    try:
        return path.stat().st_size
    except OSError:
        return None


def _sort_key(node: Node) -> tuple[bool, str]:
    """Directories first, then case-insensitive name."""
    return (not node.is_dir, node.name.lower())


def load_children(node: Node) -> None:
    """Read `node`'s immediate children (one level only), dotfiles skipped and
    directories sorted first. On an unreadable directory, leaves it empty and
    flags `load_error`. Files are a no-op."""
    if not node.is_dir:
        return
    try:
        entries = [c for c in node.path.iterdir() if not c.name.startswith(".")]
    except OSError:
        node.children = []
        node.load_error = True
        return
    kids = [
        Node(
            name=child.name,
            path=child,
            is_dir=child.is_dir(),
            size_bytes=None if child.is_dir() else _safe_size(child),
            parent=node,
        )
        for child in entries
    ]
    kids.sort(key=_sort_key)
    node.children = kids
    node.load_error = False


def ensure_loaded(node: Node) -> None:
    """Load `node`'s children once; cheap no-op if already loaded or a file."""
    if node.is_dir and node.children is None:
        load_children(node)


def build_root(root_path: Path) -> Node:
    """The model root: loaded one level deep and expanded, so the tree opens on
    the root's immediate subfolders."""
    root = Node(name=root_path.name or str(root_path), path=root_path, is_dir=root_path.is_dir())
    ensure_loaded(root)
    root.expanded = True
    return root


# --- model -> folders-only tree descriptor ----------------------------------
#
# The tree pane shows *only directories* -- files live in the details tabs. We
# walk the model depth-first in display order over *expanded* folders only,
# emitting a row per directory and tagging it with its `path`: the route from
# the root as a list of child indices among *folders* (a `TreePath`, read on
# the Rust side as `Vec<u16>`). A folder's path stays a valid prefix of its
# subfolders' paths, so a click on a row resolves back to its node. `indent` is
# folder depth.

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


def folder_rows(root: Node) -> tuple[list[dict], list[Node]]:
    """Walk the expanded model into index-aligned `(rows, nodes)`, directories
    only. Recomputed each frame, since expanding/collapsing changes which rows
    are visible."""
    rows: list[dict] = []
    nodes: list[Node] = []

    def walk(node: Node, path: list[int], indent: int) -> None:
        rows.append(
            {
                "path": path,
                "indent": indent,
                "icon": FOLDER_ICON,
                "text": _styled_text(node.name),
                "badge": None,
                "is_expanded": node.expanded,
            }
        )
        nodes.append(node)
        if node.expanded:
            for i, child in enumerate(node.subdirs()):
                walk(child, path + [i], indent + 1)

    walk(root, path=[], indent=0)
    return rows, nodes


def tree_leaf(rows: list[dict], *, selected_path: list[int], scroll_offset: int) -> dict:
    """A layout-tree *leaf* wrapping the folders-only tree descriptor. A leaf is
    `{"id", "<kind>": <descriptor>}`; the id is unique across the whole tree.
    `has_focus` keeps arrow keys / Enter routed here on the real backends.
    `scroll_offset` is the index of the first painted row -- the tree is purely
    declarative (it renders `rows[scroll_offset..]`), so the app owns it and
    advances it to keep the cursor in view; the backend derives the scrollbar
    thumb from it."""
    return {
        "id": "fm:tree",
        "tree": {
            "id": "fm:tree",
            "rows": rows,
            "selection_mode": "Single",
            "selected_path": selected_path,
            "scroll_offset": scroll_offset,
            "has_focus": True,
            "style": TREE_STYLE,
        },
    }


# --- folder -> details data_table --------------------------------------------
#
# Each details tab lists the immediate children (files + subfolders) of one
# opened folder. `width` is `{"Flex": w}` -- unitless proportional weights that
# lay out the same on every backend, unlike `{"Fixed": n}` (surface-native
# units: cells on TUI, *pixels* on native). Activating a row emits a
# `DataTable/RowActivated` event whose `idx` indexes straight into the folder's
# `children`, so a click on a subfolder row can drill into it.

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
    content subtree of tab `index`. Row order matches `folder.children`, so a
    `RowActivated{idx}` maps straight back to a child node."""
    table_id = f"fm:grid:{index}"
    columns = [
        {"title": "Name", "width": {"Flex": 2.0}, "align": "Left"},
        {"title": "Size", "width": {"Flex": 1.0}, "align": "Right"},
        {"title": "Kind", "width": {"Flex": 1.0}, "align": "Left"},
    ]

    children = folder.children or []
    if not children:
        note = "(unreadable)" if folder.load_error else "(empty folder)"
        rows = [{"cells": [_styled_text(note), _styled_text(""), _styled_text("")]}]
    else:
        rows = []
        for child in children:
            # A trailing "/" hints the row is navigable (activate to drill in).
            name = f"{child.name}/" if child.is_dir else child.name
            kind = "folder" if child.is_dir else "file"
            size = "" if child.is_dir else _human_size(child.size_bytes)
            rows.append(
                {"cells": [_styled_text(name), _styled_text(size), _styled_text(kind)]}
            )

    return {"id": table_id, "columns": columns, "rows": rows}


# --- chrome: toolbar band + status bar ---------------------------------------
#
# The toolbar is the body's chrome, carried by a `sidebar_panel` wrapper whose
# reserved top slot paints it above the content on every backend. The bindings
# don't route clicks on toolbar buttons to events yet, so it is a *key legend*
# -- an affordance, not a control; the keyboard and the tree/table clicks drive
# everything. The status bar is a live bottom band, rebuilt from state each
# frame.

TOOLBAR = {
    "id": "fm:toolbar",
    "buttons": [
        {"Action": {"id": "fm:k:move", "label": "↑↓ Move"}},
        {"Action": {"id": "fm:k:expand", "label": "→ Expand"}},
        {"Action": {"id": "fm:k:collapse", "label": "← Collapse"}},
        {"Action": {"id": "fm:k:open", "label": "↵ Open"}},
        {"Action": {"id": "fm:k:refresh", "label": "r Refresh"}},
    ],
}

# `StatusBarSegment` requires explicit fg + bg (unlike the theme-driven
# `StyledText` spans elsewhere); the accent foreground highlights the cursored
# folder's name.
_STATUS_FG = {"r": 220, "g": 220, "b": 230, "a": 255}
_STATUS_BG = {"r": 30, "g": 32, "b": 48, "a": 255}
_STATUS_ACCENT = {"r": 245, "g": 224, "b": 138, "a": 255}


def _segment(text: str, *, accent: bool = False, bold: bool = False) -> dict:
    """One `StatusBarSegment`: styled text with explicit colors."""
    return {
        "text": text,
        "fg": _STATUS_ACCENT if accent else _STATUS_FG,
        "bg": _STATUS_BG,
        "bold": bold,
    }


# --- the live QuiApp ---------------------------------------------------------
#
# event -> handler mutates plain Python attributes -> the runner repaints ->
# `render()` reads them and returns the whole frame's layout tree.

UP_KEY = {"Named": "Up"}
DOWN_KEY = {"Named": "Down"}
LEFT_KEY = {"Named": "Left"}
RIGHT_KEY = {"Named": "Right"}
ENTER_KEY = {"Named": "Enter"}


def _key_name(key) -> str | None:
    """Canonical name of a raw key. A plain `KeyPressed` carries a tagged dict
    (`{"Named": "Down"}` / `{"Char": "o"}`); a focus-scoped leaf event carries
    the already-stringified name (`"Down"` / `"o"`). Both collapse to the name."""
    if isinstance(key, str):
        return key
    if isinstance(key, dict):
        return key.get("Named") or key.get("Char")
    return None


def _event_key(event: dict) -> str | None:
    """The canonical key name from a key event in any shape the backends
    deliver: a plain `KeyPressed`, or a focus-scoped `{"Tree"/"List"/"TabBar":
    [id, {"KeyPressed": ...}]}` (the focus manager rewrites a focused leaf's
    keys into its own variant, with the key already stringified). Returns None
    for a non-key event."""
    pressed = event.get("KeyPressed")
    if isinstance(pressed, dict):
        return _key_name(pressed.get("key"))
    for wrap in ("Tree", "List", "TabBar"):
        payload = event.get(wrap)
        if isinstance(payload, list) and len(payload) == 2 and isinstance(payload[1], dict):
            inner = payload[1].get("KeyPressed")
            if isinstance(inner, dict):
                return _key_name(inner.get("key"))
            return None
    return None


class FileManager:
    """Holds the lazily-scanned model and the live UI state the `render` and
    handler callbacks share. Build one, then call `run_*` for a real backend or
    drive it headlessly with `push_event`/`step`."""

    def __init__(self, root_path: Path, width: int = 100, height: int = 30):
        self.root = build_root(root_path)
        self.height = height
        self.cursor: Node = self.root   # the selected folder node
        self.scroll = 0                 # index of the first painted tree row
        self.ratio = 0.3                # tree pane width fraction
        self.open: list[Node] = []      # folder nodes shown as tabs, in order
        self.active_tab = 0             # index into self.open
        self.message = ""               # transient status hint
        self.app = QuiApp(width, height, render=self._render)
        self.app.set_handler(self._handle)

    # --- navigation helpers --------------------------------------------------

    def _visible(self) -> tuple[list[dict], list[Node]]:
        """Current folder rows/nodes, with the cursor pulled back to its nearest
        still-visible ancestor if a collapse hid it."""
        rows, nodes = folder_rows(self.root)
        if self.cursor not in nodes:
            node = self.cursor
            while node is not None and node not in nodes:
                node = node.parent
            self.cursor = node or self.root
        return rows, nodes

    def _move(self, delta: int) -> None:
        rows, nodes = self._visible()
        i = nodes.index(self.cursor)
        self.cursor = nodes[max(0, min(len(nodes) - 1, i + delta))]

    def _tree_capacity(self) -> int:
        """Painted tree rows that fit: the frame height less the three chrome
        lines above/below the tree -- the toolbar row, the outer split's
        divider, and the bottom status line. An estimate (the real terminal
        may differ from the configured height, like the status-bar pinning in
        `_render`); erring one row short only scrolls a touch early, never
        hides the cursor."""
        return max(1, self.height - 3)

    def _scroll_to_cursor(self, cursor_index: int, row_count: int) -> None:
        """Advance `scroll` so the cursor row stays within the painted window,
        then clamp it so the last page never leaves a blank gap. The tree is
        declarative -- it renders `rows[scroll..]` -- so without this the view
        never follows the cursor and the scrollbar thumb never moves."""
        capacity = self._tree_capacity()
        if cursor_index < self.scroll:
            self.scroll = cursor_index
        elif cursor_index >= self.scroll + capacity:
            self.scroll = cursor_index - capacity + 1
        self.scroll = max(0, min(self.scroll, max(0, row_count - capacity)))

    def _expand(self) -> None:
        """Expand the cursored folder (loading it on demand); if it is already
        expanded, descend into its first subfolder."""
        node = self.cursor
        if not node.is_dir:
            return
        ensure_loaded(node)
        subdirs = node.subdirs()
        if not node.expanded and subdirs:
            node.expanded = True
        elif node.expanded and subdirs:
            self.cursor = subdirs[0]

    def _collapse(self) -> None:
        """Collapse the cursored folder; if already collapsed, jump to parent."""
        node = self.cursor
        if node.is_dir and node.expanded:
            node.expanded = False
        elif node.parent is not None:
            self.cursor = node.parent

    def _reveal(self, node: Node) -> None:
        """Expand every ancestor so `node` is a visible tree row."""
        parent = node.parent
        while parent is not None:
            ensure_loaded(parent)
            parent.expanded = True
            parent = parent.parent

    def _open(self, node: Node | None = None) -> None:
        """Open `node` (default: the cursor) as a details tab, expanding it;
        focus its tab if already open. A file just reports itself."""
        node = node or self.cursor
        if not node.is_dir:
            self.message = f"{node.name} ({_human_size(node.size_bytes)})"
            return
        ensure_loaded(node)
        for i, existing in enumerate(self.open):
            if existing.path == node.path:
                self.active_tab = i
                return
        self.open.append(node)
        self.active_tab = len(self.open) - 1

    def _refresh(self) -> None:
        node = self.cursor
        if node.is_dir:
            node.children = None
            ensure_loaded(node)
            self.message = f"reloaded {node.name}"

    def _close_tab(self, index: int) -> None:
        if 0 <= index < len(self.open):
            del self.open[index]
            if self.active_tab >= len(self.open):
                self.active_tab = max(0, len(self.open) - 1)

    def _on_row_click(self, path: list[int]) -> None:
        """A tree-row click: select that folder and toggle its expansion."""
        rows, nodes = folder_rows(self.root)
        for row, node in zip(rows, nodes):
            if row["path"] == list(path):
                self.cursor = node
                ensure_loaded(node)
                if node.subdirs():
                    node.expanded = not node.expanded
                return

    def _activate_detail(self, idx: int) -> None:
        """A details-table row was activated (clicked): drill into a subfolder,
        or report a file's size."""
        if not self.open:
            return
        folder = self.open[self.active_tab]
        children = folder.children or []
        if not (0 <= idx < len(children)):
            return
        child = children[idx]
        if child.is_dir:
            folder.expanded = True
            self._reveal(child)
            self.cursor = child
            self._open(child)
        else:
            self.message = f"{child.name} ({_human_size(child.size_bytes)})"

    # --- render + handle -----------------------------------------------------

    def _details_pane(self) -> dict:
        """The body split's second pane: a placeholder until a folder is opened,
        then a closable tabs container with one data_table tab per open folder."""
        if not self.open:
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
                "selected": self.active_tab,
                "closable": True,
                "tabs": [
                    {
                        "id": f"tab:{i}",
                        "title": folder.name,
                        "closable": True,
                        "content": {
                            "id": f"fm:grid:{i}",
                            "data_table": details_table(folder, i),
                        },
                    }
                    for i, folder in enumerate(self.open)
                ],
            },
        }

    def _status_leaf(self) -> dict:
        """The outer split's bottom pane: a live status bar showing the cursored
        folder, its item count, the open-tab summary, and any transient hint."""
        node = self.cursor
        if node.children is None:
            count_text = "unopened"
        else:
            count_text = f"{len(node.children)} items"
        left = [
            _segment(f" {node.name} ", accent=True, bold=True),
            _segment(f" {count_text} "),
        ]
        if self.message:
            left.append(_segment(f" {self.message} "))
        open_n = len(self.open)
        if open_n:
            right = [_segment(f" tab {self.active_tab + 1}/{open_n} ")]
        else:
            right = [_segment(" no tabs ")]
        right.append(_segment(f" {int(round(self.ratio * 100))}% "))
        return {
            "id": "fm:status",
            "status_bar": {
                "id": "fm:status",
                "left_segments": left,
                "right_segments": right,
            },
        }

    def _render(self) -> dict:
        """Per-frame: rebuild the whole frame from state. An outer vertical split
        stacks the chrome+body (top) over a one-line status bar (bottom); the
        body is the resizable tree/details split, wrapped in a `sidebar_panel`
        whose reserved slot paints the toolbar above it. Reads only."""
        rows, nodes = self._visible()
        cursor_index = nodes.index(self.cursor)
        selected_path = rows[cursor_index]["path"]
        self._scroll_to_cursor(cursor_index, len(rows))
        body = {
            "split": {
                "id": "fm:split",
                "orientation": "horizontal",  # panes side by side, vertical divider
                "ratio": self.ratio,
                "first": tree_leaf(rows, selected_path=selected_path, scroll_offset=self.scroll),
                "second": self._details_pane(),
            }
        }
        chrome = {
            "sidebar_panel": {
                "id": "fm:chrome",
                "toolbar": TOOLBAR,  # reserved top slot, one line per backend
                "content": body,
            }
        }
        # Pin the status bar to the bottom line: `ratio` derives from the known
        # height so the bottom pane is ~one line, and `second_min` floors it at
        # one line if the real terminal is a different size. Drags on this outer
        # divider are ignored by the handler (it keys on the body split's id).
        h = max(self.height, 2)
        return {
            "split": {
                "id": "fm:outer",
                "orientation": "vertical",  # chrome+body stacked over the status bar
                "ratio": (h - 1) / h,
                "first_min": 3.0,
                "second_min": 1.0,
                "first": chrome,
                "second": self._status_leaf(),
            }
        }

    def _handle(self, event) -> None:
        """Per-event: move the cursor, expand/collapse, open/close tabs, drill
        into folders, resize. Mutates state only; never calls into `QuiApp`."""
        if not isinstance(event, dict):
            return

        key = _event_key(event)
        if key is not None:
            self._on_key(key)
            return

        if "DataTable" in event:
            activation = event["DataTable"][1]
            if "RowActivated" in activation:
                self._activate_detail(activation["RowActivated"]["idx"])
        elif "Tree" in event:
            activation = event["Tree"][1]
            if "RowClicked" in activation:
                self._on_row_click(activation["RowClicked"]["path"])
        elif "TabBar" in event:
            activation = event["TabBar"][1]
            if "TabActivated" in activation:
                self.active_tab = activation["TabActivated"]["index"]
            elif "TabClosed" in activation:
                self._close_tab(activation["TabClosed"]["index"])
        elif "SplitDividerDragged" in event:
            drag = event["SplitDividerDragged"]
            # Only the body split is user-resizable; the outer chrome/status
            # split is fixed, so ignore drags that aren't the body's.
            if drag.get("tree") == "fm:split":
                self.ratio = drag["new_ratio"]

    def _on_key(self, name: str) -> None:
        self.message = ""
        if name == "Down":
            self._move(1)
        elif name == "Up":
            self._move(-1)
        elif name == "Right":
            self._expand()
        elif name == "Left":
            self._collapse()
        elif name in ("Enter", "o"):
            self._open()
        elif name == "r":
            self._refresh()


# --- inspection / headless helpers -------------------------------------------


def _dump(node: Node, indent: int = 0) -> None:
    """Human-readable dump of the loaded model."""
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


def _dump_steps(root_path: Path) -> None:
    root = build_root(root_path)
    print("# model (root loaded one level)")
    _dump(root)
    print("\n# folders-only tree rows")
    rows, _nodes = folder_rows(root)
    _dump_folder_rows(rows)
    print("\n# details table for the root folder")
    _dump_grid(details_table(root, 0))


class _Quit(Exception):
    """Raised from the handler to unwind out of `run_tui()`/`run_native()` --
    a terminal has no window-manager close event, so a quit keystroke is the
    only way to ask `run_tui()` to return."""


def _wrap_quit(fm: FileManager) -> None:
    """Wrap the handler so `q`/Escape exits the real run loops, leaving the
    headless demo (which never sees those keys) untouched."""
    inner = fm._handle

    def handler(event) -> None:
        key = _event_key(event) if isinstance(event, dict) else None
        if key in ("q", "Escape"):
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
    """Drive the app with Python owning the loop: expand a folder, open it as a
    tab, drill into a subfolder from its details table, then print the final
    screen -- runnable with no feature build."""
    steps = [
        _key_event(DOWN_KEY),    # cursor onto the first subfolder
        _key_event(RIGHT_KEY),   # expand it (load on demand)
        _key_event(ENTER_KEY),   # open it as a details tab
        # Activate the first details row (a click would deliver this), drilling
        # into it if it is a subfolder.
        {"DataTable": ["fm:grid:0", {"RowActivated": {"idx": 0}}]},
    ]
    for event in steps:
        fm.app.push_event(event)
        fm.app.step()
    print(fm.app.screen())
    open_names = [n.name for n in fm.open]
    print(f"\ncursor={fm.cursor.name}  open_tabs={open_names}  "
          f"active_tab={fm.active_tab}  ratio={fm.ratio}")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    root = Path(args[0]) if args else Path.cwd()

    if "--dump" in flags:
        _dump_steps(root.resolve())
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
