"""Black-box coverage for `quadraui_binding_core::DecodeError`'s per-variant
exception taxonomy: `MissingFieldError`, `UnknownVariantError`,
`TypeMismatchError`, `UnknownKeyError`, `VersionMismatchError`, and
`DecodeFailedError` each subclass `InvalidDescriptorError` (so existing
`except InvalidDescriptorError`/`pytest.raises(InvalidDescriptorError,
match=...)` callers keep matching unchanged), carry a `code` class
attribute mirroring `quadraui_binding_core::DecodeError::code`, and raise
with a message naming the full path into the layout tree at which the
problem was found -- not just a bare field name.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_decode_error_taxonomy.py
"""

import pytest

from quadraui import (
    DecodeFailedError,
    InvalidDescriptorError,
    MissingFieldError,
    QuiApp,
    TypeMismatchError,
    UnknownKeyError,
    UnknownVariantError,
    VersionMismatchError,
)


def _button(node_id, label):
    return {"id": node_id, "button": {"id": node_id, "label": label}}


def test_missing_field_names_the_split_s_own_path():
    app = QuiApp(20, 5)

    def render():
        return {
            "split": {
                "id": "frm",
                "orientation": "horizontal",
                # "ratio" deliberately omitted.
                "first": _button("a", "A"),
                "second": _button("b", "B"),
            }
        }

    app.set_render(render)
    with pytest.raises(MissingFieldError, match="split#frm") as excinfo:
        app.screen()
    assert excinfo.value.code == "missing_field"
    assert isinstance(excinfo.value, InvalidDescriptorError)


def test_unknown_variant_names_the_bad_orientation_and_the_split_s_path():
    app = QuiApp(20, 5)

    def render():
        return {
            "split": {
                "id": "frm",
                "orientation": "diagonal",
                "ratio": 0.5,
                "first": _button("a", "A"),
                "second": _button("b", "B"),
            }
        }

    app.set_render(render)
    with pytest.raises(UnknownVariantError, match="split#frm") as excinfo:
        app.screen()
    assert excinfo.value.code == "unknown_variant"
    assert "diagonal" in str(excinfo.value)


def test_type_mismatch_names_the_offending_key():
    app = QuiApp(20, 5)

    def render():
        return {"row": "not a list"}

    app.set_render(render)
    with pytest.raises(TypeMismatchError, match="row/column children") as excinfo:
        app.screen()
    assert excinfo.value.code == "type_mismatch"


def test_unknown_key_names_the_typo_d_padding_key():
    app = QuiApp(20, 5)

    def render():
        return {"row": [_button("a", "A")], "padding": {"lef": 4}}

    app.set_render(render)
    with pytest.raises(UnknownKeyError, match="lef") as excinfo:
        app.screen()
    assert excinfo.value.code == "unknown_key"


def test_version_mismatch_names_both_versions():
    app = QuiApp(20, 5)

    def render():
        return {"wire_version": 999, "button": {"id": "b1", "label": "Go"}}

    app.set_render(render)
    with pytest.raises(VersionMismatchError, match="wire_version") as excinfo:
        app.screen()
    assert excinfo.value.code == "version_mismatch"
    assert "999" in str(excinfo.value)


def test_a_duplicate_leaf_id_raises_decode_failed_error_naming_both_leaves():
    # The catch-all `DecodeError::Borrow` variant -- a bespoke cross-field
    # validation (`validate_unique_leaf_ids`) rather than a missing/wrongly
    # typed/unknown field -- raises `DecodeFailedError`, with `code ==
    # "borrow"`, instead of the bare `InvalidDescriptorError` every other
    # non-`DecodeError` raise in this binding also uses.
    app = QuiApp(20, 5)

    def render():
        return {"row": [_button("a", "A"), _button("a", "A")]}

    app.set_render(render)
    with pytest.raises(DecodeFailedError, match="leaf#a") as excinfo:
        app.screen()
    assert excinfo.value.code == "borrow"
    assert isinstance(excinfo.value, InvalidDescriptorError)
    assert 'duplicate leaf id "a"' in str(excinfo.value)


def test_a_nested_decode_error_names_every_ancestor_on_the_way_down():
    # The motivating case: a "ratio" missing several levels into the tree
    # names every ancestor on the way down, not just the render callback's
    # own top-level label.
    app = QuiApp(20, 5)

    def render():
        return {
            "tabs": {
                "id": "frm",
                "tabs": [
                    {
                        "id": "one",
                        "title": "One",
                        "content": {
                            "panel": {
                                "id": "chrome",
                                "content": {
                                    "split": {
                                        "id": "inner",
                                        "orientation": "horizontal",
                                        "first": _button("a", "A"),
                                        "second": _button("b", "B"),
                                    }
                                },
                            }
                        },
                    }
                ],
            }
        }

    app.set_render(render)
    with pytest.raises(MissingFieldError) as excinfo:
        app.screen()
    message = str(excinfo.value)
    assert "tabs#frm" in message
    assert "panel#chrome" in message
    assert "split#inner" in message
