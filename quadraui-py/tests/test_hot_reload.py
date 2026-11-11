"""Black-box coverage for `quadraui.app.App`'s hot reload: an app whose view
code lives in a real module on disk keeps running while that file is
rewritten, picks the new code up, and keeps component state, the state on
its bound-method view's own object, and framework widget state; a broken
edit leaves the last good code running.

The app is driven through `quadraui.testing` -- the app-author test
harness -- so a click is a click and a keypress is a keypress, with no
wire event spelled out here. Only the reload calls themselves go straight
to `App`: `enable_hot_reload`/`reload_if_changed` are the app's own
lifecycle API, not something a user does to a widget.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_hot_reload.py
"""

import importlib
import os
import sys
import textwrap
import time
import uuid

import pytest

from quadraui.app import App
from quadraui.testing import harness


class _Module:
    """A throwaway module written to `tmp_path` and imported for real."""

    def __init__(self, tmp_path, source):
        self.name = "hot_reload_fixture_" + uuid.uuid4().hex[:8]
        self.path = tmp_path / f"{self.name}.py"
        self._tick = 0
        self.write(source)
        sys.path.insert(0, str(tmp_path))
        self.module = importlib.import_module(self.name)

    def write(self, source):
        self.path.write_text(textwrap.dedent(source))
        # A distinct mtime per write, so change detection never depends on
        # the filesystem's timestamp granularity.
        self._tick += 10
        stamp = time.time() + self._tick
        os.utime(self.path, (stamp, stamp))


COUNTER_V1 = """
    from quadraui.app import Button, Column, Component, TextDisplay

    def _line(text):
        return {"spans": [{"text": text, "fg": None, "bg": None, "bold": False, "italic": False, "underline": False}]}

    class Counter(Component):
        def initial_state(self):
            return {"count": 0}

        def view(self, state):
            def bump():
                state["count"] += 1
            return Column(
                TextDisplay([_line(f"v1 count={state['count']}")], id="t:text"),
                Button("Plus", id="b:plus", on_click=bump),
            )

    class Shell:
        def __init__(self):
            self.title = "first"

        def view(self):
            return Column(TextDisplay([_line(f"title={self.title}")], id="t:title"), Counter())
"""

COUNTER_V2 = COUNTER_V1.replace("v1 count=", "v2 count=").replace(
    'return {"count": 0}', 'return {"count": 0, "step": 5}'
).replace(
    "f\"v2 count={state['count']}\"", "f\"v2 count={state['count']} step={state['step']}\""
).replace(
    'f"title={self.title}"', 'f"new title={self.title}"'
)


def _click(ui, widget):
    """Activates a button the way a keyboard user does: focus it, press
    Enter."""
    ui.focus(widget)
    ui.press("Enter")


def _app(fixture, **kwargs):
    shell = fixture.module.Shell()
    app = App(60, 10, view=shell.view)
    app.enable_hot_reload(fixture.module, interval=None, **kwargs)
    return harness(app), shell


