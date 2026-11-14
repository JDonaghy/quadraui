"""Typed node constructors: small functions that build a `Node` -- a tiny,
backend-agnostic description of one layout-tree element -- instead of a raw
wire dict. Each constructor takes the same fields the matching
`quadraui.wire` `TypedDict` names, plus `on_*` callback keywords for the
`UiEvent` shapes that element can raise (see `quadraui.app.runtime` for how
those get dispatched). `Node.build` closes over everything except its
children, so `quadraui.app.runtime._walk` only has to resolve children and
an id before calling it.

A constructor that produces a `TypedDict` your type checker already knows
(`ButtonDescriptor`, `TreeViewDescriptor`, ...) is deliberately *not*
re-validated here -- passing a malformed `rows`/`columns`/`style` fails the
same way it always has, at the native extension's own decode step.
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable, Dict, List, Literal, Optional, Sequence

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module (no backing `.py`) -- see its own
    # module docstring -- so this import only runs under a type checker.
    from quadraui.wire import (
        Color,
        ColumnDescriptor,
        DataRowDescriptor,
        Icon,
        StatusBarSegment,
        TabBarSegmentDescriptor,
        TextDisplayLineDescriptor,
        ToolbarButtonDescriptor,
        ToolbarDescriptor,
        TreeRowDescriptor,
        TreeStyle,
    )

__all__ = [
    "Node",
    "Split",
    "Column",
    "Row",
    "SidebarPanel",
    "Scroll",
    "Tab",
    "Tabs",
    "Tree",
    "Table",
    "StatusBar",
    "TextDisplay",
    "Button",
    "Toolbar",
]

# A renderable value: a `Node`, a `Component` (duck-typed -- `quadraui.app`
# imports both from the same place, but `nodes.py` itself never imports
# `component.py`, so there is no import cycle), a raw wire dict/`None`
# (passed through unchanged, the escape hatch into the lower-level API), or
# a list of any of those (a repeated child slot, e.g. `Tabs`' own `tabs`).
Renderable = Any

# `TreeViewDescriptor.style` is a required field on the wire (unlike every
# other layout knob this layer leaves optional) -- used whenever `Tree(...)`
# doesn't pass its own `style=`.
_DEFAULT_TREE_STYLE = {
    "indent": 2,
    "show_chevrons": True,
    "chevron_expanded": "▾",
    "chevron_collapsed": "▸",
}


@dataclass
class Node:
    """One layout-tree element awaiting an id and a place in the tree.
    `kind` is cosmetic (debugging only); `children` maps each named child
    slot this node has (`"first"`/`"second"` for `Split`, `"tabs"` for
    `Tabs`, ...) to whatever `Renderable` was passed in. `callbacks` names
    the `on_*` keywords this node's own constructor was given, by the same
    name `quadraui.app.runtime`'s event-dispatch table looks them up under.
    `build(resolved_id, resolved_children)` returns the wire dict: every
    constructor below supplies its own closure, so a `Node` itself never
    needs to know the wire shape of anything but its own children slots.
    """

    kind: str
    id: Optional[str]
    callbacks: Dict[str, Callable[..., None]]
    children: Dict[str, Renderable]
    build: Callable[[str, Dict[str, Any]], Dict[str, Any]]


def _callbacks(**named: Optional[Callable[..., None]]) -> Dict[str, Callable[..., None]]:
    return {name: fn for name, fn in named.items() if fn is not None}


# --- composition: split / column / row / sidebar_panel / scroll / tabs -----


def Split(
    first: Renderable,
    second: Renderable,
    *,
    id: Optional[str] = None,
    orientation: str = "horizontal",
    ratio: float = 0.5,
    first_min: Optional[float] = None,
    second_min: Optional[float] = None,
) -> Node:
    """A resizable two-pane divider. `ratio` is read back as *uncontrolled*
    from the second frame on as long as every render resends the same
    literal -- a user drag then persists in the framework's own
    widget-state store with no state this layer needs to track; see
    `QuiApp.widget_state`'s own doc."""

    def build(node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "id": node_id,
            "orientation": orientation,
            "ratio": ratio,
            "first": children["first"],
            "second": children["second"],
        }
        if first_min is not None:
            payload["first_min"] = first_min
        if second_min is not None:
            payload["second_min"] = second_min
        return {"split": payload}

    return Node("split", id, {}, {"first": first, "second": second}, build)


