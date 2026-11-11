"""The view half of `examples/hot_reload_demo.py`: edit this file while the
demo runs (change a label, the counter step, the layout) and the running
app picks the edit up, keeping the counter value, the note list, and the
list's selection. A syntax error leaves the last good version on screen
with the error in the status bar until the file is fixed.
"""

from quadraui.app import Button, Column, Component, StatusBar, TextDisplay, Tree


def line(text):
    return {"spans": [{"text": text, "fg": None, "bg": None, "bold": False, "italic": False, "underline": False}]}


_FG = {"r": 220, "g": 220, "b": 230, "a": 255}
_BG = {"r": 30, "g": 32, "b": 48, "a": 255}


def _segment(text):
    return {"text": text, "fg": _FG, "bg": _BG, "bold": False}


def _row(i, text):
    return {"path": [i], "indent": 0, "icon": None, "text": line(text), "badge": None, "is_expanded": False}


class Counter(Component):
    """Component-local state: survives every reload of this file."""

    def initial_state(self):
        return {"count": 0}

    def view(self, state):
        def bump():
            state["count"] += 1

        return Column(
            TextDisplay([line(f"Counter: {state['count']}")], id="hr:count"),
            Button("Add one", id="hr:add", on_click=bump),
        )


class NotesApp:
    """App-object state (`self.notes`) survives a reload: the instance is
    kept and only its class is swapped for the edited one."""

    def __init__(self):
        self.notes = ["edit examples/hot_reload/notes_view.py", "save it", "watch this window"]
        self.app = None

    def add_note(self):
        self.notes.append(f"note {len(self.notes) + 1}")

    def status(self):
        if self.app is None:
            return " ready "
        if self.app.reload_error is not None:
            return f" reload failed: {self.app.reload_error!r} "
        return f" reloads: {self.app.reload_count} "

    def view(self):
        return Column(
            TextDisplay([line("Hot reload demo")], id="hr:title"),
            Counter(),
            Button("Add note", id="hr:note", on_click=self.add_note),
            Tree([_row(i, n) for i, n in enumerate(self.notes)], id="hr:notes"),
            StatusBar([_segment(self.status())], [], id="hr:status"),
        )