def test_reload_picks_up_edited_view_and_keeps_component_and_app_object_state(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    ui, shell = _app(fixture)
    assert ui.contains("v1 count=0")

    _click(ui, "b:plus")
    _click(ui, "b:plus")
    shell.title = "edited-at-runtime"
    ui.step()
    assert ui.contains("v1 count=2")

    fixture.write(COUNTER_V2)
    assert ui.app.reload_if_changed() is True
    ui.step()

    # New code renders, the component's count survived, the key the new
    # initial_state() introduced was merged in, and the bound-method view's
    # own object kept its attribute while running the new method body.
    assert ui.contains("v2 count=2 step=5"), ui.screen()
    assert ui.contains("new title=edited-at-runtime"), ui.screen()
    assert type(shell) is fixture.module.Shell
    assert ui.app.reload_error is None
    assert ui.app.reload_count == 1

    # Callbacks registered by the new code still drive the same state.
    _click(ui, "b:plus")
    assert ui.contains("v2 count=3 step=5"), ui.screen()


def test_reload_if_changed_is_a_no_op_without_an_edit(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    ui, _ = _app(fixture)
    assert ui.app.reload_if_changed() is False
    assert ui.app.reload_count == 0


def test_syntax_error_keeps_the_last_good_code_running(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    ui, _ = _app(fixture)
    _click(ui, "b:plus")

    fixture.write(COUNTER_V1 + "\n    def broken(:\n")
    assert ui.app.reload_if_changed() is False
    assert isinstance(ui.app.reload_error, SyntaxError)
    ui.step()
    assert ui.contains("v1 count=1"), ui.screen()
    _click(ui, "b:plus")
    assert ui.contains("v1 count=2"), ui.screen()

    # Fixing the file recovers and clears the error, state intact.
    fixture.write(COUNTER_V2)
    assert ui.app.reload_if_changed() is True
    ui.step()
    assert ui.app.reload_error is None
    assert ui.contains("v2 count=2 step=5"), ui.screen()


def test_view_that_raises_after_reload_rolls_back_to_the_previous_code(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    ui, _ = _app(fixture)
    _click(ui, "b:plus")

    fixture.write(COUNTER_V1.replace("def view(self):\n", "def view(self):\n            raise RuntimeError('boom')\n"))
    assert ui.app.reload_if_changed() is True
    ui.step()

    assert ui.contains("v1 count=1"), ui.screen()
    assert isinstance(ui.app.reload_error, RuntimeError)
    assert ui.contains("title=first"), ui.screen()
    _click(ui, "b:plus")
    assert ui.contains("v1 count=2"), ui.screen()


def test_framework_widget_state_survives_a_reload(tmp_path):
    source = """
        from quadraui.app import Tree

        def _row(i):
            return {"path": [i], "indent": 0, "icon": None,
                    "text": {"spans": [{"text": f"%s row {i}", "fg": None, "bg": None,
                                        "bold": False, "italic": False, "underline": False}]},
                    "badge": None, "is_expanded": False}

        def view():
            return Tree([_row(i) for i in range(3)], id="t:tree")
    """
    fixture = _Module(tmp_path, source % "old")
    app = App(40, 6, view=fixture.module.view)
    app.enable_hot_reload(fixture.module.__name__, interval=None)
    ui = harness(app)
    assert ui.contains("old row 0")

    ui.find(id="t:tree").row("old row 2").click()
    before = ui.state("t:tree")

    fixture.write(source % "new")
    assert ui.app.reload_if_changed() is True
    ui.step()

    assert ui.contains("new row 2"), ui.screen()
    assert ui.state("t:tree") == before


def test_on_reload_hook_runs_after_a_successful_reload(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    seen = []
    ui, _ = _app(fixture, on_reload=lambda a: seen.append(a.reload_count))
    fixture.write(COUNTER_V2)
    ui.app.reload_if_changed()
    assert seen == [0]


def test_watcher_thread_wakes_the_event_loop_on_an_edit(tmp_path):
    fixture = _Module(tmp_path, COUNTER_V1)
    shell = fixture.module.Shell()
    unclaimed = []
    app = App(60, 10, view=shell.view, on_event=unclaimed.append)
    app.enable_hot_reload(fixture.module, interval=0.02)
    ui = harness(app)
    try:
        fixture.write(COUNTER_V2)
        deadline = time.monotonic() + 5
        while ui.app.reload_count == 0 and time.monotonic() < deadline:
            ui.app.run(1)
        assert ui.app.reload_count == 1
        assert ui.contains("v2 count=0 step=5"), ui.screen()
        assert unclaimed == [], "the reload wake must not reach on_event"
    finally:
        ui.app.disable_hot_reload()


def test_a_script_run_as_main_is_rejected_with_a_clear_error():
    with pytest.raises(ValueError, match="__main__"):
        App(10, 3, view=lambda: None).enable_hot_reload(sys.modules["__main__"], interval=None)


# --- examples/hot_reload_demo.py -----------------------------------------


def _load_demo(tmp_path):
    """Copies the demo and its view package into `tmp_path` (so the test
    can rewrite the view file) and imports the copy."""
    import importlib.util
    import pathlib
    import shutil

    examples = pathlib.Path(__file__).resolve().parent.parent / "examples"
    shutil.copy(examples / "hot_reload_demo.py", tmp_path / "hot_reload_demo.py")
    shutil.copytree(examples / "hot_reload", tmp_path / "hot_reload")
    for name in ("hot_reload", "hot_reload.notes_view"):
        sys.modules.pop(name, None)
    spec = importlib.util.spec_from_file_location("hot_reload_demo_under_test", tmp_path / "hot_reload_demo.py")
    demo = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(demo)
    return demo, tmp_path / "hot_reload" / "notes_view.py"


def _bump_mtime(path):
    stamp = time.time() + 30
    os.utime(path, (stamp, stamp))


def test_demo_edit_repaints_and_keeps_counter_notes_and_selection(tmp_path):
    demo, view_file = _load_demo(tmp_path)
    app, _notes = demo.build(interval=None)
    ui = harness(app)
    try:
        assert ui.contains("Hot reload demo")
        _click(ui, "hr:add")
        _click(ui, "hr:add")
        _click(ui, "hr:note")
        ui.find(id="hr:notes").row(index=1).click()
        assert ui.contains("Counter: 2"), ui.screen()
        assert ui.contains("note 4"), ui.screen()
        selection = ui.state("hr:notes")

        view_file.write_text(view_file.read_text().replace('"Hot reload demo"', '"Edited title"'))
        _bump_mtime(view_file)
        assert ui.app.reload_if_changed() is True
        ui.step()

        assert ui.contains("Edited title"), ui.screen()
        assert ui.contains("Counter: 2"), ui.screen()
        assert ui.contains("note 4"), ui.screen()
        assert ui.contains("reloads: 1"), ui.screen()
        assert ui.state("hr:notes") == selection
    finally:
        ui.app.disable_hot_reload()


def test_demo_shows_a_broken_edit_in_the_status_bar(tmp_path):
    demo, view_file = _load_demo(tmp_path)
    app, _ = demo.build(interval=None)
    ui = harness(app)
    try:
        _click(ui, "hr:add")
        view_file.write_text(view_file.read_text() + "\ndef broken(:\n")
        _bump_mtime(view_file)
        assert ui.app.reload_if_changed() is False
        ui.step()
        assert ui.contains("reload failed"), ui.screen()
        assert ui.contains("Counter: 1"), ui.screen()
    finally:
        ui.app.disable_hot_reload()


def test_demo_quits_on_q():
    import importlib.util
    import pathlib

    path = pathlib.Path(__file__).resolve().parent.parent / "examples" / "hot_reload_demo.py"
    spec = importlib.util.spec_from_file_location("hot_reload_demo_quit", path)
    demo = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(demo)
    app, _ = demo.build(interval=None)
    ui = harness(app)
    try:
        with pytest.raises(demo._Quit):
            ui.press("q")
    finally:
        ui.app.disable_hot_reload()
