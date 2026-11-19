"""Black-box acceptance test for `examples/task_list_demo.py`: drives row
selection through `quadraui.testing`'s headless harness
(`h.find(role="list").row(index=...).select()`), the delete confirm
dialog through `h.press`, and the detail form's title/done fields
through `App.push_event`/`run` -- the same `FormEvent` round trip
`test_settings_form_demo.py` uses, since the harness has no `role="form"`
support yet.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_task_list_demo.py
"""

import importlib.util
import pathlib

from quadraui.testing import harness

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "task_list_demo.py"
)


def _load_task_list_demo():
    spec = importlib.util.spec_from_file_location("task_list_demo", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _form_event(field_id, variant, payload):
    return {"Form": ["tasks:form", {variant: {"id": field_id, **payload}}]}


def test_selecting_a_row_through_the_harness_moves_the_cursor():
    task_list_demo = _load_task_list_demo()
    app, demo = task_list_demo.build()
    h = harness(app)

    h.find(role="list").row(index=2).select()

    assert demo.selected_idx == 2


def test_editing_the_title_field_updates_the_selected_task():
    task_list_demo = _load_task_list_demo()
    app, demo = task_list_demo.build()

    app.push_event(_form_event("tasks:form:title", "TextInputChanged", {"value": "Ship it"}))
    app.run(1)

    assert demo.tasks[0]["title"] == "Ship it"


def test_toggling_done_updates_the_selected_task():
    task_list_demo = _load_task_list_demo()
    app, demo = task_list_demo.build()

    app.push_event(_form_event("tasks:form:done", "ToggleChanged", {"value": True}))
    app.run(1)

    assert demo.tasks[0]["done"] is True


def test_pressing_d_then_enter_deletes_the_selected_task_via_the_confirm_dialog():
    task_list_demo = _load_task_list_demo()
    app, demo = task_list_demo.build()
    h = harness(app)
    before = len(demo.tasks)
    deleted_title = demo.tasks[0]["title"]

    h.press("d")
    assert h.contains(f'Delete "{deleted_title}"?')

    h.press("Enter")

    assert len(demo.tasks) == before - 1
    assert demo.message == "Deleted"
    assert not any(t["title"] == deleted_title for t in demo.tasks)


def test_pressing_d_then_escape_cancels_the_delete():
    task_list_demo = _load_task_list_demo()
    app, demo = task_list_demo.build()
    h = harness(app)
    before = len(demo.tasks)

    h.press("d")
    h.press("Escape")

    assert len(demo.tasks) == before
