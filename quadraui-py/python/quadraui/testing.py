"""App-author test harness: drive a `quadraui` app from a test the way a
user drives it, without writing a single wire event by hand.

A test built on this module says what a person would say -- *find the
tree, click the row labelled "alpha", press Enter, assert "a1.txt" is on
screen* -- and the harness turns each of those into the real
`event -> handle -> render` round trip the headless runner already
performs. Nothing here is a second rendering path or a mock: every query
reads the descriptors the app actually rendered, and every synthesised
action is the same `UiEvent` the engine itself would deliver.

    from quadraui.testing import harness

    h = harness(app)
    h.find(role="tree").row("alpha").click()
    h.press("Enter")
    h.assert_text("a1.txt")

**Querying.** `find`/`find_all`/`query` take any combination of `id=`,
`role=` (the wire kind key: `"tree"`, `"data_table"`, `"tabs"`,
`"button"`, ...) and `text=` (a substring of everything that widget
renders). They walk the widget tree the last render produced, in
document order, and by default skip whatever is not on screen -- the
content of an inactive tab is there in the wire tree but invisible to a
user, so it is invisible to a query too unless `include_hidden=True`.
`Widget.find`/`Widget.find_all` run the same search over one subtree.

**Acting.** Row, tab and divider actions are synthesised from the
descriptor the app rendered, so a test names a row by its text instead of
by a path or a pixel: `tree.row("alpha").toggle()`,
`table.row(index=1).double_click()`, `tabs[0].close()`,
`split.drag_divider(0.6)`. Keyboard input is `h.press("Down")` /
`h.type("hello")` globally, or `widget.press("Down")` for the
focus-scoped shape a real backend delivers to a focused widget. A button
is clicked through the engine's own hit test: `h.click_text("Refresh")`
(and `button.click()`) finds the label on the rendered frame and pushes a
`MouseDown` at those coordinates, so the test never hardcodes one.

**Asserting.** `h.screen()` is the rendered frame's text,
`h.contains(...)` a substring check, and `h.assert_text(...)` /
`h.assert_not_text(...)` the same checks with the whole frame printed in
the failure message. Widget-level state the app itself does not track
(scroll offset, active tab, split ratio) reads off `widget.state`.

**Staleness.** A `Widget`/`Row`/`Tab` is a snapshot of one frame. Any
action re-renders, so re-query after acting rather than reusing a handle
across a state change; an action on a stale handle still dispatches (it
carries only an id and an index), but its `text`/`descriptor` describe
the older frame.

The harness wraps a `quadraui.app.App` directly. For a plain `QuiApp` it
needs the same `render` callable the app gave the `QuiApp`, since there
is no way to read a registered callback back out:

    h = harness(qui_app, render=my_render)
"""

from __future__ import annotations

import re
from typing import Any, Callable, Dict, Iterator, List, Optional, Sequence, Tuple, Union

__all__ = [
    "Harness",
    "Row",
    "Tab",
    "ToolbarButton",
    "Widget",
    "WidgetNotFound",
    "harness",
]


class WidgetNotFound(AssertionError):
    """No widget matched a `find()`. An `AssertionError` so an unmatched
    query reads as a failed test, not as a harness crash."""


# Layout nodes that hold other nodes rather than painting a primitive; every
# other non-meta key on a node names the primitive kind it paints.
_CONTAINER_ROLES = frozenset({"row", "column", "split", "tabs", "panel", "sidebar_panel", "split_tree"})

# Keys a layout node may carry alongside its one role key.
_META_KEYS = frozenset({"id", "padding", "margin", "gap"})

# Descriptor fields whose string values are text a user can read. `glyph` and
# `fallback` (an icon and its ASCII stand-in) are deliberately absent: they are
# decoration, and matching `text=` against them would make `find(text="/")`
# hit every folder row.
_TEXT_KEYS = frozenset({"text", "label", "title", "placeholder", "fallback_text"})

# The event wrapper each focusable role's own events arrive under.
_EVENT_SCOPES = {
    "tree": "Tree",
    "data_table": "DataTable",
    "list": "List",
    "tabs": "TabBar",
    "tab_bar": "TabBar",
    "status_bar": "StatusBar",
}

