"""`Widget`: base class for a script-authored node with its own
`measure`/`paint`/`on_event`/`a11y` -- for a widget that can't be built by
composing existing nodes (reach for `Component` first; see `AGENTS.md`'s
"shape of an app" section). Builds directly on the `Canvas`
`DrawOp`/`CanvasRegion` display-list vocabulary every other `quadraui`
primitive paints through (`quadraui.wire.CanvasDescriptor`) -- `paint`
returns the same `{"ops": [...], "regions": [...]}` shape, so a script
widget degrades to TUI exactly the way a hand-written `Canvas` descriptor
already does.

**Why registration is a module-level side channel, not a `QuiApp` call.**
A `view()` builds this node's whole subtree -- including every `Widget`
instance in it -- synchronously inside the render callback `QuiApp.render`
invokes. That callback's own contract ("must not call back into this
`QuiApp`" -- see `QuiApp.set_render`'s doc) rules out a `Widget.build()`
that reaches back into the live app to register itself: `AppLogic::render`
already holds this app's one mutable borrow for the callback's whole
duration, so a reentrant call into it would panic. `_PENDING` is a plain
Python dict instead: `build()` stashes this widget (plus its props/state/
focusable flag) under its own wire id, and `_take_pending()` drains it --
called by the native extension once per render, right after the callback
above has already returned the wire tree, when calling back into `QuiApp`
is safe again.

Rendering only (Tree-mode `view()` output, i.e. anywhere a `Node` is
accepted) resolves a `Widget` into a live script-backed primitive -- the
flat six-slot `QuiApp.set_render` shape (`"button"`/`"text_display"`/...)
has no per-id additive slot to put one in, so a `Widget` placed there
paints as its placeholder (an empty, non-interactive canvas) instead.
"""

from __future__ import annotations

from typing import Any, Dict, Optional

from .nodes import Node

__all__ = ["Widget"]

_PENDING: Dict[str, Dict[str, Any]] = {}


def _take_pending() -> Dict[str, Dict[str, Any]]:
    """Drains and returns every `Widget` registered since the last call.
    Called by the native extension once per render; never call this from
    app code."""
    global _PENDING
    pending, _PENDING = _PENDING, {}
    return pending


class Widget(Node):
    """Subclass and override `measure`/`paint`; override `on_event`/
    `a11y` for interaction/accessibility (both default to "nothing to
    report"). Give every instance an explicit `id=` -- it is reused every
    frame as this widget's wire id and the namespace its declared
    `CanvasRegion` ids are resolved against; an auto-id (position-derived,
    see `quadraui.app.runtime`'s module doc) works but is fragile the same
    way it is for any other node.

    A fresh instance is normally built every frame, the same as any other
    node constructor -- keep this class itself stateless and put whatever
    the widget needs to remember in `props`/`state` (read back from the
    app's own model each render) rather than on `self`, the same
    `Component`/MVU discipline every other node in this layer follows.

    `props` is read back by `paint`/`on_event`/`a11y` every frame exactly
    as passed in here -- pass whatever the view needs to draw itself.
    `state` is separate, script-tracked state (e.g. a drag in progress)
    threaded through `on_event` only, matching the Rust-side
    `ScriptWidget` contract this wraps.
    """

    def __init__(
        self,
        *,
        id: Optional[str] = None,
        props: Optional[Dict[str, Any]] = None,
        state: Optional[Dict[str, Any]] = None,
        focusable: bool = False,
    ) -> None:
        self.props: Dict[str, Any] = {} if props is None else props
        self.state: Dict[str, Any] = {} if state is None else state
        self.focusable = focusable
        super().__init__("widget", id, {}, {}, self._build)

    def measure(self, ctx: Dict[str, Any], constraints: Dict[str, Any]) -> Dict[str, float]:
        """Sizes this widget for the layout solver. `ctx` carries
        `line_height`/`char_width`/`nerd_fonts_enabled` (the backend's
        native units); `constraints` carries `max_width`/`max_height`,
        each `None` when that axis is unconstrained. Returns
        `{"width": ..., "height": ...}`, in the same units as `ctx`.
        Must not raise `NotImplementedError` in a widget actually placed
        in a tree -- the default here is only a reminder to override it."""
        raise NotImplementedError("Widget subclasses must override measure()")

    def paint(
        self, ctx: Dict[str, Any], props: Dict[str, Any], rect: Dict[str, float]
    ) -> Dict[str, Any]:
        """Builds this frame's display list. `rect` is this widget's own
        resolved area (`x`/`y`/`width`/`height`), in `ctx`'s units.
        Returns `{"ops": [...], "regions": [...]}` -- the same shape
        `quadraui.wire.CanvasDescriptor` documents. Every op/region
        coordinate is LOCAL to `rect` ((0, 0) is this widget's own
        top-left corner), not the screen -- the same convention a `Canvas`
        node's own `ops`/`regions` already use."""
        raise NotImplementedError("Widget subclasses must override paint()")

    def on_event(self, event: Dict[str, Any], props: Dict[str, Any], state: Dict[str, Any]) -> bool:
        """Handles a region click/double-click or a focused key press.
        `event` is `{"Canvas": <CanvasEvent>}` for a region click (the
        same payload shape a plain `Canvas` node's own region event
        carries, minus the leading widget id -- this widget already knows
        which one it is) or `{"Key": <Key>}` for a key pressed while this
        widget holds focus. Returns whether this widget consumed it; an
        app-level `on_*` callback stored on `self` (not `props`/`state`)
        is the usual way to also notify the surrounding app -- see
        `quadraui-py/examples/rating.py`. The default consumes nothing."""
        return False

    def a11y(self, props: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Optional accessibility node:
        `{"info": {...A11yInfo fields...}, "children": [...]}` (every
        `A11yInfo` field is optional and defaults to "not asserted"; see
        `quadraui.wire`'s module for the full field list). `None` (the
        default) reports nothing -- correct for a widget with no
        accessible surface of its own."""
        return None

    def _build(self, node_id: str, _children: Dict[str, Any]) -> Dict[str, Any]:
        _PENDING[node_id] = {
            "widget": self,
            "props": self.props,
            "state": self.state,
            "focusable": self.focusable,
        }
        return {
            "id": node_id,
            "canvas": {"id": node_id, "ops": [], "regions": [], "focusable": self.focusable},
        }
