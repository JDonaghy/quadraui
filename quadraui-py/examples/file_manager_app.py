"""`examples/file_manager.py`, ported onto `quadraui.app`: the same
cross-backend two-pane file manager (lazily-loaded folders-only tree on the
left, one closable data-table tab per opened folder on the right, a live
status bar), but every interactive leaf now carries its own `on_*`
callback instead of `FileManager._handle` switching on widget id and event
shape. Scroll, the split ratio, and the active tab remain framework-owned
widget state -- nothing here reads or writes any of the three, same as
the original.

Interaction, and the headless/`--tui`/`--native` run modes, are identical
to `file_manager.py`'s own docstring; see that module for the walkthrough
of what each key/click does. `--dump` (the raw-descriptor inspection mode)
is not ported here -- it has nothing to do with the app layer this port
exists to demonstrate, and re-adding its ~100 lines of inspection
plumbing would undercut the shorter-and-with-no-manual-bookkeeping point
of the port. Pass anything other than `--tui`/`--native` (including
`--dump`) and this module falls back to the headless demo.

One wire-level difference from the original: a details tab's own id is
`Tabs`' auto-id convention (`"<tabs_id>:tab:<index>"`, here `"fm:tabs:tab:0"`,
...) rather than the original's bare `"tab:0"` -- harmless (nothing reads a
tab id directly), but worth knowing if you diff the two examples' screens.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from quadraui.app import App, SidebarPanel, Split, StatusBar, Tab, Table, Tabs, TextDisplay, Toolbar, Tree, key_name

if TYPE_CHECKING:
    from quadraui.wire import Color, DataRowDescriptor, Icon, StyledText, TreeRowDescriptor, TreeStyle


@dataclass
class Node:
    """One filesystem entry in the lazily-scanned model -- see
    `file_manager.py`'s own `Node` for `children is None`'s meaning."""

    name: str
    path: Path
    is_dir: bool
    size_bytes: int | None = None
    parent: "Node | None" = field(default=None, repr=False)
    children: list["Node"] | None = None
    expanded: bool = False
    load_error: bool = False

    def subdirs(self) -> list["Node"]:
        return [c for c in (self.children or ()) if c.is_dir]


def _safe_size(path: Path) -> int | None:
    try:
        return path.stat().st_size
    except OSError:
        return None


def _sort_key(node: Node) -> tuple[bool, str]:
    return (not node.is_dir, node.name.lower())


def load_children(node: Node) -> None:
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
    if node.is_dir and node.children is None:
        load_children(node)


def build_root(root_path: Path) -> Node:
    root = Node(name=root_path.name or str(root_path), path=root_path, is_dir=root_path.is_dir())
    ensure_loaded(root)
    root.expanded = True
    return root

# Resent unchanged every frame, so the body split's ratio resolves as
# uncontrolled from the second frame on -- see `Split`'s own doc.
BODY_SPLIT_RATIO = 0.3

TREE_STYLE: "TreeStyle" = {
    "indent": 2,
    "show_chevrons": True,
    "chevron_expanded": "▾",
    "chevron_collapsed": "▸",
}
FOLDER_ICON: "Icon" = {"glyph": "\U0001f4c1", "fallback": "/", "color": None}
FILE_ICON: "Icon" = {"glyph": "\U0001f4c4", "fallback": " ", "color": None}


def _styled_text(text: str) -> "StyledText":
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def folder_rows(root: Node) -> tuple[list["TreeRowDescriptor"], list[Node]]:
    """Walk the expanded model into index-aligned `(rows, nodes)`, directories
    only -- recomputed every frame."""
    rows: list["TreeRowDescriptor"] = []
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

_SIZE_UNITS = ("B", "KB", "MB", "GB", "TB")


def _human_size(n: int | None) -> str:
    if n is None:
        return ""
    size = float(n)
    i = 0
    while size >= 1024 and i < len(_SIZE_UNITS) - 1:
        size /= 1024
        i += 1
    return f"{int(size)} {_SIZE_UNITS[i]}" if i == 0 else f"{size:.1f} {_SIZE_UNITS[i]}"


def details_rows(folder: Node) -> list["DataRowDescriptor"]:
    """The data_table rows for one opened folder's contents, subfolders
    first (sorted by `_sort_key` already), a trailing "/" marking a
    navigable row."""
    children = folder.children or []
    if not children:
        note = "(unreadable)" if folder.load_error else "(empty folder)"
        return [{"cells": [_styled_text(note), _styled_text(""), _styled_text("")]}]
    rows: list["DataRowDescriptor"] = []
    for child in children:
        name = f"{child.name}/" if child.is_dir else child.name
        kind = "folder" if child.is_dir else "file"
        size = "" if child.is_dir else _human_size(child.size_bytes)
        rows.append({"cells": [_styled_text(name), _styled_text(size), _styled_text(kind)]})
    return rows

DETAILS_COLUMNS = [
    {"title": "Name", "width": {"Flex": 2.0}, "align": "Left"},
    {"title": "Size", "width": {"Flex": 1.0}, "align": "Right"},
    {"title": "Kind", "width": {"Flex": 1.0}, "align": "Left"},
]

TOOLBAR = Toolbar(
    id="fm:toolbar",
    buttons=[
        {"Action": {"id": "fm:k:move", "label": "↑↓ Move"}},
        {"Action": {"id": "fm:k:expand", "label": "→ Expand"}},
        {"Action": {"id": "fm:k:collapse", "label": "← Collapse"}},
        {"Action": {"id": "fm:k:open", "label": "↵ Open"}},
        {"Action": {"id": "fm:k:refresh", "label": "r Refresh"}},
    ],
)

_STATUS_FG: "Color" = {"r": 220, "g": 220, "b": 230, "a": 255}
_STATUS_BG: "Color" = {"r": 30, "g": 32, "b": 48, "a": 255}
_STATUS_ACCENT: "Color" = {"r": 245, "g": 224, "b": 138, "a": 255}


def _segment(text: str, *, accent: bool = False, bold: bool = False) -> dict:
    return {"text": text, "fg": _STATUS_ACCENT if accent else _STATUS_FG, "bg": _STATUS_BG, "bold": bold}


class FileManager:
    """Holds the lazily-scanned model and the live UI state `view`/the
    node callbacks below share. Build one, then call `run_*` for a real
    backend or drive it headlessly with `push_event`/`step`."""

    def __init__(self, root_path: Path, width: int = 100, height: int = 30):
        self.root = build_root(root_path)
        self.height = height
        self.cursor: Node = self.root
        self.open: list[Node] = []
        self.message = ""
        self.app = App(width, height, view=self._view, on_event=self._on_event)
        self.actions = self.app.actions

    # --- navigation helpers --------------------------------------------------

    def _visible(self) -> tuple[list["TreeRowDescriptor"], list[Node]]:
        rows, nodes = folder_rows(self.root)
        if self.cursor not in nodes:
            node: Node | None = self.cursor
            while node is not None and node not in nodes:
                node = node.parent
            self.cursor = node or self.root
        return rows, nodes

    def _move(self, delta: int) -> None:
        rows, nodes = self._visible()
        i = nodes.index(self.cursor)
        self.cursor = nodes[max(0, min(len(nodes) - 1, i + delta))]

    def _expand(self) -> None:
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
        node = self.cursor
        if node.is_dir and node.expanded:
            node.expanded = False
        elif node.parent is not None:
            self.cursor = node.parent

    def _reveal(self, node: Node) -> None:
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
                self.actions.apply_tab_activated("fm:tabs", i)
                return
        self.open.append(node)
        self.actions.apply_tab_activated("fm:tabs", len(self.open) - 1)

    def _refresh(self) -> None:
        node = self.cursor
        if node.is_dir:
            node.children = None
            ensure_loaded(node)
            self.message = f"reloaded {node.name}"

    def _close_tab(self, index: int) -> None:
        if 0 <= index < len(self.open):
            del self.open[index]

    def _on_row_click(self, path: list[int]) -> None:
        rows, nodes = folder_rows(self.root)
        for row, node in zip(rows, nodes):
            if row["path"] == list(path):
                self.cursor = node
                ensure_loaded(node)
                if node.subdirs():
                    node.expanded = not node.expanded
                return

    def _on_row_toggle_expand(self, path: list[int]) -> None:
        rows, nodes = folder_rows(self.root)
        for row, node in zip(rows, nodes):
            if row["path"] == list(path):
                ensure_loaded(node)
                if node.subdirs():
                    node.expanded = not node.expanded
                return

    def _activate_detail(self, folder: Node, idx: int) -> None:
        """A details-table row was activated (double-click or Enter): drill
        into a subfolder, or report a file's size. Bound to `folder`
        directly by `_details_pane`'s own closure -- no id parsing needed
        to recover which tab this came from."""
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

    # --- view + global keys ---------------------------------------------------

    def _details_pane(self):
        if not self.open:
            return TextDisplay(
                [{"spans": [{"text": "Select a folder and press Enter to open it.", "fg": None, "bg": None}]}],
                id="fm:empty",
            )
        tabs = [
            Tab(
                Table(
                    DETAILS_COLUMNS,
                    details_rows(folder),
                    id=f"fm:grid:{i}",
                    on_activate=lambda idx, folder=folder: self._activate_detail(folder, idx),
                ),
                title=folder.name,
                closable=True,
            )
            for i, folder in enumerate(self.open)
        ]
        return Tabs(tabs, id="fm:tabs", closable=True, on_close=self._close_tab)

    def _status_leaf(self):
        node = self.cursor
        count_text = "unopened" if node.children is None else f"{len(node.children)} items"
        left = [_segment(f" {node.name} ", accent=True, bold=True), _segment(f" {count_text} ")]
        if self.message:
            left.append(_segment(f" {self.message} "))
        open_n = len(self.open)
        right = [_segment(f" {open_n} tab{'s' if open_n != 1 else ''} ")] if open_n else [_segment(" no tabs ")]
        return StatusBar(left, right, id="fm:status")

    def _view(self):
        rows, nodes = self._visible()
        selected_path = rows[nodes.index(self.cursor)]["path"]
        tree = Tree(
            rows,
            id="fm:tree",
            selected_path=selected_path,
            has_focus=True,
            style=TREE_STYLE,
            on_row_click=self._on_row_click,
            on_toggle_expand=self._on_row_toggle_expand,
        )
        body = Split(tree, self._details_pane(), id="fm:split", ratio=BODY_SPLIT_RATIO)
        chrome = SidebarPanel(body, id="fm:chrome", toolbar=TOOLBAR)
        h = max(self.height, 2)
        return Split(
            chrome,
            self._status_leaf(),
            id="fm:outer",
            orientation="vertical",
            ratio=(h - 1) / h,
            first_min=3.0,
            second_min=1.0,
        )

    def _on_event(self, event) -> None:
        """Only truly global input lands here: every click/row-activation
        the view's own node callbacks above already claim never reaches
        this at all (`App._dispatch` returns `True` for those)."""
        name = key_name(event)
        if name is None:
            return
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


class _Quit(Exception):
    """Raised from `_on_event` to unwind out of `run_tui()`/`run_native()`."""


def _wrap_quit(fm: FileManager) -> None:
    inner = fm._on_event

    def on_event(event) -> None:
        if key_name(event) in ("q", "Escape"):
            raise _Quit
        inner(event)

    fm.app.set_on_event(on_event)

UP_KEY = {"Named": "Up"}
DOWN_KEY = {"Named": "Down"}
LEFT_KEY = {"Named": "Left"}
RIGHT_KEY = {"Named": "Right"}
ENTER_KEY = {"Named": "Enter"}


def _key_event(key) -> dict:
    return {"KeyPressed": {"key": key, "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False}, "repeat": False}}


def _run_headless(fm: FileManager) -> None:
    """Expand a folder, open it as a tab, drill into a subfolder from its
    details table, then print the final screen."""
    steps = [
        _key_event(DOWN_KEY),
        _key_event(RIGHT_KEY),
        _key_event(ENTER_KEY),
        {"DataTable": ["fm:grid:0", {"RowDoubleClicked": {"idx": 0}}]},
    ]
    for event in steps:
        fm.app.push_event(event)
        fm.app.step()
    print(fm.app.screen())
    open_names = [n.name for n in fm.open]
    tree_state = fm.app.widget_state("fm:tree") or {}
    split_state = fm.app.widget_state("fm:split") or {}
    tabs_state = fm.app.widget_state("fm:tabs") or {}
    print(
        f"\ncursor={fm.cursor.name}  open_tabs={open_names}  "
        f"active_tab={tabs_state.get('active_tab')}  "
        f"tree_scroll_offset={tree_state.get('scroll_offset')}  "
        f"split_ratio={split_state.get('ratio')}"
    )


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    root = Path(args[0]) if args else Path.cwd()

    fm = FileManager(root.resolve())

    if "--tui" in flags:
        _wrap_quit(fm)
        try:
            fm.app.run_tui()
        except _Quit:
            pass
    elif "--native" in flags:
        _wrap_quit(fm)
        try:
            fm.app.run_native(title="quadraui file manager")
        except _Quit:
            pass
    else:
        _run_headless(fm)
