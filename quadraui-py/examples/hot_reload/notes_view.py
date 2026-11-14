"""The view half of `examples/hot_reload_demo.py`: edit this file while the
demo runs (change a label, the counter step, the layout) and the running
app picks the edit up, keeping the counter value, the note list, and the
list's selection. A syntax error leaves the last good version on screen
with the error in the status bar until the file is fixed.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Dict, List, Optional

from quadraui.app import App, Button, Column, Component, StatusBar, TextDisplay, Tree

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import Color, StatusBarSegment, StyledText, TextDisplayLineDescriptor, TreeRowDescriptor


def line(text: str) -> "StyledText":
    return {"spans": [{"text": text, "fg": None, "bg": None, "bold": False, "italic": False, "underline": False}]}


def _display_line(text: str) -> "TextDisplayLineDescriptor":
    """One `text_display` line with the same single uncolored span `line()`
    builds; its own helper rather than reusing `line()`'s return, since
    `TextDisplayLineDescriptor` adds `decoration`/`timestamp` on top of
    `StyledText`'s own `spans`."""
    return {"spans": [{"text": text, "fg": None, "bg": None, "bold": False, "italic": False, "underline": False}]}


_FG: "Color" = {"r": 220, "g": 220, "b": 230, "a": 255}
_BG: "Color" = {"r": 30, "g": 32, "b": 48, "a": 255}


def _segment(text: str) -> "StatusBarSegment":
    return {"text": text, "fg": _FG, "bg": _BG, "bold": False}


def _row(i: int, text: str) -> "TreeRowDescriptor":
    return {"path": [i], "indent": 0, "icon": None, "text": line(text), "badge": None, "is_expanded": False}


class Counter(Component):
    """Component-local state: survives every reload of this file."""

    def initial_state(self) -> Dict[str, Any]:
        return {"count": 0}

    def view(self, state: Dict[str, Any]) -> Any:
        def bump() -> None:
            state["count"] += 1

        return Column(
            TextDisplay([_display_line(f"Counter: {state['count']}")], id="hr:count"),
            Button("Add one", id="hr:add", on_click=bump),
        )


class NotesApp:
    """App-object state (`self.notes`) survives a reload: the instance is
    kept and only its class is swapped for the edited one."""

    def __init__(self) -> None:
        self.notes: List[str] = ["edit examples/hot_reload/notes_view.py", "save it", "watch this window"]
        self.app: Optional[App] = None

    def add_note(self) -> None:
        self.notes.append(f"note {len(self.notes) + 1}")

    def status(self) -> str:
        if self.app is None:
            return " ready "
        if self.app.reload_error is not None:
            return f" reload failed: {self.app.reload_error!r} "
        return f" reloads: {self.app.reload_count} "

    def view(self) -> Any:
        return Column(
            TextDisplay([_display_line("Hot reload demo")], id="hr:title"),
            Counter(),
            Button("Add note", id="hr:note", on_click=self.add_note),
            Tree([_row(i, n) for i, n in enumerate(self.notes)], id="hr:notes"),
            StatusBar([_segment(self.status())], [], id="hr:status"),
        )
