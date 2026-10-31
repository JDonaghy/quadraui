"""Black-box coverage for the additive primitive mechanism: `insert_progress`,
`insert_spinner`, `insert_image` and `clear_extra` build a `ProgressBar`,
`Spinner` and `Image` descriptor from a plain Python dict, each rendering
alongside (not instead of) the six fixed slots, and the same three kinds
also work as a layout-tree leaf behind a render callback -- the one other
path a descriptor can reach the screen through.

These three primitives prove the additive mechanism described in the
module docstring of ``quadraui_py.lib``: registering a new one costs one
``ExtraPrimitive`` variant, one string in ``EXTRA_KIND_KEYS``, one decode
arm and one paint arm -- never an edit to the six-slot machinery
(`SLOT_*`, `is_known_slot`/`clear_slot`, `LeafDescriptor`'s six-variant
set) that `test_descriptors.py` exercises.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_extra_primitives.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp, SlotOccupiedError


def test_insert_progress_renders_label():
    app = QuiApp(40, 20)
    app.insert_progress(
        {"id": "progress:download", "label": "Downloading", "value": 0.5}
    )
    assert app.screen_contains("Downloading")


def test_insert_spinner_renders_label():
    app = QuiApp(40, 20)
    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Indexing"})
    assert app.screen_contains("Indexing")


def test_insert_image_renders_fallback_text():
    app = QuiApp(40, 20)
    app.insert_image(
        {
            "id": "image:logo",
            "source": {"Path": "assets/logo.png"},
            "intrinsic_size": None,
            "fit": "Contain",
            "fallback_text": "[LOGO]",
        }
    )
    assert app.screen_contains("[LOGO]")


def test_extra_primitives_coexist_with_fixed_slots():
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:ok", "label": "OK"})
    app.insert_progress({"id": "progress:download", "label": "Downloading"})
    assert app.screen_contains("OK")
    assert app.screen_contains("Downloading")


def test_insert_progress_rejects_when_already_occupied():
    app = QuiApp(40, 20)
    app.insert_progress({"id": "progress:download", "label": "Downloading"})
    with pytest.raises(SlotOccupiedError):
        app.insert_progress({"id": "progress:other", "label": "Other"})


def test_clear_extra_then_reinsert():
    app = QuiApp(40, 20)
    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Indexing"})
    assert app.clear_extra("spinner") is True
    assert not app.screen_contains("Indexing")

    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Booting"})
    assert app.screen_contains("Booting")


def test_clear_extra_on_empty_slot_returns_false():
    app = QuiApp(40, 20)
    assert app.clear_extra("progress") is False


def test_clear_extra_on_unknown_kind_returns_false():
    # Unlike `slot_clear`, an unrecognized kind has nothing to clear rather
    # than raising -- there is no fixed enum of valid kinds to validate
    # against from the Python side.
    app = QuiApp(40, 20)
    assert app.clear_extra("not_a_real_kind") is False


def test_insert_image_invalid_descriptor_raises():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.insert_image({"id": "image:logo"})


def test_render_callback_tree_leaf_uses_extra_primitive():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {"id": "p:1", "progress": {"id": "progress:download", "label": "Downloading"}},
                {"id": "s:1", "spinner": {"id": "spinner:lsp_boot", "label": "Indexing"}},
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Downloading")
    assert app.screen_contains("Indexing")


def test_render_callback_flat_dict_still_works_with_extra_key():
    app = QuiApp(40, 20)

    def render():
        return {
            "button": {"id": "demo:ok", "label": "OK"},
            "image": {
                "id": "image:logo",
                "source": {"Path": "assets/logo.png"},
                "intrinsic_size": None,
                "fit": "Contain",
                "fallback_text": "[LOGO]",
            },
        }

    app.set_render(render)
    assert app.screen_contains("OK")
    assert app.screen_contains("[LOGO]")
