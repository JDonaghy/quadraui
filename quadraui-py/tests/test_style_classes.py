"""Black-box coverage for named style sets: `QuiApp.define_style(name,
overrides)` and the `"class"` key any layout-tree node may carry to pull a
registered set's tokens in -- the CSS-class analogue of the per-node
`"style"` key `test_style_override.py` covers.

Every tree here is wrapped in a `"row"`, and every assertion reads back a
painted cell through `app.cell_colors(x, y)`, for the same reasons
`test_style_override.py`'s own module doc gives.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_style_classes.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def _text_leaf(leaf_id, **extra):
    """One `"text_display"` leaf -- the probe for a plain `"bg"` override,
    since a text display reads `Theme::background` for its own fill."""
    leaf = {
        "id": leaf_id,
        "text_display": {"id": leaf_id, "lines": [{"spans": [{"text": "x"}]}]},
    }
    leaf.update(extra)
    return leaf


def _app_with(leaf, width=10):
    app = QuiApp(width, 1)
    app.set_render(lambda: {"row": [leaf]})
    return app


def test_a_defined_class_applies_its_sets_tokens_to_the_node():
    app = QuiApp(10, 1)
    app.define_style("card", {"bg": "#112233"})
    app.set_render(lambda: {"row": [_text_leaf("td", **{"class": "card"})]})
    assert app.cell_colors(0, 0)[1] == (17, 34, 51)


def test_a_class_does_not_leak_to_an_unclassed_sibling():
    # Two equal-share leaves in a 10-wide row -- 5 columns each. Only the
    # second names the class.
    app = QuiApp(10, 1)
    app.define_style("card", {"bg": "#112233"})
    app.set_render(
        lambda: {
            "row": [
                _text_leaf("left"),
                _text_leaf("right", **{"class": "card"}),
            ]
        }
    )
    default_bg = app.cell_colors(0, 0)[1]
    assert app.cell_colors(1, 0)[1] == default_bg, "the unclassed left child is unaffected"
    assert app.cell_colors(6, 0)[1] == (17, 34, 51)


def test_later_classes_win_the_tokens_earlier_ones_also_named():
    def app_with_classes(classes):
        app = QuiApp(10, 1)
        app.define_style("first", {"bg": "#ff0000"})
        app.define_style("second", {"bg": "#00ff00"})
        app.set_render(lambda: {"row": [_text_leaf("td", **{"class": classes})]})
        return app

    assert app_with_classes(["first", "second"]).cell_colors(0, 0)[1] == (0, 255, 0)
    assert app_with_classes(["second", "first"]).cell_colors(0, 0)[1] == (255, 0, 0)
    # A single whitespace-separated string names the same two sets, in the
    # same order -- the CSS `class=` convention.
    assert app_with_classes("first second").cell_colors(0, 0)[1] == (0, 255, 0)


def test_an_inline_style_key_wins_over_the_nodes_own_class():
    app = QuiApp(10, 1)
    app.define_style("card", {"bg": "#ff0000", "fg": "#0000ff"})
    app.set_render(
        lambda: {
            "row": [_text_leaf("td", style={"bg": "#00ff00"}, **{"class": "card"})]
        }
    )
    fg, bg = app.cell_colors(0, 0)
    assert bg == (0, 255, 0), "the inline \"style\" bg beats the class's own"
    assert fg == (0, 0, 255), "a class token the inline style didn't name still applies"


def test_redefining_a_set_repaints_with_the_new_tokens():
    app = QuiApp(10, 1)
    app.define_style("card", {"bg": "#ff0000"})
    app.set_render(lambda: {"row": [_text_leaf("td", **{"class": "card"})]})
    assert app.cell_colors(0, 0)[1] == (255, 0, 0)

    app.define_style("card", {"bg": "#00ff00"})
    assert app.cell_colors(0, 0)[1] == (0, 255, 0)


def test_an_unknown_class_name_is_ignored_rather_than_raising():
    # The v1 unknown-key policy: a name no set was defined under resolves
    # to nothing, leaving the node painted exactly as if it had no "class"
    # key at all -- unlike an unknown key *inside* "style", which raises.
    plain = _app_with(_text_leaf("td")).cell_colors(0, 0)[1]
    unknown = _app_with(_text_leaf("td", **{"class": "no-such-set"})).cell_colors(0, 0)[1]
    assert unknown == plain

    app = QuiApp(10, 1)
    app.define_style("card", {"bg": "#112233"})
    app.set_render(
        lambda: {"row": [_text_leaf("td", **{"class": ["no-such-set", "card"]})]}
    )
    assert app.cell_colors(0, 0)[1] == (17, 34, 51), "the known name still applies"


def test_builtin_sets_exist_without_any_define_style_call():
    app = QuiApp(10, 1)
    assert app.style_set_names() == ["danger", "primary", "secondary", "subtle"]

    # `"danger"` maps onto the Danger button variant's own resting fill,
    # `Theme::error_fg`, through both the `surface_bg` and `accent_bg`
    # aliases -- so it reaches a `Secondary` button (surface_bg).
    plain = QuiApp(6, 1)
    plain.set_render(
        lambda: {
            "row": [{"id": "b", "button": {"id": "b", "label": "Go", "variant": "Secondary"}}]
        }
    )
    default_bg = plain.cell_colors(0, 0)[1]

    classed = QuiApp(6, 1)
    error_fg = classed.theme()["error_fg"]
    classed.set_render(
        lambda: {
            "row": [
                {
                    "id": "b",
                    "button": {"id": "b", "label": "Go", "variant": "Secondary"},
                    "class": "danger",
                }
            ]
        }
    )
    danger_bg = classed.cell_colors(0, 0)[1]
    assert danger_bg != default_bg
    assert danger_bg == (error_fg["r"], error_fg["g"], error_fg["b"])


def test_a_defined_set_replaces_a_builtin_of_the_same_name():
    app = QuiApp(10, 1)
    app.define_style("danger", {"bg": "#00ff00"})
    app.set_render(lambda: {"row": [_text_leaf("td", **{"class": "danger"})]})
    assert app.cell_colors(0, 0)[1] == (0, 255, 0)
    assert app.style_set_names() == ["danger", "primary", "secondary", "subtle"]


def test_define_style_rejects_an_unknown_override_key():
    app = QuiApp(10, 1)
    with pytest.raises(InvalidDescriptorError, match="unknown key"):
        app.define_style("card", {"background": "#000000"})


def test_define_style_rejects_an_empty_name():
    app = QuiApp(10, 1)
    with pytest.raises(InvalidDescriptorError, match="name"):
        app.define_style("", {"bg": "#000000"})


def test_define_style_rejects_a_name_containing_whitespace():
    # Closes the gap a "class" string/array entry would otherwise open:
    # both split on whitespace (see
    # test_a_whitespace_separated_array_entry_also_names_several_sets), so
    # a name containing it could never be referenced through "class" at
    # all.
    app = QuiApp(10, 1)
    with pytest.raises(InvalidDescriptorError, match="whitespace"):
        app.define_style("card danger", {"bg": "#000000"})


def test_a_whitespace_separated_array_entry_also_names_several_sets():
    app = QuiApp(10, 1)
    app.define_style("first", {"bg": "#ff0000"})
    app.define_style("second", {"bg": "#00ff00"})
    app.set_render(lambda: {"row": [_text_leaf("td", **{"class": ["first second"]})]})
    assert app.cell_colors(0, 0)[1] == (0, 255, 0), "a space inside one array entry still splits"


def test_class_rejects_a_wrongly_typed_value():
    app = QuiApp(10, 1)
    app.set_render(lambda: {"row": [_text_leaf("td", **{"class": 7})]})
    with pytest.raises(InvalidDescriptorError, match="class"):
        app.screen()
