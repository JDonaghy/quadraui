"""Acceptance test for `examples/settings_form_demo.py`: drives each
field kind's own `FormEvent` through `App.push_event`/`run`, not
`quadraui.testing.harness` -- the harness has no `role="form"` (or
`"checkbox"`/`"slider"`/`"select"`/`"password"`/`"textarea"`) support yet
to query or act on a field/leaf by id without a hardcoded coordinate, so
this stays at the same `App`-level the `Form`/field dispatch itself is
tested at in `test_app_layer.py`, exercising the real engine's own
`event -> handle -> render` round trip the same way `list_demo.py`'s own
headless section does.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_settings_form_demo.py
"""

import importlib.util
import pathlib

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "settings_form_demo.py"
)


def _load_settings_form_demo():
    spec = importlib.util.spec_from_file_location("settings_form_demo", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _form_event(field_id, variant, payload):
    return {"Form": ["settings:form", {variant: {"id": field_id, **payload}}]}


def test_toggling_word_wrap_updates_state():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:word_wrap", "ToggleChanged", {"value": False}))
    app.run(1)

    assert demo.word_wrap is False


def test_typing_a_name_updates_state_on_every_keystroke():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:name", "TextInputChanged", {"value": "Ada"}))
    app.run(1)

    assert demo.display_name == "Ada"


def test_moving_the_slider_updates_state():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:volume", "SliderChanged", {"value": 8.0}))
    app.run(1)

    assert demo.volume == 8.0


def test_picking_a_theme_updates_state():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:theme", "DropdownChanged", {"selected_idx": 2}))
    app.run(1)

    assert demo.theme_idx == 2


def test_password_change_and_submit_update_separate_state():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:password", "TextInputChanged", {"value": "s3cr3t"}))
    app.run(1)
    assert demo.password == "s3cr3t"
    assert demo.password_submitted == ""

    app.push_event(_form_event("settings:password", "TextInputCommitted", {"value": "s3cr3t"}))
    app.run(1)
    assert demo.password_submitted == "s3cr3t"


def test_editing_notes_updates_state():
    settings_form_demo = _load_settings_form_demo()
    app, demo = settings_form_demo.build()

    app.push_event(_form_event("settings:notes", "TextInputChanged", {"value": "line one\nline two"}))
    app.run(1)

    assert demo.notes == "line one\nline two"


def test_view_builds_with_no_raw_wire_dict():
    """`SettingsFormDemo.view()` returns a `Form` of field-builder `Node`s
    only -- rendering it must produce a well-formed `FormDescriptor` with
    every field kind represented, with nothing in `view()` itself ever
    spelling out a `{"form": ...}`/`{"kind": ...}` dict by hand."""
    settings_form_demo = _load_settings_form_demo()
    app, _demo = settings_form_demo.build()

    descriptor = app._render()["form"]
    kinds = {field["id"]: next(iter(field["kind"])) if isinstance(field["kind"], dict) else field["kind"] for field in descriptor["fields"]}
    assert kinds == {
        "settings:word_wrap": "Toggle",
        "settings:name": "TextInput",
        "settings:volume": "Slider",
        "settings:theme": "Dropdown",
        "settings:password": "PasswordInput",
        "settings:notes": "TextArea",
    }
