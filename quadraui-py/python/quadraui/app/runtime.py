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

from . import hot_reload as _hr
from .component import Component
from .nodes import Node

__all__ = ["App", "key_name"]


def _auto_id(path: Tuple[Any, ...]) -> str:
    """A stable id for a `Node` that didn't give its own: deterministic for
    a given structural position, as long as the shape of the tree above it
    (how many siblings, which slot names) doesn't change between renders --
    the same assumption a list index or dict key relies on anywhere else."""
    return "auto:" + ".".join(str(part) for part in path)


def _component_type_key(component: Component) -> str:
    """The type half of a component's state key: the class's qualified
    name rather than the class object, so the class a hot reload replaces
    still maps to the state slot its predecessor filled."""
    cls = type(component)
    return f"{cls.__module__}.{cls.__qualname__}"


def _walk(
    value: Any,
    path: Tuple[Any, ...],
    registry: Dict[str, Dict[str, Callable[..., None]]],
    old_states: Dict[Any, Dict[str, Any]],
    new_states: Dict[Any, Dict[str, Any]],
    migrate: bool = False,
) -> Any:
    if value is None:
        return None
    if isinstance(value, list):
        return [_walk(item, path + (i,), registry, old_states, new_states, migrate) for i, item in enumerate(value)]
    if isinstance(value, Component):
        state_key = (_component_type_key(value), value.key if value.key is not None else path)
        state = new_states.get(state_key)
        if state is None:
            state = old_states.get(state_key)
            if state is None:
                state = value.initial_state()
            elif migrate:
                state = value.migrate_state(state)
            new_states[state_key] = state
        child = value.view(state)
        return _walk(child, path + ("component",), registry, old_states, new_states, migrate)
    if isinstance(value, Node):
        node_id = value.id if value.id is not None else _auto_id(path)
        children_wire = {
            slot: _walk(child, path + (slot,), registry, old_states, new_states, migrate)
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
        self._watcher: Optional[_hr.ModuleWatcher] = None
        self._poller: Optional[_hr._PollThread] = None
        self._on_reload: Optional[Callable[["App"], None]] = None
        self._rollback: Optional[Callable[[], None]] = None
        self._migrate_next_render = False
        self.reload_error: Optional[BaseException] = None
        self.reload_count = 0
        self.qui = QuiApp(width, height, render=self._render)
        self.qui.set_handler(self._handle)
        self.actions = self.qui.actions()

    def set_on_event(self, on_event: Optional[Callable[[Any], None]]) -> None:
        """Replaces the global `on_event` fallback given at construction (or
        clears it, with `None`) -- the public way to compose/swap the
        escape-hatch handler after the fact, instead of assigning the
        private `_on_event` attribute directly."""
        self._on_event = on_event

    # --- hot reload -----------------------------------------------------------

    def enable_hot_reload(
        self,
        *modules: Any,
        interval: Optional[float] = 0.5,
        on_reload: Optional[Callable[["App"], None]] = None,
    ) -> None:
        """Watches `modules` (module objects or dotted names, reloaded in
        the order given -- list a dependency before the module importing
        from it) and re-imports them when their source files change,
        keeping component, app-object and widget state; see
        `quadraui.app.hot_reload` for exactly what survives.

        With `interval` (seconds) a daemon thread checks the files and
        wakes the event loop on a change, so `run_tui()`/`run_native()`
        pick an edit up without any input; with `interval=None` nothing
        is checked until `reload_if_changed()` is called. `on_reload(app)`
        runs after every successful reload, before the first render with
        the new code. Calling this again replaces the watch list."""
        self.disable_hot_reload()
        self._watcher = _hr.ModuleWatcher(_hr.resolve_modules(modules))
        self._on_reload = on_reload
        if interval is not None:
            waker = self.qui.waker()
            self._poller = _hr._PollThread(self._watcher, interval, lambda: waker.post(_hr.HOT_RELOAD_WAKE))

    def disable_hot_reload(self) -> None:
        """Stops watching (and stops the watcher thread, if any)."""
        if self._poller is not None:
            self._poller.stop()
        self._poller = None
        self._watcher = None

    def reload_if_changed(self) -> bool:
        """Reloads the watched modules if any source file changed since the
        last check; `True` when the re-import succeeded, with the same
        "not yet rendered" caveat `reload()` documents. Needs
        `enable_hot_reload()` first."""
        if self._watcher is None:
            raise RuntimeError("enable_hot_reload() has not been called")
        if not self._watcher.changed():
            return False
        return self.reload()

    def reload(self) -> bool:
        """Re-imports every watched module now, whether or not it changed,
        and swaps the view/`on_event` over to the new code. On failure the
        previous code stays in place, the exception is stored on
        `reload_error`, and this returns `False`.

        `True` means the re-import and the view/`on_event` rebind
        succeeded -- *not* that the new code has rendered, since rendering
        is driven separately (the next `step()`). A view that raises on
        that first render is rolled back there instead: `reload_error`
        becomes that exception and `reload_count` goes back down, after
        this call has already returned `True`. Read `reload_error` after
        the next render, not this return value, to know a reload stuck."""
        if self._watcher is None:
            raise RuntimeError("enable_hot_reload() has not been called")
        modules = self._watcher.modules
        self._watcher.snapshot()
        try:
            saved = _hr.reload_modules(modules)
        except Exception as exc:  # a broken edit must not take the app down
            self.reload_error = exc
            return False
        reloaded = {m.__name__: m for m in modules}
        rebound: Dict[int, Tuple[Any, type]] = {}
        old_view, old_on_event = self._view, self._on_event

        def rollback() -> None:
            _hr.restore_modules(saved)
            for instance, cls in rebound.values():
                instance.__class__ = cls
            self._view, self._on_event = old_view, old_on_event
            self._migrate_next_render = False

        try:
            self._view = _hr.rebind_callable(self._view, reloaded, rebound)
            self._on_event = _hr.rebind_callable(self._on_event, reloaded, rebound)
            if self._on_reload is not None:
                self._on_reload(self)
        except Exception as exc:
            rollback()
            self.reload_error = exc
            return False
        self._rollback = rollback
        self._migrate_next_render = True
        self.reload_error = None
        self.reload_count += 1
        return True

    # --- render / dispatch ---------------------------------------------------

    def _render(self) -> Any:
        if self._rollback is None:
            return self._render_once()
        rollback, self._rollback = self._rollback, None
        try:
            return self._render_once()
        except Exception as exc:
            rollback()
            self.reload_error = exc
            self.reload_count -= 1
            return self._render_once()

    def _render_once(self) -> Any:
        registry: Dict[str, Dict[str, Callable[..., None]]] = {}
        new_states: Dict[Any, Dict[str, Any]] = {}
        migrate, self._migrate_next_render = self._migrate_next_render, False
        try:
            wire = _walk(self._view(), (), registry, self._component_states, new_states, migrate)
        except BaseException:
            self._migrate_next_render = migrate
            raise
        self._registry = registry
        self._component_states = new_states
        return wire

    def _handle(self, event: Any) -> None:
        if isinstance(event, dict) and event.get("User") == _hr.HOT_RELOAD_WAKE and self._watcher is not None:
            self.reload_if_changed()
            return
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