_NAMED_KEYS = frozenset(
    {
        "Escape",
        "Tab",
        "BackTab",
        "Enter",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Up",
        "Down",
        "Left",
        "Right",
        "CapsLock",
        "NumLock",
        "ScrollLock",
        "Menu",
    }
)

_FUNCTION_KEY = re.compile(r"^[fF]([1-9][0-9]?)$")


def _key_payload(key: str) -> Any:
    """The wire `Key` for a key named the way a person says it: `"Down"`,
    `"Enter"`, `"F2"`, or any single character (`"a"`, `" "`)."""
    match = _FUNCTION_KEY.match(key)
    if match is not None:
        return {"Named": {"F": int(match.group(1))}}
    canonical = {name.lower(): name for name in _NAMED_KEYS}.get(key.lower())
    if canonical is not None:
        return {"Named": canonical}
    if len(key) == 1:
        return {"Char": key}
    raise ValueError(f"not a key name: {key!r} (expected a named key, F1-F99, or one character)")


def _modifiers(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Dict[str, bool]:
    """The wire `Modifiers`; `meta` is the portable name for the key the
    wire calls `cmd` (Command on macOS, Super elsewhere)."""
    return {"shift": shift, "ctrl": ctrl, "alt": alt, "cmd": meta}


def _text_of(value: Any) -> str:
    """Every readable string inside a descriptor fragment, in render order,
    space-separated -- a `StyledText`'s spans, a row's cells, a status
    bar's segments, a button's label, all the same way."""
    out: List[str] = []
    _collect_text(value, out)
    return " ".join(part for part in out if part)


def _collect_text(value: Any, out: List[str]) -> None:
    if isinstance(value, str):
        out.append(value)
        return
    if isinstance(value, list):
        for item in value:
            _collect_text(item, out)
        return
    if not isinstance(value, dict):
        return
    for key, item in value.items():
        if key in _TEXT_KEYS:
            _collect_text(item, out)
        elif isinstance(item, (list, dict)):
            _collect_text(item, out)


class Row:
    """One row of a tree, table or list, named by what it renders rather
    than by its index or tree path. Every action is the `UiEvent` the
    engine delivers when a user does that to this row."""

    def __init__(self, widget: "Widget", index: int, descriptor: Dict[str, Any]) -> None:
        self.widget = widget
        self.index = index
        self.descriptor = descriptor

    @property
    def path(self) -> Optional[List[int]]:
        """The tree path of this row, or `None` for a flat table/list row."""
        return self.descriptor.get("path")

    @property
    def text(self) -> str:
        """Everything this row renders, cells and all, space-separated."""
        return _text_of(self.descriptor)

    @property
    def cells(self) -> List[str]:
        """One string per table column; a tree/list row renders a single
        cell, so it reports a one-element list."""
        cells = self.descriptor.get("cells")
        if cells is None:
            return [_text_of(self.descriptor.get("text"))]
        return [_text_of(cell) for cell in cells]

    @property
    def expanded(self) -> bool:
        """Whether a tree row is currently showing its children."""
        return bool(self.descriptor.get("is_expanded", False))

    def click(self) -> None:
        """A single click on the row body: selects, never opens."""
        self._act("click")

    def select(self) -> None:
        """Moves the selection onto this row, the way an arrow key does."""
        self._act("select")

    def double_click(self) -> None:
        """A double click: whatever "open this" means for this app."""
        self._act("double_click")

    def activate(self) -> None:
        """Activates the row without a mouse -- Enter on a focused list or
        table delivers this where a double click delivers `double_click`."""
        self._act("activate")

    def toggle(self) -> None:
        """Clicks a tree row's expand/collapse chevron, which expands or
        collapses in place and leaves the cursor where it was."""
        self._act("toggle")

    def _act(self, verb: str) -> None:
        """Each role renders rows differently and reports them differently:
        a tree addresses a row by `path` and a flat table/list by `idx`,
        and each has its own name for "the user opened this". `_ROW_EVENTS`
        is that table, one entry per (role, verb)."""
        shapes = _ROW_EVENTS.get(self.widget.role)
        variant = None if shapes is None else shapes.get(verb)
        if variant is None:
            raise TypeError(f"a {self.widget.role!r} row has no {verb!r} action")
        name, payload = variant
        self.widget._send_scoped({name: payload(self)})

    def __repr__(self) -> str:
        return f"<Row {self.index} of {self.widget.role} {self.widget.id!r}: {self.text!r}>"


def _by_path(row: Row) -> Dict[str, Any]:
    return {"path": row.path}


def _by_index(row: Row) -> Dict[str, Any]:
    return {"idx": row.index}


# (role, verb) -> the `UiEvent` variant the engine delivers for it.
_ROW_EVENTS: Dict[str, Dict[str, Tuple[str, Callable[[Row], Dict[str, Any]]]]] = {
    "tree": {
        "click": ("RowClicked", lambda row: {"path": row.path, "modifiers": _modifiers(False, False, False, False)}),
        "select": ("SelectionChanged", _by_path),
        "double_click": ("RowDoubleClicked", _by_path),
        "activate": ("RowDoubleClicked", _by_path),
        "toggle": ("RowToggleExpand", _by_path),
    },
    "data_table": {
        "click": ("RowSelected", _by_index),
        "select": ("RowSelected", _by_index),
        "double_click": ("RowDoubleClicked", _by_index),
        "activate": ("RowActivated", _by_index),
    },
    "list": {
        "click": ("SelectionChanged", _by_index),
        "select": ("SelectionChanged", _by_index),
        "double_click": ("ItemDoubleClicked", _by_index),
        "activate": ("ItemActivated", _by_index),
    },
}


class Tab:
    """One tab of a `tabs` node: its title, the widget tree it holds, and
    the click/close a user performs on its header."""

    def __init__(self, widget: "Widget", index: int, descriptor: Dict[str, Any], content: Optional["Widget"]) -> None:
        self.widget = widget
        self.index = index
        self.descriptor = descriptor
        self.content = content

    @property
    def title(self) -> str:
        return _text_of(self.descriptor.get("title"))

    @property
    def active(self) -> bool:
        return self.index == self.widget.active_index

    def click(self) -> None:
        """Clicks the tab header, making this the shown tab."""
        self.widget._send_scoped({"TabActivated": {"index": self.index}})

    def close(self) -> None:
        """Clicks the tab's close affordance."""
        self.widget._send_scoped({"TabClosed": {"index": self.index}})

    def __repr__(self) -> str:
        return f"<Tab {self.index} {self.title!r}{' active' if self.active else ''}>"


class ToolbarButton:
    """One button of a toolbar strip, clicked through the engine's own hit
    test against the label as it painted."""

    def __init__(self, harness: "Harness", toolbar_id: Optional[str], index: int, button_id: str, label: str) -> None:
        self.harness = harness
        self.toolbar_id = toolbar_id
        self.index = index
        self.id = button_id
        self.label = label

    def click(self) -> None:
        self.harness.click_text(self.label)

    def __repr__(self) -> str:
        return f"<ToolbarButton {self.id!r}: {self.label!r}>"


class Widget:
    """One node of the tree the app's last render produced: its wire `id`,
    its `role` (the primitive kind it paints, or the layout kind it holds
    other widgets in), the text it renders, and the actions a user can
    perform on it.

    Read any descriptor field the app declared with `widget["field"]`, and
    the framework-owned state the app does *not* track (scroll offset,
    active tab, split ratio, selection) with `widget.state`."""

    def __init__(
        self,
        harness: "Harness",
        role: str,
        descriptor: Any,
        *,
        node_id: Optional[str],
        visible: bool,
    ) -> None:
        self._harness = harness
        self.role = role
        self.descriptor = descriptor
        self.id = node_id
        self.visible = visible
        self.children: List["Widget"] = []
        #: Which tab of an enclosing `tabs` node holds this widget, if any.
        self._tab_index: Optional[int] = None

    # --- reading -------------------------------------------------------------

    @property
    def text(self) -> str:
        """Everything this widget and its children render, space-separated."""
        own = _text_of(self.descriptor) if not isinstance(self.descriptor, list) else ""
        parts = [own] + [child.text for child in self.children]
        return " ".join(part for part in parts if part)

    def text_of(self, field: str) -> str:
        """The readable text of one descriptor field -- a status bar's
        `"left_segments"`, a tree row's `"text"`, and so on."""
        if not isinstance(self.descriptor, dict):
            return ""
        return _text_of(self.descriptor.get(field))

    def __getitem__(self, field: str) -> Any:
        if not isinstance(self.descriptor, dict):
            raise KeyError(field)
        return self.descriptor[field]

    def get(self, field: str, default: Any = None) -> Any:
        if not isinstance(self.descriptor, dict):
            return default
        return self.descriptor.get(field, default)

    @property
    def state(self) -> Dict[str, Any]:
        """The engine's own stored state for this widget id -- scroll
        offset, active tab, split ratio, selection -- empty when the engine
        is tracking nothing for it yet."""
        if self.id is None:
            return {}
        return self._harness.state(self.id)

    @property
    def scroll_offset(self) -> int:
        """How far this widget has scrolled, from the engine's own store."""
        return int(self.state.get("scroll_offset") or 0)

    @property
    def ratio(self) -> Optional[float]:
        """A split's divider position as the engine resolved it -- which is
        where an uncontrolled divider's dragged position actually lives,
        not in the `"ratio"` the app keeps re-declaring."""
        value = self.state.get("ratio")
        return None if value is None else float(value)

    # --- rows ----------------------------------------------------------------

    @property
    def rows(self) -> List[Row]:
        """Every row this widget renders, in render order."""
        if not isinstance(self.descriptor, dict):
            return []
        rows = self.descriptor.get("rows")
        if rows is None:
            rows = self.descriptor.get("items")
        if rows is None:
            raise TypeError(f"a {self.role!r} widget has no rows")
        return [Row(self, i, row) for i, row in enumerate(rows)]

    @property
    def row_texts(self) -> List[str]:
        """Each row's rendered text, in render order."""
        return [row.text for row in self.rows]

    def row(self, text: Optional[str] = None, *, index: Optional[int] = None, path: Optional[Sequence[int]] = None) -> Row:
        """The row matching `text` (a substring of what it renders), an
        explicit `index`, or a tree `path`. Raises `WidgetNotFound` when
        nothing matches, naming every row that is there."""
        rows = self.rows
        if index is not None:
            if not -len(rows) <= index < len(rows):
                raise WidgetNotFound(f"{self.role} {self.id!r} has {len(rows)} rows, no index {index}")
            return rows[index]
        if path is not None:
            wanted = list(path)
            for row in rows:
                if row.path == wanted:
                    return row
            raise WidgetNotFound(f"no row at path {wanted} in {self.role} {self.id!r}")
        if text is None:
            raise TypeError("row() needs one of text, index or path")
        for row in rows:
            if text in row.text:
                return row
        raise WidgetNotFound(f"no row matching {text!r} in {self.role} {self.id!r}; rows are {self.row_texts}")

    @property
    def selected_row(self) -> Optional[Row]:
        """The row the app rendered as selected (a tree's `selected_path`,
        a table's selected index), or `None` when nothing is."""
        if isinstance(self.descriptor, dict) and "selected_path" in self.descriptor:
            wanted = self.descriptor.get("selected_path")
            for row in self.rows:
                if row.path == wanted:
                    return row
            return None
        selected = self.state.get("selected_idx")
        if selected is None and isinstance(self.descriptor, dict):
            selected = self.descriptor.get("selected_idx")
        if selected is None:
            return None
        rows = self.rows
        return rows[selected] if 0 <= selected < len(rows) else None

    # --- tabs ----------------------------------------------------------------

    @property
    def tabs(self) -> List[Tab]:
        """Every tab of a `tabs`/`tab_bar` node, in render order."""
        if not isinstance(self.descriptor, dict) or "tabs" not in self.descriptor:
            raise TypeError(f"a {self.role!r} widget has no tabs")
        entries = self.descriptor["tabs"]
        contents = {child._tab_index: child for child in self.children if child._tab_index is not None}
        return [Tab(self, i, entry, contents.get(i)) for i, entry in enumerate(entries)]

    @property
    def active_index(self) -> int:
        """Which tab is showing -- from the engine's own store, which is
        where an uncontrolled tab strip's active tab actually lives."""
        state = self.state
        if "active_tab" in state:
            return int(state["active_tab"])
        if isinstance(self.descriptor, dict) and self.descriptor.get("selected") is not None:
            return int(self.descriptor["selected"])
        return 0

    @property
    def active_tab(self) -> Optional[Tab]:
        tabs = self.tabs
        index = self.active_index
        return tabs[index] if 0 <= index < len(tabs) else None

    # --- toolbar -------------------------------------------------------------

    @property
    def buttons(self) -> List[ToolbarButton]:
        """Every action button of a toolbar strip (separators and spacers
        are not buttons and are skipped)."""
        if not isinstance(self.descriptor, dict) or "buttons" not in self.descriptor:
            raise TypeError(f"a {self.role!r} widget has no buttons")
        out: List[ToolbarButton] = []
        for i, entry in enumerate(self.descriptor["buttons"]):
            if not isinstance(entry, dict):
                continue
            for payload in entry.values():
                if isinstance(payload, dict) and "label" in payload:
                    out.append(ToolbarButton(self._harness, self.id, i, payload.get("id", ""), payload["label"]))
        return out

    @property
    def button_labels(self) -> List[str]:
        return [button.label for button in self.buttons]

    def button(self, label: str) -> ToolbarButton:
        """The toolbar button whose label contains `label`."""
        for button in self.buttons:
            if label in button.label:
                return button
        raise WidgetNotFound(f"no toolbar button matching {label!r}; labels are {self.button_labels}")

    # --- acting ---------------------------------------------------------------

    def click(self) -> None:
        """Clicks this widget through the engine's own hit test, by finding
        what it painted on the frame. Defined for widgets a user clicks as
        a whole (a button); click a row or a tab through `row(...)`/
        `tabs[...]` instead, which carry their own position."""
        if self.role != "button":
            raise TypeError(
                f"a {self.role!r} widget is not clicked as a whole -- use .row(...)/.tabs[...]"
                " for a part of it, or harness.click_text(...) for an exact label"
            )
        label = self.text_of("label") or self.text
        if not label:
            raise WidgetNotFound(f"button {self.id!r} renders no label to click")
        self._harness.click_text(label)

    def focus(self) -> None:
        """Moves focus here, the way Tab does."""
        if self.id is None:
            raise TypeError(f"a {self.role!r} widget has no id to focus")
        self._harness.focus(self.id)

    def press(
        self,
        key: str,
        *,
        shift: bool = False,
        ctrl: bool = False,
        alt: bool = False,
        meta: bool = False,
    ) -> None:
        """Presses a key *at this widget*: the focus-scoped shape a real
        backend delivers to a focused widget, not the plain top-level
        `KeyPressed` `harness.press` sends."""
        scope = _EVENT_SCOPES.get(self.role)
        if scope is None or self.id is None:
            raise TypeError(f"a {self.role!r} widget takes no focus-scoped keys")
        payload = _key_payload(key)
        name = payload.get("Char") or payload["Named"]
        if isinstance(name, dict):
            name = f"F{name['F']}"
        self._harness.dispatch(
            {scope: [self.id, {"KeyPressed": {"key": name, "modifiers": _modifiers(shift, ctrl, alt, meta)}}]}
        )

    def drag_divider(self, ratio: float, *, index: int = 0) -> None:
        """Drags a split's divider to `ratio` of the split's own extent."""
        if self.role not in ("split", "split_tree") or self.id is None:
            raise TypeError(f"a {self.role!r} widget has no divider")
        self._harness.dispatch({"SplitDividerDragged": {"tree": self.id, "split_index": index, "new_ratio": ratio}})

    def _send_scoped(self, activation: Dict[str, Any]) -> None:
        scope = _EVENT_SCOPES.get(self.role)
        if scope is None or self.id is None:
            raise TypeError(f"a {self.role!r} widget delivers no scoped events")
        self._harness.dispatch({scope: [self.id, activation]})

    # --- searching this subtree -----------------------------------------------

    def walk(self, *, include_hidden: bool = False) -> Iterator["Widget"]:
        """This widget and every descendant, in render order."""
        if include_hidden or self.visible:
            yield self
            for child in self.children:
                for found in child.walk(include_hidden=include_hidden):
                    yield found

    def find(self, **query: Any) -> "Widget":
        """`Harness.find`, restricted to this widget's own subtree."""
        return _first(self.walk(include_hidden=query.get("include_hidden", False)), query, self)

    def find_all(self, **query: Any) -> List["Widget"]:
        """`Harness.find_all`, restricted to this widget's own subtree."""
        return _matches(self.walk(include_hidden=query.get("include_hidden", False)), query)

    def __repr__(self) -> str:
        return f"<Widget {self.role} id={self.id!r}>"


def _matches(widgets: Iterator[Widget], query: Dict[str, Any]) -> List[Widget]:
    wanted_id = query.get("id")
    wanted_role = query.get("role")
    wanted_text = query.get("text")
    unknown = set(query) - {"id", "role", "text", "include_hidden"}
    if unknown:
        raise TypeError(f"unknown query keys: {sorted(unknown)} (expected id, role, text, include_hidden)")
    out = []
    for widget in widgets:
        if wanted_id is not None and widget.id != wanted_id:
            continue
        if wanted_role is not None and widget.role != wanted_role:
            continue
        if wanted_text is not None and wanted_text not in widget.text:
            continue
        out.append(widget)
    return out


def _first(widgets: Iterator[Widget], query: Dict[str, Any], scope: Optional[Widget]) -> Widget:
    found = _matches(widgets, query)
    if found:
        return found[0]
    terms = ", ".join(f"{k}={v!r}" for k, v in query.items() if v and k != "include_hidden")
    where = f" under {scope!r}" if scope is not None else ""
    raise WidgetNotFound(f"no widget matching {terms or '<anything>'}{where}")


class Harness:
    """A running app, queried and driven the way a user drives it. Build
    one with `harness(app)` rather than calling this directly."""

    def __init__(self, app: Any, *, render: Optional[Callable[[], Any]] = None) -> None:
        qui = getattr(app, "qui", None)
        if qui is None:
            qui = app
        elif render is None:
            render = app._render
        if render is None:
            raise TypeError(
                "harness(QuiApp) needs the same render callable the app was given "
                "(render=...); a `quadraui.app.App` supplies its own"
            )
        self.app = app
        self.qui = qui
        self._render: Callable[[], Any] = render
        self._wire: Any = None
        self._root: Optional[Widget] = None
        self.qui.set_render(self._capture)
        self.refresh()

    # --- the frame ------------------------------------------------------------

    def _capture(self) -> Any:
        """Stands in for the app's own render callback so every real paint
        hands this harness the wire tree the app just produced -- queries
        then read exactly what painted, with no second render pass."""
        self._wire = self._render()
        self._root = None
        return self._wire

    def _invalidate(self) -> None:
        self._wire = None
        self._root = None

    def refresh(self) -> None:
        """Repaints and re-reads the widget tree. Every action here does
        this already; call it after driving the app some other way."""
        self.qui.screen()
        self._root = None

    def screen(self) -> str:
        """The rendered frame as text, exactly as the terminal shows it."""
        return self.qui.screen()

    def screen_lines(self) -> List[str]:
        return self.screen().splitlines()

    def contains(self, text: str) -> bool:
        """Whether `text` is somewhere on the rendered frame."""
        return self.qui.screen_contains(text)

    def assert_text(self, *texts: str) -> None:
        """Fails, printing the whole frame, unless every `texts` is on it."""
        missing = [text for text in texts if not self.contains(text)]
        if missing:
            raise AssertionError(f"not on screen: {missing}\n--- screen ---\n{self.screen()}")

    def assert_not_text(self, *texts: str) -> None:
        """Fails, printing the whole frame, if any `texts` is on it."""
        present = [text for text in texts if self.contains(text)]
        if present:
            raise AssertionError(f"unexpectedly on screen: {present}\n--- screen ---\n{self.screen()}")

    def state(self, widget_id: str) -> Dict[str, Any]:
        """The engine's own stored state for a widget id, or `{}`."""
        return self.qui.widget_state(widget_id) or {}

    # --- querying -------------------------------------------------------------

    @property
    def root(self) -> Widget:
        """The outermost widget of the last rendered frame."""
        if self._root is None:
            if self._wire is None:
                self.refresh()
            self._root = _build(self, self._wire)
        return self._root

    def widgets(self, *, include_hidden: bool = False) -> List[Widget]:
        """Every widget of the last rendered frame, in render order."""
        return list(self.root.walk(include_hidden=include_hidden))

    def find(
        self,
        *,
        id: Optional[str] = None,  # noqa: A002 - reads as a query key, not a builtin
        role: Optional[str] = None,
        text: Optional[str] = None,
        include_hidden: bool = False,
    ) -> Widget:
        """The first widget matching every given term, in render order.
        Raises `WidgetNotFound` when nothing does."""
        query = {"id": id, "role": role, "text": text, "include_hidden": include_hidden}
        return _first(self.root.walk(include_hidden=include_hidden), query, None)

    def find_all(
        self,
        *,
        id: Optional[str] = None,  # noqa: A002
        role: Optional[str] = None,
        text: Optional[str] = None,
        include_hidden: bool = False,
    ) -> List[Widget]:
        """Every widget matching the given terms, in render order."""
        query = {"id": id, "role": role, "text": text, "include_hidden": include_hidden}
        return _matches(self.root.walk(include_hidden=include_hidden), query)

    def query(self, **terms: Any) -> Optional[Widget]:
        """`find`, but `None` instead of raising when nothing matches."""
        found = self.find_all(**terms)
        return found[0] if found else None

    def exists(self, **terms: Any) -> bool:
        """Whether any widget matches the given terms."""
        return bool(self.find_all(**terms))

    # --- acting ---------------------------------------------------------------

    def dispatch(self, event: Any) -> None:
        """Pushes one raw `UiEvent` and steps the app -- the escape hatch
        for an event shape this harness has no verb for yet. Everything
        else on this class is a nicer name for a call to this."""
        self.qui.push_event(event)
        self.qui.step()
        self._invalidate()

    def press(
        self,
        key: str,
        *,
        shift: bool = False,
        ctrl: bool = False,
        alt: bool = False,
        meta: bool = False,
        repeat: bool = False,
    ) -> None:
        """Presses a key with nothing focus-scoped about it -- the plain
        top-level `KeyPressed` a terminal delivers. Use `widget.press` for
        the shape a focused widget receives."""
        self.dispatch(
            {
                "KeyPressed": {
                    "key": _key_payload(key),
                    "modifiers": _modifiers(shift, ctrl, alt, meta),
                    "repeat": repeat,
                }
            }
        )

    def type(self, text: str) -> None:  # noqa: A003 - the verb a test wants
        """Types `text` one character at a time."""
        for char in text:
            self.press(char)

    def focus(self, target: Union[str, Widget, None]) -> None:
        """Moves focus to a widget (or clears it with `None`)."""
        widget_id = target.id if isinstance(target, Widget) else target
        self.dispatch({"FocusChanged": widget_id})

    def click_at(self, x: float, y: float) -> None:
        """Clicks one cell of the frame; the engine hit-tests it against
        whatever actually painted there. Press *and* release, so a widget
        that paints a pressed state does not stay stuck in it."""
        position = {"x": float(x), "y": float(y)}
        self.dispatch({"MouseDown": {"button": "Left", "position": position, "modifiers": {}}})
        self.dispatch({"MouseUp": {"button": "Left", "position": position}})

    def click_text(self, text: str, *, nth: int = 0) -> None:
        """Clicks the middle of where `text` painted on the frame -- the
        way a user clicks a label, with no coordinate in the test. `nth`
        picks between repeats of the same text, in reading order."""
        found = self.locate_all(text)
        if len(found) <= nth:
            raise WidgetNotFound(
                f"{text!r} appears {len(found)} time(s) on screen, no occurrence {nth}"
                f"\n--- screen ---\n{self.screen()}"
            )
        x, y = found[nth]
        self.click_at(x, y)

    def locate_all(self, text: str) -> List[Tuple[float, float]]:
        """The centre cell of every place `text` painted, in reading
        order. `click_text` is this plus a click."""
        out: List[Tuple[float, float]] = []
        for y, line in enumerate(self.screen_lines()):
            start = line.find(text)
            while start != -1:
                out.append((start + len(text) / 2.0, float(y)))
                start = line.find(text, start + 1)
        return out

    def step(self) -> bool:
        """Pumps one event-loop iteration without pushing anything."""
        result = self.qui.step()
        self._invalidate()
        return result


def harness(app: Any, *, render: Optional[Callable[[], Any]] = None) -> Harness:
    """Wraps a running app for querying and driving from a test. Pass a
    `quadraui.app.App` on its own, or a plain `QuiApp` together with the
    same `render` callable it was constructed with."""
    return Harness(app, render=render)


# --- building the widget tree out of one rendered frame ----------------------


def _role_of(node: Dict[str, Any]) -> Optional[str]:
    roles = [key for key in node if key not in _META_KEYS]
    return roles[0] if len(roles) == 1 else None


def _build(harness: Harness, wire: Any) -> Widget:
    root = Widget(harness, "frame", {}, node_id=None, visible=True)
    _append(harness, root, wire, visible=True)
    if len(root.children) == 1:
        return root.children[0]
    return root


def _append(harness: Harness, parent: Widget, node: Any, *, visible: bool) -> None:
    if node is None:
        return
    if isinstance(node, list):
        for item in node:
            _append(harness, parent, item, visible=visible)
        return
    if not isinstance(node, dict):
        return
    role = _role_of(node)
    if role is None:
        return
    payload = node[role]
    node_id = node.get("id")
    if isinstance(payload, dict):
        node_id = payload.get("id", node_id)
    widget = Widget(harness, role, payload, node_id=node_id, visible=visible)
    parent.children.append(widget)
    if role in _CONTAINER_ROLES:
        _append_children(harness, widget, role, payload, visible=visible)


def _append_children(harness: Harness, widget: Widget, role: str, payload: Any, *, visible: bool) -> None:
    if role in ("row", "column"):
        _append(harness, widget, payload, visible=visible)
    elif role == "split":
        _append(harness, widget, payload.get("first"), visible=visible)
        _append(harness, widget, payload.get("second"), visible=visible)
    elif role in ("panel", "sidebar_panel"):
        toolbar = payload.get("toolbar")
        if toolbar is not None:
            _append(harness, widget, {"toolbar": toolbar}, visible=visible)
        _append(harness, widget, payload.get("content"), visible=visible)
    elif role == "tabs":
        active = widget.active_index
        for index, entry in enumerate(payload.get("tabs", [])):
            before = len(widget.children)
            _append(harness, widget, entry.get("content"), visible=visible and index == active)
            for child in widget.children[before:]:
                child._tab_index = index
    elif role == "split_tree":
        _append_split_tree(harness, widget, payload.get("root"), visible=visible)


def _append_split_tree(harness: Harness, widget: Widget, node: Any, *, visible: bool) -> None:
    if not isinstance(node, dict):
        return
    if "leaf" in node:
        _append(harness, widget, node["leaf"], visible=visible)
        return
    split = node.get("split")
    if isinstance(split, dict):
        _append_split_tree(harness, widget, split.get("first"), visible=visible)
        _append_split_tree(harness, widget, split.get("second"), visible=visible)
