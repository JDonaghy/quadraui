"""Runnable demo of named style sets: `app.define_style(name, overrides)`
plus the `"class"` key any layout-tree node may carry -- the CSS-class
analogue of the per-node `"style"` key `style_override_demo.py` shows.

A `"row"` of four buttons, each pulling its look from a *name* rather than
from inline tokens: "Save" uses the built-in `"primary"` set, "Discard" a
host-defined `"brand"` set, "Reset" the two of them together (`"brand
danger"` -- later name wins the tokens both name), and "Quiet" the built-in
`"subtle"` set.

Redefining `"brand"` (`b` in the interactive terminal, or a direct
`redefine_brand()` call headless) repaints every node that names it, with
no change to the render callback at all -- which is the whole point of a
named set over inline tokens.

Run headless (prints the screen before and after redefining `"brand"`):

    maturin develop
    python quadraui-py/examples/style_classes_demo.py

Run in a real terminal (`b` redefines `"brand"`, `q`/Escape quits):

    python quadraui-py/examples/style_classes_demo.py --interactive
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Dict

from quadraui import QuiApp

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import Key, UiEvent

WIDTH = 48
HEIGHT = 1

_BRAND_TEAL = "#1f6f6f"
_BRAND_PLUM = "#6f1f6f"
_REDEFINE_KEY: "Key" = {"Char": "b"}


def _button(leaf_id: str, label: str, classes: str) -> Dict[str, Any]:
    # `"size": "Content"` sizes each button to its own natural label width
    # instead of an equal four-way share of `WIDTH`, so no `"[ ... ]"`
    # chrome clips.
    return {
        "id": leaf_id,
        "button": {"id": leaf_id, "label": label, "variant": "Secondary"},
        "size": "Content",
        "class": classes,
    }


class StyleClassesDemo:
    """Owns the one piece of state this demo needs -- which colour the
    host-defined `"brand"` set currently holds -- and a `render` callback
    that never mentions a colour at all, only class names."""

    def __init__(self) -> None:
        self.brand_bg = _BRAND_TEAL
        self.app = QuiApp(WIDTH, HEIGHT, render=self._render)
        self.app.set_handler(self._handle)
        # Obtained once up front, same restriction `QuiApp.actions`'s own
        # doc gives -- there is no reachable way to call it again once
        # `run_tui` owns this thread, only before. `_handle` below calls
        # `define_style` through this handle rather than through `self.app`
        # directly: `self.app` already holds its one mutable borrow for
        # the whole dispatch that invokes `_handle`, so a direct
        # `self.app.define_style(...)` call from there raises "already
        # borrowed".
        self.actions = self.app.actions()
        # `"surface_bg"` is the alias a Secondary button's fill actually
        # reads -- a plain `"bg"` would be a silent no-op on it, see
        # `docs/bindings-wire-format.md`'s own per-leaf token table.
        self.app.define_style("brand", {"surface_bg": self.brand_bg})

    def _render(self) -> Dict[str, Any]:
        return {
            "row": [
                _button("save", "Save", "primary"),
                _button("discard", "Discard", "brand"),
                # Both sets name `surface_bg`; the later one in the list,
                # `"danger"`, wins it, so this button renders with
                # `"danger"`'s fill rather than `"brand"`'s.
                _button("reset", "Reset", "brand danger"),
                _button("quiet", "Quiet", "subtle"),
            ],
            "gap": 1,
        }

    def _handle(self, event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            if event["KeyPressed"]["key"] == _REDEFINE_KEY:
                self._redefine_brand_from_handler()

    def redefine_brand(self) -> None:
        """Swaps what `"brand"` means, from top-level script code. Every
        node naming it repaints on the next frame; the render callback is
        untouched."""
        self.brand_bg = _BRAND_PLUM if self.brand_bg == _BRAND_TEAL else _BRAND_TEAL
        self.app.define_style("brand", {"surface_bg": self.brand_bg})
        self.app.step()

    def _redefine_brand_from_handler(self) -> None:
        """Same swap as `redefine_brand`, but callable from inside `_handle`
        itself: queues through `self.actions` (obtained once in
        `__init__`) rather than calling `self.app.define_style` directly,
        which would raise "already borrowed" from here -- see
        `__init__`'s own comment. The queued action is applied before this
        dispatch returns, so a running `run_tui` loop repaints without
        needing an explicit `step()` call."""
        self.brand_bg = _BRAND_PLUM if self.brand_bg == _BRAND_TEAL else _BRAND_TEAL
        self.actions.define_style("brand", {"surface_bg": self.brand_bg})


def run_demo() -> StyleClassesDemo:
    demo = StyleClassesDemo()
    demo.app.step()
    return demo


class _Quit(Exception):
    """Sentinel raised from the interactive handler to unwind out of
    `run_tui()`."""


def _is_quit_key(key: "Key") -> bool:
    return key == {"Char": "q"} or key == {"Named": "Escape"}


def _run_interactive() -> None:
    demo = StyleClassesDemo()
    original_handle = demo._handle

    def handler(event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            if _is_quit_key(event["KeyPressed"]["key"]):
                raise _Quit
        original_handle(event)

    demo.app.set_handler(handler)
    try:
        demo.app.run_tui()
    except _Quit:
        pass
    print(f"style_classes_demo: closed with brand={demo.brand_bg}")


if __name__ == "__main__":
    import sys

    if "--interactive" in sys.argv:
        _run_interactive()
    else:
        demo = run_demo()
        print(f"registered style sets: {demo.app.style_set_names()}")
        print(f"\nwith \"brand\" = {_BRAND_TEAL}:")
        print(demo.app.screen())
        demo.redefine_brand()
        print(f"\nafter redefining \"brand\" to {demo.brand_bg}:")
        print(demo.app.screen())
