"""Black-box acceptance test for `examples/list_demo.py`: drives row
selection and activation through `quadraui.testing`'s headless harness
(`h.find(role="list").row(index=...).select()`/`.activate()`) rather than
hand-building a `{"List": [id, {...}]}` wire event -- the same
`event -> handle -> render` round trip a real terminal or GTK backend
delivers.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_list_demo.py
"""

import importlib.util
import pathlib

from quadraui.testing import harness

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "list_demo.py"
)


def _load_list_demo():
    spec = importlib.util.spec_from_file_location("list_demo", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_selecting_a_row_through_the_harness_updates_selected_idx():
    list_demo = _load_list_demo()
    app, demo = list_demo.build()
    h = harness(app)

    h.find(role="list").row(index=2).select()

    assert demo.selected_idx == 2
    assert demo.opened == []


def test_activating_a_row_through_the_harness_opens_it():
    list_demo = _load_list_demo()
    app, demo = list_demo.build()
    h = harness(app)

    h.find(role="list").row(index=2).select()
    h.find(role="list").row(index=2).activate()

    assert demo.opened == ["src/main.rs"]


def test_double_clicking_a_row_through_the_harness_also_opens_it():
    """`on_activate` fires on either `ItemDoubleClicked` or `ItemActivated`
    -- the harness's own `double_click()` drives the mouse path, `activate()`
    the keyboard path, and both must land on the same callback."""
    list_demo = _load_list_demo()
    app, demo = list_demo.build()
    h = harness(app)

    h.find(role="list").row(index=1).double_click()

    assert demo.opened == ["Cargo.toml"]
