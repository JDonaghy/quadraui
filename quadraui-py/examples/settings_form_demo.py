"""Runnable demo of `Form` and its typed field builders: a settings page
with a boolean, a text field, a slider, a dropdown, a password field and
a notes text area, wired up with per-field `on_change`/`on_submit`, with
no raw wire dict and no `"Form"` branch in `on_event` -- the typed-field
shape issue #2463 adds, in place of the raw `{"form": {"fields": [...]}}`
leaf a lower-level app would have to hand-write.

Run headless (drives one change per field kind through `App.push_event`,
then prints what each callback recorded):

    maturin develop
    python quadraui-py/examples/settings_form_demo.py

Run in a real terminal (Tab/Shift-Tab move between fields, Space toggles
the checkbox, arrow keys adjust the slider/dropdown, typing edits the
text fields, `q`/Escape quits):

    python quadraui-py/examples/settings_form_demo.py --tui
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Any

from quadraui.app import (
    App,
    Form,
    PasswordField,
    SelectField,
    SliderField,
    TextAreaField,
    TextInputField,
    ToggleField,
)

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import UiEvent

WIDTH = 50
HEIGHT = 10

THEMES = ["Light", "Dark", "High Contrast"]


class _Quit(Exception):
    """Raised from `_on_event` to unwind out of `run_tui()`."""


class SettingsFormDemo:
    """Owns the one piece of state this demo needs: the current value of
    every field, kept as plain attributes and echoed back into the next
    `view()` -- the controlled shape every field builder's own docstring
    documents."""

    def __init__(self) -> None:
        self.word_wrap = True
        self.display_name = ""
        self.volume = 5.0
        self.theme_idx = 0
        self.password = ""
        self.password_submitted = ""
        self.notes = ""

    def view(self) -> Any:
        return Form(
            [
                ToggleField(
                    self.word_wrap,
                    id="settings:word_wrap",
                    label="Word wrap",
                    on_change=self._on_word_wrap,
                ),
                TextInputField(
                    self.display_name,
                    id="settings:name",
                    label="Display name",
                    placeholder="anonymous",
                    on_change=self._on_display_name,
                ),
                SliderField(
                    self.volume,
                    0.0,
                    10.0,
                    id="settings:volume",
                    step=0.5,
                    label="Volume",
                    on_change=self._on_volume,
                ),
                SelectField(
                    THEMES,
                    self.theme_idx,
                    id="settings:theme",
                    label="Theme",
                    on_change=self._on_theme,
                ),
                PasswordField(
                    self.password,
                    id="settings:password",
                    label="Password",
                    on_change=self._on_password,
                    on_submit=self._on_password_submit,
                ),
                TextAreaField(
                    self.notes,
                    id="settings:notes",
                    label="Notes",
                    visible_rows=2,
                    on_change=self._on_notes,
                ),
            ],
            id="settings:form",
            has_focus=True,
        )

    def _on_word_wrap(self, value: bool) -> None:
        self.word_wrap = value

    def _on_display_name(self, value: str) -> None:
        self.display_name = value

    def _on_volume(self, value: float) -> None:
        self.volume = value

    def _on_theme(self, idx: int) -> None:
        self.theme_idx = idx

    def _on_password(self, value: str) -> None:
        self.password = value

    def _on_password_submit(self, value: str) -> None:
        self.password_submitted = value

    def _on_notes(self, value: str) -> None:
        self.notes = value

    def on_event(self, event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                raise _Quit


def build() -> "tuple[App, SettingsFormDemo]":
    demo = SettingsFormDemo()
    app = App(WIDTH, HEIGHT, view=demo.view, on_event=demo.on_event)
    return app, demo


def _run_headless(app: App, demo: SettingsFormDemo) -> None:
    print("before any interaction:")
    print(app.screen())

    app.push_event({"Form": ["settings:form", {"ToggleChanged": {"id": "settings:word_wrap", "value": False}}]})
    app.run(1)
    print(f"\nafter flipping word wrap, word_wrap == {demo.word_wrap}")

    app.push_event(
        {"Form": ["settings:form", {"TextInputChanged": {"id": "settings:name", "value": "Ada"}}]}
    )
    app.run(1)
    print(f"after typing a name, display_name == {demo.display_name!r}")

    app.push_event({"Form": ["settings:form", {"SliderChanged": {"id": "settings:volume", "value": 8.0}}]})
    app.run(1)
    print(f"after moving the slider, volume == {demo.volume}")

    app.push_event({"Form": ["settings:form", {"DropdownChanged": {"id": "settings:theme", "selected_idx": 1}}]})
    app.run(1)
    print(f"after picking a theme, theme == {THEMES[demo.theme_idx]!r}")

    app.push_event(
        {"Form": ["settings:form", {"TextInputChanged": {"id": "settings:password", "value": "s3cr3t"}}]}
    )
    app.run(1)
    app.push_event(
        {"Form": ["settings:form", {"TextInputCommitted": {"id": "settings:password", "value": "s3cr3t"}}]}
    )
    app.run(1)
    print(f"after typing and committing a password, password_submitted == {demo.password_submitted!r}")


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"settings_form_demo: notes == {demo.notes!r}")
    else:
        _run_headless(app, demo)
