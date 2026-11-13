"""Runnable demo of the per-node `"style"` wire key: a `"row"` of three
buttons ("Save" / "Discard" / "Cancel"), where only "Discard" carries its
own `"style": {"surface_bg": ...}` override -- a scoped re-colour of
exactly that button's own fill, not the row's, not its siblings'.

Toggling the override (`d` in the interactive terminal, or a direct
`_render()` call headless) proves two things at once:

- `"surface_bg"` is the semantic alias that actually reaches a
  `Secondary`-variant `Button`'s fill -- a plain `"bg"` override, which
  only ever reaches `Theme.background`, is a silent no-op on it (see
  `docs/bindings-wire-format.md`'s own per-leaf token table).
- The override is purely per-node: "Save"/"Cancel" paint with the
  default `surface_bg` on every frame, whether or not "Discard"'s own
  override is active.

Run headless (prints the screen before and after toggling the override):

    maturin develop
    python quadraui-py/examples/style_override_demo.py

Run in a real terminal (`d` toggles the override, `q`/Escape quits):

    python quadraui-py/examples/style_override_demo.py --interactive
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Dict

from quadraui import QuiApp

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import Key, UiEvent

WIDTH = 32
HEIGHT = 1

_DANGER_BG = "#aa2222"
_TOGGLE_KEY: "Key" = {"Char": "d"}


class StyleOverrideDemo:
    """Owns the one piece of state this demo needs -- whether "Discard"'s
    own `"style"` override is active -- and a `render` callback that reads
    it fresh every frame; `step()`/`run_tui()` share the same callback."""

    def __init__(self) -> None:
        self.state: Dict[str, bool] = {"discard_styled": False}
        self.app = QuiApp(WIDTH, HEIGHT, render=self._render)
        self.app.set_handler(self._handle)

    def _render(self) -> Dict[str, Any]:
        # `"size": "Content"` sizes each button to its own natural label
        # width instead of an equal three-way share of `WIDTH` -- so
        # "Discard"'s own `"[ Discard ]"` chrome never clips.
        discard_leaf: Dict[str, Any] = {
            "id": "discard",
            "button": {"id": "discard", "label": "Discard", "variant": "Secondary"},
            "size": "Content",
        }
        if self.state["discard_styled"]:
            discard_leaf["style"] = {"surface_bg": _DANGER_BG}
        return {
            "row": [
                {
                    "id": "save",
                    "button": {"id": "save", "label": "Save", "variant": "Secondary"},
                    "size": "Content",
                },
                discard_leaf,
                {
                    "id": "cancel",
                    "button": {"id": "cancel", "label": "Cancel", "variant": "Secondary"},
                    "size": "Content",
                },
            ],
            "gap": 1,
        }

    def _handle(self, event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            if event["KeyPressed"]["key"] == _TOGGLE_KEY:
                self.state["discard_styled"] = not self.state["discard_styled"]

    def toggle(self) -> None:
        self.state["discard_styled"] = not self.state["discard_styled"]
        self.app.step()


def run_demo() -> StyleOverrideDemo:
    demo = StyleOverrideDemo()
    demo.app.step()
    return demo


class _Quit(Exception):
    """Sentinel raised from the interactive handler to unwind out of
    `run_tui()`."""


def _is_quit_key(key: "Key") -> bool:
    return key == {"Char": "q"} or key == {"Named": "Escape"}


def _run_interactive() -> None:
    demo = StyleOverrideDemo()
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
    print(f"style_override_demo: closed with discard_styled={demo.state['discard_styled']}")


if __name__ == "__main__":
    import sys

    if "--interactive" in sys.argv:
        _run_interactive()
    else:
        demo = run_demo()
        print("before toggling \"Discard\"'s own style override:")
        print(demo.app.screen())
        demo.toggle()
        print('\nafter toggling "Discard"\'s own style override on:')
        print(demo.app.screen())
