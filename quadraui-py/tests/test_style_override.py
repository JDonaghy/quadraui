"""Black-box coverage for the per-node `"style"` key `quadraui_binding_core::
wire::style_override_from_value` decodes off any layout-tree node dict, and
`quadraui_binding_core::render::paint_layout`'s `LayoutNode::Styled` arm
applies as a scoped `Theme`/`Style` override for that node's own subtree.

Every tree here is wrapped in a `"row"` -- `"style"` only takes effect on
the layout-tree decode path (`quadraui_binding_core::wire::is_layout_tree`
sniffs for `"row"`/`"column"`/`"split"`/..., never a bare leaf key like
`"button"`/`"text_display"` on its own), the same flat-vs-tree split
`test_wire_version.py`'s own `test_render_flat_shape_also_accepts_wire_version`
exercises from the other side.

`app.cell_colors(x, y)` is this surface's own pixel-readback probe (see
`test_theme.py`'s own doc for why), used the same way here: a known,
fixed-size viewport makes the probed coordinate deterministic without
needing a `find`-by-label lookup (`QuiApp` has none -- only `QuiShellApp`
does).

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_style_override.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def test_style_bg_override_on_a_row_changes_a_background_reading_leafs_cells():
    # A `"text_display"` leaf reads `Theme::background` for its own fill
    # (unlike a bordered/filled surface -- see
    # `quadraui_binding_core::layout::StyleOverride`'s own doc table), so
    # a plain `"bg"` override on the enclosing `"row"` is the right probe
    # for the generic "container-ish" case.
    app = QuiApp(10, 1)
    app.set_render(
        lambda: {
            "row": [
                {
                    "id": "td",
                    "text_display": {"id": "td", "lines": [{"spans": [{"text": "x"}]}]},
                }
            ],
            "style": {"bg": "#112233"},
        }
    )
    assert app.cell_colors(0, 0)[1] == (17, 34, 51)


def test_style_override_on_a_nested_leaf_does_not_leak_to_its_sibling():
    # Two equal-share `"text_display"` leaves in a 10-wide row -- each
    # gets 5 columns. Only the second carries its own `"style"` override.
    app = QuiApp(10, 1)
    app.set_render(
        lambda: {
            "row": [
                {
                    "id": "left",
                    "text_display": {"id": "left", "lines": [{"spans": [{"text": "x"}]}]},
                },
                {
                    "id": "right",
                    "text_display": {"id": "right", "lines": [{"spans": [{"text": "x"}]}]},
                    "style": {"bg": "#112233"},
                },
            ]
        }
    )
    default_bg = app.cell_colors(0, 0)[1]
    assert app.cell_colors(1, 0)[1] == default_bg, "the unstyled left child is unaffected"
    assert app.cell_colors(6, 0)[1] == (17, 34, 51), "the styled right child shows its own override"


def test_surface_bg_is_the_alias_a_secondary_button_fill_actually_reads():
    # `"[ Go ]"` at a 6x1 viewport exactly fills the button -- cell (0, 0)
    # is the opening bracket, filled with the same colour as the rest of
    # the button body.
    def app_with(style):
        app = QuiApp(6, 1)
        leaf = {"id": "btn", "button": {"id": "btn", "label": "Go", "variant": "Secondary"}}
        if style is not None:
            leaf["style"] = style
        app.set_render(lambda: {"row": [leaf]})
        return app

    default_bg = app_with(None).cell_colors(0, 0)[1]
    assert app_with({"bg": "#00ff00"}).cell_colors(0, 0)[1] == default_bg, (
        "a plain \"bg\" override must not change a Secondary button's fill -- "
        "it reads Theme::surface_bg, not Theme::background"
    )
    assert app_with({"surface_bg": "#00ff00"}).cell_colors(0, 0)[1] == (0, 255, 0), (
        '"surface_bg" is the alias that actually reaches a Secondary button\'s fill'
    )


def test_style_rejects_an_unknown_key():
    app = QuiApp(10, 1)
    app.set_render(
        lambda: {
            "row": [{"id": "td", "text_display": {"id": "td", "lines": []}}],
            "style": {"background": "#000000"},
        }
    )
    with pytest.raises(InvalidDescriptorError, match="style"):
        app.screen()
