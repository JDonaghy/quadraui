"""Black-box coverage for `quadraui_binding_core::DecodeError`'s per-variant
exception taxonomy: `MissingFieldError`, `UnknownVariantError`,
`TypeMismatchError`, `UnknownKeyError`, `VersionMismatchError`, and
`DecodeFailedError` each subclass `InvalidDescriptorError` (so existing
`except InvalidDescriptorError`/`pytest.raises(InvalidDescriptorError,
match=...)` callers keep matching unchanged), carry a `code` class
attribute mirroring `quadraui_binding_core::DecodeError::code` and a `path`
instance attribute mirroring `quadraui_binding_core::DecodeError::path`
verbatim, and raise with a message naming the full path into the layout
tree at which the problem was found -- not just a bare field name.

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
    assert excinfo.value.path == "render › split#frm"
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
    assert excinfo.value.path == "render › split#frm"
    assert "diagonal" in str(excinfo.value)


def test_type_mismatch_names_the_offending_key():
    app = QuiApp(20, 5)

    def render():
        return {"row": "not a list"}

    app.set_render(render)
    with pytest.raises(TypeMismatchError, match="children") as excinfo:
        app.screen()
    assert excinfo.value.code == "type_mismatch"
    assert excinfo.value.path == 'render › "row"'


def test_unknown_key_names_the_typo_d_padding_key():
    app = QuiApp(20, 5)

    def render():
        return {"row": [_button("a", "A")], "padding": {"lef": 4}}

    app.set_render(render)
    with pytest.raises(UnknownKeyError, match="lef") as excinfo:
        app.screen()
    assert excinfo.value.code == "unknown_key"
    assert excinfo.value.path == 'render › "row"'


def test_version_mismatch_names_both_versions():
    app = QuiApp(20, 5)

    def render():
        return {"wire_version": 999, "button": {"id": "b1", "label": "Go"}}

    app.set_render(render)
    with pytest.raises(VersionMismatchError, match="wire_version") as excinfo:
        app.screen()
    assert excinfo.value.code == "version_mismatch"
    assert excinfo.value.path == "render"
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
    assert excinfo.value.path == "render › children[1] › leaf#a"
    assert isinstance(excinfo.value, InvalidDescriptorError)
    assert 'duplicate leaf id "a"' in str(excinfo.value)


def test_a_nested_decode_error_s_path_names_every_ancestor_on_the_way_down():
    # The motivating case: a "ratio" missing several levels into the tree
    # names every ancestor on the way down, not just the render callback's
    # own top-level label -- in both the message and the `path` attribute,
    # which carries `quadraui_binding_core::DecodeError::path` verbatim
    # (the ` › `-joined string the message itself is derived from).
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
    assert excinfo.value.code == "missing_field"
    assert excinfo.value.path == (
        "render › tabs#frm › tab#one › content › panel#chrome › content › split#inner"
    )


def test_unknown_optional_key_is_silently_ignored_with_no_hook_registered():
    # The default policy, with no `on_unknown_key` registered and `strict`
    # left at its default `False`: a misspelt optional key never stops the
    # render, matching every app's behaviour before this policy existed.
    app = QuiApp(20, 5)

    def render():
        return {"row": [_button("a", "A")], "focussed": "a"}

    app.set_render(render)
    app.screen()


def test_unknown_optional_key_warns_through_the_registered_hook_once_per_key():
    # A misspelt optional key ("focussed" for "focused_index") surfaces
    # through `on_unknown_key(key, path)`, at most once per distinct key
    # for this app's whole lifetime, and never stops the render.
    calls = []
    app = QuiApp(20, 5, on_unknown_key=lambda key, path: calls.append((key, path)))

    def render():
        return {
            "tabs": {"id": "frm", "tabs": [], "focussed": 0},
        }

    app.set_render(render)
    app.screen()
    app.screen()
    app.screen()
    assert calls == [("focussed", "render › tabs#frm")]


def test_strict_true_rejects_the_same_unknown_optional_key():
    app = QuiApp(20, 5, strict=True)

    def render():
        return {
            "tabs": {"id": "frm", "tabs": [], "focussed": 0},
        }

    app.set_render(render)
    with pytest.raises(UnknownKeyError, match="focussed") as excinfo:
        app.screen()
    assert excinfo.value.code == "unknown_key"
    assert excinfo.value.path == "render › tabs#frm"


def test_set_strict_toggles_the_policy_after_construction():
    # `set_strict` changes behaviour at runtime, not just at construction:
    # off by default, flipping it on rejects the next render, and flipping
    # it back off decodes the same descriptor unaffected again.
    app = QuiApp(20, 5)

    def render():
        return {
            "tabs": {"id": "frm", "tabs": [], "focussed": 0},
        }

    app.set_render(render)
    app.screen()

    app.set_strict(True)
    with pytest.raises(UnknownKeyError, match="focussed"):
        app.screen()

    app.set_strict(False)
    app.screen()


def test_set_on_unknown_key_registers_a_hook_after_construction():
    calls = []
    app = QuiApp(20, 5)

    def render():
        return {
            "tabs": {"id": "frm", "tabs": [], "focussed": 0},
        }

    app.set_render(render)
    app.set_on_unknown_key(lambda key, path: calls.append((key, path)))
    app.screen()
    app.screen()
    assert calls == [("focussed", "render › tabs#frm")]


def test_set_on_unknown_key_replaces_the_constructor_hook():
    ctor_calls = []
    replaced_calls = []
    app = QuiApp(20, 5, on_unknown_key=lambda key, path: ctor_calls.append((key, path)))

    def render():
        return {
            "tabs": {"id": "frm", "tabs": [], "focussed": 0},
        }

    app.set_render(render)
    app.set_on_unknown_key(lambda key, path: replaced_calls.append((key, path)))
    app.screen()
    assert ctor_calls == []
    assert replaced_calls == [("focussed", "render › tabs#frm")]