def Column(*items: Renderable, padding: Any = None, margin: Optional[float] = None, gap: Optional[float] = None) -> Node:
    """A vertical stack. Unaddressable on the wire (`LayoutColumnNode` has
    no `id` of its own), so it never takes callbacks."""

    def build(_node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        wire: Dict[str, Any] = {"column": children["items"]}
        if padding is not None:
            wire["padding"] = padding
        if margin is not None:
            wire["margin"] = margin
        if gap is not None:
            wire["gap"] = gap
        return wire

    return Node("column", None, {}, {"items": list(items)}, build)


def Row(*items: Renderable, padding: Any = None, margin: Optional[float] = None, gap: Optional[float] = None) -> Node:
    """A horizontal stack -- `Column`'s sibling, same unaddressable shape."""

    def build(_node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        wire: Dict[str, Any] = {"row": children["items"]}
        if padding is not None:
            wire["padding"] = padding
        if margin is not None:
            wire["margin"] = margin
        if gap is not None:
            wire["gap"] = gap
        return wire

    return Node("row", None, {}, {"items": list(items)}, build)


def SidebarPanel(
    content: Renderable,
    *,
    id: Optional[str] = None,
    toolbar: Optional["ToolbarDescriptor"] = None,
    toolbar_height: Optional[float] = None,
) -> Node:
    """A reserved-top-slot chrome wrapper -- `toolbar` is a raw
    `ToolbarDescriptor` (build one with `Toolbar(...)`), not a child `Node`:
    it paints above `content` but isn't itself a layout-tree child."""

    def build(node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        payload: Dict[str, Any] = {"id": node_id, "content": children["content"]}
        if toolbar is not None:
            payload["toolbar"] = toolbar
        if toolbar_height is not None:
            payload["toolbar_height"] = toolbar_height
        return {"sidebar_panel": payload}

    return Node("sidebar_panel", id, {}, {"content": content}, build)


def Scroll(
    content: Renderable,
    *,
    id: Optional[str] = None,
    axis: Literal["vertical", "horizontal"] = "vertical",
    offset: Optional[float] = None,
    padding: Any = None,
    margin: Optional[float] = None,
) -> Node:
    """A scrollable viewport around a single `content` subtree, built on
    `quadraui::ScrollView`. `axis` picks which dimension scrolls
    (`"vertical"`/`"horizontal"`); `offset` is the same controlled/
    uncontrolled surface-pixel scroll position `scroll_offset` is
    elsewhere -- omit it to let the framework own the position across
    frames (mouse wheel, PageUp/PageDown, a scrollbar drag), or pass it on
    every render to pin it from the app's own state. `padding`/`margin`
    inset the viewport before `content` is measured/painted; there's no
    `gap` -- a scroll has only one content subtree, nothing to space it
    from."""

    def build(node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        payload: Dict[str, Any] = {"id": node_id, "content": children["content"]}
        if axis != "vertical":
            payload["axis"] = axis
        if offset is not None:
            payload["offset"] = offset
        wire: Dict[str, Any] = {"scroll": payload}
        if padding is not None:
            wire["padding"] = padding
        if margin is not None:
            wire["margin"] = margin
        return wire

    return Node("scroll", id, {}, {"content": content}, build)


@dataclass
class _TabSpec:
    content: Renderable
    title: str
    id: Optional[str] = None
    closable: Optional[bool] = None
    is_dirty: Optional[bool] = None
    is_preview: Optional[bool] = None


def Tab(
    content: Renderable,
    *,
    title: str,
    id: Optional[str] = None,
    closable: Optional[bool] = None,
    is_dirty: Optional[bool] = None,
    is_preview: Optional[bool] = None,
) -> _TabSpec:
    """One `Tabs(...)` entry. `id` defaults to `"<tabs_id>:tab:<index>"` if
    omitted -- stable across re-render as long as this tab's position among
    its siblings doesn't change, the same rule `Node`'s own auto-id follows."""
    return _TabSpec(content, title, id, closable, is_dirty, is_preview)


def Tabs(
    tabs: Sequence[_TabSpec],
    *,
    id: Optional[str] = None,
    closable: Optional[bool] = None,
    right_segments: Optional[List["TabBarSegmentDescriptor"]] = None,
    active_accent: Optional["Color"] = None,
    compact: Optional[bool] = None,
    on_close: Optional[Callable[[int], None]] = None,
    on_activate: Optional[Callable[[int], None]] = None,
) -> Node:
    """A closable tab strip. Which tab is *active* is framework-owned
    widget state (a click already applies `TabActivated` before this
    layer's own `on_activate` would even see it) -- `on_activate` exists for
    the rarer case an app wants to react to the switch itself (logging,
    say), not to pin the index back into this node; see `Node` callbacks
    for how it is looked up. `on_close` fires on the tab's own `TabClosed`;
    this node never removes the tab itself -- the caller still owns `tabs`."""

    def build(node_id: str, children: Dict[str, Any]) -> Dict[str, Any]:
        wired_tabs = []
        for i, (spec, content_wire) in enumerate(zip(tabs, children["tabs"])):
            entry: Dict[str, Any] = {
                "id": spec.id or f"{node_id}:tab:{i}",
                "title": spec.title,
                "content": content_wire,
            }
            if spec.closable is not None:
                entry["closable"] = spec.closable
            if spec.is_dirty is not None:
                entry["is_dirty"] = spec.is_dirty
            if spec.is_preview is not None:
                entry["is_preview"] = spec.is_preview
            wired_tabs.append(entry)
        payload: Dict[str, Any] = {"id": node_id, "tabs": wired_tabs}
        if closable is not None:
            payload["closable"] = closable
        if right_segments is not None:
            payload["right_segments"] = right_segments
        if active_accent is not None:
            payload["active_accent"] = active_accent
        if compact is not None:
            payload["compact"] = compact
        return {"tabs": payload}

    callbacks = _callbacks(on_close=on_close, on_activate=on_activate)
    children = {"tabs": [spec.content for spec in tabs]}
    return Node("tabs", id, callbacks, children, build)


# --- leaves -------------------------------------------------------------


def Tree(
    rows: List["TreeRowDescriptor"],
    *,
    id: Optional[str] = None,
    selection_mode: str = "Single",
    selected_path: Optional[List[int]] = None,
    has_focus: bool = False,
    style: Optional["TreeStyle"] = None,
    on_row_click: Optional[Callable[[List[int]], None]] = None,
    on_toggle_expand: Optional[Callable[[List[int]], None]] = None,
    on_activate: Optional[Callable[[List[int]], None]] = None,
    on_select: Optional[Callable[[List[int]], None]] = None,
) -> Node:
    """A `TreeRowDescriptor` list. `on_row_click` fires on `RowClicked`
    (a row body click -- select it); `on_toggle_expand` on
    `RowToggleExpand` (the chevron); `on_activate` on `RowDoubleClicked`;
    `on_select` on `SelectionChanged`. Scroll is framework-owned; nothing
    here ever needs to read or write it."""

    def build(node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "id": node_id,
            "rows": rows,
            "selection_mode": selection_mode,
            "selected_path": selected_path if selected_path is not None else [],
            "has_focus": has_focus,
            "style": style if style is not None else _DEFAULT_TREE_STYLE,
        }
        return {"id": node_id, "tree": payload}

    callbacks = _callbacks(
        on_row_click=on_row_click,
        on_toggle_expand=on_toggle_expand,
        on_activate=on_activate,
        on_select=on_select,
    )
    return Node("tree", id, callbacks, {}, build)


def Table(
    columns: List["ColumnDescriptor"],
    rows: List["DataRowDescriptor"],
    *,
    id: Optional[str] = None,
    on_activate: Optional[Callable[[int], None]] = None,
    on_select: Optional[Callable[[int], None]] = None,
    on_header_click: Optional[Callable[[int], None]] = None,
) -> Node:
    """A `DataTableDescriptor`. `on_activate` fires on either
    `RowDoubleClicked` or `RowActivated` (a mouse double-click and an
    Enter-to-activate both mean "open this row"); `on_select` on
    `RowSelected`; `on_header_click` on `HeaderClicked`."""

    def build(node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": node_id,
            "data_table": {"id": node_id, "columns": columns, "rows": rows},
        }

    callbacks = _callbacks(
        on_activate=on_activate,
        on_select=on_select,
        on_header_click=on_header_click,
    )
    return Node("data_table", id, callbacks, {}, build)


def StatusBar(
    left_segments: List["StatusBarSegment"],
    right_segments: List["StatusBarSegment"],
    *,
    id: Optional[str] = None,
    on_segment_click: Optional[Callable[[str], None]] = None,
) -> Node:
    """A `StatusBarDescriptor`. `on_segment_click` fires on `SegmentClicked`,
    with that segment's own `id`."""

    def build(node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": node_id,
            "status_bar": {
                "id": node_id,
                "left_segments": left_segments,
                "right_segments": right_segments,
            },
        }

    callbacks = _callbacks(on_segment_click=on_segment_click)
    return Node("status_bar", id, callbacks, {}, build)


def TextDisplay(lines: List["TextDisplayLineDescriptor"], *, id: Optional[str] = None) -> Node:
    """A `TextDisplayDescriptor` leaf -- plain read-only text."""

    def build(node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        return {"id": node_id, "text_display": {"id": node_id, "lines": lines}}

    return Node("text_display", id, {}, {}, build)


def Button(
    label: str,
    *,
    id: Optional[str] = None,
    on_click: Optional[Callable[[], None]] = None,
    variant: Optional[str] = None,
    icon: Optional["Icon"] = None,
    tooltip: Optional[str] = None,
    focused: Optional[bool] = None,
    disabled: Optional[bool] = None,
) -> Node:
    """A `ButtonDescriptor` leaf. `quadraui` never turns a raw `MouseDown`
    into "this button was clicked" on its own (there is no dedicated
    `ButtonPressed` `UiEvent`) -- it only hit-tests the click and attaches
    *which* button it landed on (`widget` plus a `button_body` `hit`). So
    `on_click` is dispatched by `quadraui.app.runtime.App` itself, from
    whichever of the two real signals a backend actually sends: that
    hit-tested `MouseDown`, or an `Enter`/`Space` `KeyPressed` while this
    button is focused (the same two paths `examples/tui_interactive_demo.py`
    hand-rolls)."""

    def build(node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        payload: Dict[str, Any] = {"id": node_id, "label": label}
        for key, value in (
            ("variant", variant),
            ("icon", icon),
            ("tooltip", tooltip),
            ("focused", focused),
            ("disabled", disabled),
        ):
            if value is not None:
                payload[key] = value
        return {"id": node_id, "button": payload}

    callbacks = _callbacks(on_click=on_click)
    return Node("button", id, callbacks, {}, build)


_toolbar_auto_ids = itertools.count()


def Toolbar(buttons: List["ToolbarButtonDescriptor"], *, id: Optional[str] = None) -> "ToolbarDescriptor":
    """A raw `ToolbarDescriptor` -- not a `Node`: every place a toolbar can
    go (`SidebarPanel`'s own `toolbar=`, a `LayoutLeafNode`'s `"toolbar"`
    key) wants the descriptor dict directly, and the bindings don't yet
    route toolbar button clicks to an event (see `ButtonDescriptor`'s own
    doc on why `on_click` lives on `Button`, not here).

    Because this result never passes through `Node`/`_walk`, omitting
    `id=` can't fall back to a tree-position auto-id the way every other
    constructor here does -- there is no tree position to derive it from.
    It falls back to a call-order counter instead, stable only as long as
    this call site runs the same number of times, in the same order, on
    every render. That holds for the common case (build one `Toolbar(...)`
    once, e.g. as a module-level constant reused every frame); give an
    explicit `id=` if constructing a fresh one inside `view()` itself."""
    resolved_id = id if id is not None else f"auto:toolbar:{next(_toolbar_auto_ids)}"
    return {"id": resolved_id, "buttons": buttons}
