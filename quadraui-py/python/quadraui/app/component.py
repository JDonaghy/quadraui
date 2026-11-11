"""`Component`: a reusable unit of view + local state, usable as a node
wherever a `Node`/dict is accepted.

A fresh `Component()` instance is typically constructed every frame (inline,
inside `render()`, the same way a `Node` constructor is called every
frame) -- it is cheap and holds no state of its own on `self`. Its *state*
instead lives in a dict `quadraui.app.runtime.App` owns and keys by this
component's position in the tree (or its explicit `key=`, when a list of
components needs one that survives reordering): `view(state)` reads and
mutates that dict directly, and the next render passes the same dict back
in, so a `state["count"] += 1` inside an event callback survives to the
component's next `view()` call exactly like a module-level app's own
instance attributes do.
"""

from __future__ import annotations

from typing import Any, Dict, Optional

__all__ = ["Component"]


class Component:
    """Subclass and override `initial_state()` (the state dict's shape the
    first time this component's `key`/position is ever seen) and
    `view(state)` (returns whatever this component renders as -- a `Node`,
    another `Component`, a raw wire dict, or `None`).

    `key`, if given, identifies this component's state slot directly,
    instead of the (type, tree-position) pair `quadraui.app.runtime._walk`
    otherwise derives (the type part is the class's qualified name, so a
    hot-reloaded class keeps its slots) -- needed only when a list of components can reorder
    (state should follow the item, not the slot it happened to render into
    this frame)."""

    key: Optional[Any] = None

    def __init__(self, *, key: Optional[Any] = None) -> None:
        self.key = key

    def initial_state(self) -> Dict[str, Any]:
        return {}

    def view(self, state: Dict[str, Any]) -> Any:
        raise NotImplementedError

    def migrate_state(self, state: Dict[str, Any]) -> Dict[str, Any]:
        """Called once per surviving state slot on the first render after
        `App` hot-reloads this component's code (see
        `quadraui.app.hot_reload`); returns the state dict the reloaded
        `view(state)` sees from then on. The default keeps every existing
        value and adds any key the current `initial_state()` has that
        `state` lacks; override it to rename or reshape keys an edit
        changed."""
        for name, value in self.initial_state().items():
            state.setdefault(name, value)
        return state
