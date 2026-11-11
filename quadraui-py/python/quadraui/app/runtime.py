"""`App`: wraps a `QuiApp` so `view()` returns `Node`/`Component` trees
instead of raw wire dicts, and event dispatch goes through the `on_*`
callbacks those trees carried on their last render instead of one big
`handle(event)` switch.

Walking the tree (`_walk`) does three things in the same pass: resolves
every `Node`'s wire id (explicit `id=`, or a stable auto-id derived from
the node's position in the tree -- see `_auto_id`), expands every
`Component` into whatever its own `view(state)` returns (creating that
component's state slot from `initial_state()` the first time its
`key`/position is seen, and threading the *same* dict back in on every
later render), and records each node's `callbacks` into a fresh
id -> {event_name: callable} registry. `App._handle` looks a dispatched
event's own widget id up in that registry (rebuilt by the render this
event's own `step()` just triggered) and calls whichever callback matches
the event's shape -- see `_CONTAINER_EVENT_MAP` for the event-shape ->
callback-name table, and `Button`'s own doc for why its `on_click` needs
its own two-path dispatch instead of a table entry. `App._render` swaps
in only the component-state keys seen during that same pass, so a
component that stops being rendered (a closed tab, a removed list row)
drops its state slot instead of leaving it behind for a future, unrelated
component to inherit.

A callback mutating app/component state needs no explicit "please
redraw" call: `QuiApp` already repaints once per `step()` whenever a
render callback is registered (see `QuiApp`'s own module doc), so the
mutation a callback makes is already visible by the time that same
`step()` returns.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, Optional, Tuple

from .component import Component
from .nodes import Node

__all__ = ["App", "key_name"]


def _auto_id(path: Tuple[Any, ...]) -> str:
    """A stable id for a `Node` that didn't give its own: deterministic for
    a given structural position, as long as the shape of the tree above it
    (how many siblings, which slot names) doesn't change between renders --
    the same assumption a list index or dict key relies on anywhere else."""
    return "auto:" + ".".join(str(part) for part in path)


def _walk(
    value: Any,
    path: Tuple[Any, ...],
    registry: Dict[str, Dict[str, Callable[..., None]]],
    old_states: Dict[Any, Dict[str, Any]],
    new_states: Dict[Any, Dict[str, Any]],
) -> Any:
    if value is None:
        return None
    if isinstance(value, list):
        return [_walk(item, path + (i,), registry, old_states, new_states) for i, item in enumerate(value)]
    if isinstance(value, Component):
        state_key = (type(value), value.key if value.key is not None else path)
        state = new_states.get(state_key)
        if state is None:
            state = old_states.get(state_key)
            if state is None:
                state = value.initial_state()
            new_states[state_key] = state
        child = value.view(state)
        return _walk(child, path + ("component",), registry, old_states, new_states)
    if isinstance(value, Node):
        node_id = value.id if value.id is not None else _auto_id(path)
        children_wire = {
            slot: _walk(child, path + (slot,), registry, old_states, new_states)
            for slot, child in value.children.items()
        }
        if value.callbacks:
            registry[node_id] = value.callbacks
        return value.build(node_id, children_wire)
    if isinstance(value, dict):
        return value
    raise TypeError(f"not a renderable node: {value!r}")


def _raw_key_name(key: Any) -> Optional[str]:
    """Canonical name of a raw `Key`: a plain top-level `KeyPressed` carries
    a tagged dict (`{"Named": "Down"}`/`{"Char": "o"}`/`{"Named": {"F": 2}}`);
    a focus-scoped leaf event carries the already-stringified name. Both
    collapse to the name (a function key becomes `"F2"`)."""
    if isinstance(key, str):
        return key
    if not isinstance(key, dict):
        return None
    char = key.get("Char")
    if char is not None:
        return char
    named = key.get("Named")
    if named is None:
        return None
    if isinstance(named, dict):
        return f"F{named['F']}"
    return named


def key_name(event: Any) -> Optional[str]:
    """The canonical key name out of a key event in any shape a backend
    delivers one: a plain top-level `KeyPressed`, or a focus-scoped
    `{"<Tree/List/TabBar/...>": [id, {"KeyPressed": ...}]}` -- the focus
    manager rewrites a focused leaf's own keys into that shape before this
    layer ever sees them. Returns `None` for anything else, including a
    bare-string event (`isinstance(event, str)`)."""
    if not isinstance(event, dict):
        return None
    if "KeyPressed" in event:
        return _raw_key_name(event["KeyPressed"]["key"])
    for inner in event.values():
        if isinstance(inner, (list, tuple)) and len(inner) == 2:
            _widget_id, activation = inner
            if isinstance(activation, dict) and "KeyPressed" in activation:
                return _raw_key_name(activation["KeyPressed"]["key"])
    return None


_ACTIVATE_KEY_NAMES = ("Enter", " ")


def _is_activate_key(name: Optional[str]) -> bool:
    return name in _ACTIVATE_KEY_NAMES


# Maps a container-event wrapper's own key (`event["Tree"]`, `event["DataTable"]`,
# ...) to {variant_name: (callback_name, args_from_payload)} -- the shape
# every typed leaf node constructor's own `on_*` keyword documents.
_CONTAINER_EVENT_MAP: Dict[str, Dict[str, Tuple[str, Callable[[dict], tuple]]]] = {
    "Tree": {
        "RowClicked": ("on_row_click", lambda p: (p["path"],)),
        "RowToggleExpand": ("on_toggle_expand", lambda p: (p["path"],)),
        "RowDoubleClicked": ("on_activate", lambda p: (p["path"],)),
        "SelectionChanged": ("on_select", lambda p: (p["path"],)),
    },
    "DataTable": {
        "RowDoubleClicked": ("on_activate", lambda p: (p["idx"],)),
        "RowActivated": ("on_activate", lambda p: (p["idx"],)),
        "RowSelected": ("on_select", lambda p: (p["idx"],)),
        "HeaderClicked": ("on_header_click", lambda p: (p["col"],)),
    },
    "TabBar": {
        "TabClosed": ("on_close", lambda p: (p["index"],)),
        "TabActivated": ("on_activate", lambda p: (p["index"],)),
    },
    "StatusBar": {
        "SegmentClicked": ("on_segment_click", lambda p: (p["id"],)),
    },
}


class App:
    """A `QuiApp` driven by a `view()` callback returning `Node`/`Component`
    trees, with events dispatched to the `on_*` callbacks those trees
    carried on their last render rather than a hand-written `handle`.

    `on_event`, if given, is still called for every event this layer's own
    dispatch doesn't claim (a global key, an unrecognized shape, ...) --
    the escape hatch a keyboard-driven app (arrow keys moving a cursor with
    no single widget "owning" them) still needs; see `key_name` for
    pulling a key name out of whatever shape it arrived in.
    """

    def __init__(
        self,
        width: int,
        height: int,
        *,
        view: Callable[[], Any],
        on_event: Optional[Callable[[Any], None]] = None,
    ) -> None:
        from quadraui import QuiApp  # local import: avoids a cycle with quadraui/__init__.py

        self._view = view
        self._on_event = on_event
        self._registry: Dict[str, Dict[str, Callable[..., None]]] = {}
        self._component_states: Dict[Any, Dict[str, Any]] = {}
        self._focused_id: Optional[str] = None
        self.qui = QuiApp(width, height, render=self._render)
        self.qui.set_handler(self._handle)
        self.actions = self.qui.actions()

    def set_on_event(self, on_event: Optional[Callable[[Any], None]]) -> None:
        """Replaces the global `on_event` fallback given at construction (or
        clears it, with `None`) -- the public way to compose/swap the
        escape-hatch handler after the fact, instead of assigning the
        private `_on_event` attribute directly."""
        self._on_event = on_event

    # --- render / dispatch ---------------------------------------------------

    def _render(self) -> Any:
        registry: Dict[str, Dict[str, Callable[..., None]]] = {}
        new_states: Dict[Any, Dict[str, Any]] = {}
        wire = _walk(self._view(), (), registry, self._component_states, new_states)
        self._registry = registry
        self._component_states = new_states
        return wire

    def _handle(self, event: Any) -> None:
        if isinstance(event, dict) and "FocusChanged" in event:
            self._focused_id = event["FocusChanged"]
        if self._dispatch(event):
            return
        if self._on_event is not None:
            self._on_event(event)

    def _dispatch(self, event: Any) -> bool:
        """Tries every node-callback dispatch path in turn; `True` means a
        callback ran (so `_handle` must not also call `on_event`)."""
        if not isinstance(event, dict):
            return False

        if "KeyPressed" in event:
            name = _raw_key_name(event["KeyPressed"]["key"])
            if self._focused_id is not None and _is_activate_key(name):
                callback = self._registry.get(self._focused_id, {}).get("on_click")
                if callback is not None:
                    callback()
                    return True
            return False

        if "MouseDown" in event:
            payload = event["MouseDown"]
            widget = payload.get("widget")
            hit = event.get("hit")
            if widget and isinstance(hit, dict) and hit.get("kind") == "button_body":
                callback = self._registry.get(widget, {}).get("on_click")
                if callback is not None:
                    callback()
                    return True
            return False

        for container, variants in _CONTAINER_EVENT_MAP.items():
            if container not in event:
                continue
            widget_id, activation = event[container]
            if not isinstance(activation, dict):
                return False
            for variant_name, (callback_name, extract_args) in variants.items():
                if variant_name in activation:
                    callback = self._registry.get(widget_id, {}).get(callback_name)
                    if callback is not None:
                        callback(*extract_args(activation[variant_name]))
                        return True
                    return False
            return False

        return False

    # --- thin passthroughs to the wrapped QuiApp -----------------------------

    def step(self) -> bool:
        return self.qui.step()

    def run(self, max_steps: int = 0) -> bool:
        return self.qui.run(max_steps)

    def push_event(self, event: Any) -> None:
        self.qui.push_event(event)

    def push_key_event(self, ch: int) -> None:
        self.qui.push_key_event(ch)

    def screen(self) -> str:
        return self.qui.screen()

    def screen_contains(self, needle: str) -> bool:
        return self.qui.screen_contains(needle)

    def widget_state(self, widget_id: str) -> Optional[dict]:
        return self.qui.widget_state(widget_id)

    def run_tui(self, mouse: bool = True) -> None:
        self.qui.run_tui(mouse)

    def run_native(self, **kwargs: Any) -> None:
        self.qui.run_native(**kwargs)
