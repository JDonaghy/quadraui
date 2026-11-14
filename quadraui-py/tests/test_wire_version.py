"""Black-box coverage for the `"wire_version"` key `quadraui_binding_core::
wire::check_wire_version` accepts on a `set_render` callback's returned
dict -- a versionless tree (every pre-existing render callback), a tree at
the current `quadraui_binding_core::wire_schema::WIRE_VERSION`, and an
older version are all accepted; a version newer than this host understands
is a clear `InvalidDescriptorError`.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_wire_version.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def test_render_tree_with_no_wire_version_key_is_accepted():
    app = QuiApp(20, 5)

    def render():
        return {"row": [{"id": "a", "button": {"id": "a", "label": "OK"}}]}

    app.set_render(render)
    assert app.screen_contains("OK")


def test_render_tree_carrying_wire_version_1_is_accepted():
    app = QuiApp(20, 5)

    def render():
        return {
            "wire_version": 1,
            "row": [{"id": "a", "button": {"id": "a", "label": "OK"}}],
        }

    app.set_render(render)
    assert app.screen_contains("OK")


def test_render_flat_shape_also_accepts_wire_version():
    app = QuiApp(20, 5)

    def render():
        return {"wire_version": 1, "button": {"id": "demo:ok", "label": "OK"}}

    app.set_render(render)
    assert app.screen_contains("OK")


def test_render_wire_version_newer_than_this_host_understands_is_an_error():
    app = QuiApp(20, 5)

    def render():
        return {"wire_version": 999, "button": {"id": "demo:ok", "label": "OK"}}

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError, match="wire_version"):
        app.screen()
