"""Black-box coverage for the container/layout primitives bound to Python:
`Panel`, `SidebarPanel` and `SplitTree`, each a `LayoutNode` variant exactly
like the pre-existing `row`/`column`/`split`/`tabs` -- constructed from a
plain Python dict behind a `set_render` callback and recursively holding
any other `LayoutNode` as content/children.

Also covers the two render-callback layout-node gaps closed alongside this
binding work:

- an empty `"tabs"` list renders without error (no tab strip entries, no
  selected content) -- a render callback that starts with zero tabs (e.g.
  before any file is open) doesn't need to special-case the empty case
  itself.
- a `"split"` node's `"first_min"`/`"second_min"` are honoured as a minimum
  pane size, specified in portable line-count units rather than a
  backend-native one.
- a `"row"`/`"column"` node's optional `"padding"`/`"gap"`/`"margin"`
  spacing keys, siblings of the `"row"`/`"column"` key in the same node
  dict, are honoured by the shared layout pass.
- a `"split"`/`"tabs"`/`"panel"` node's own `"padding"`/`"margin"`
  spacing keys, honoured the same way; a `"split"` node's `"gap"` also
  works (the space between its two panes), while `"tabs"`/`"panel"` reject
  `"gap"` as a decode error -- there's no second sibling to space it
  from.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_layout_containers.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def _text(node_id, text):
    return {
        "id": node_id,
        "text_display": {
            "id": node_id,
            "lines": [{"spans": [{"text": text, "fg": None, "bg": None}]}],
        },
    }


def test_panel_renders_nested_content():
    app = QuiApp(40, 20)

    def render():
        return {
            "panel": {
                "id": "panel:outer",
                "title": {"spans": [{"text": "Explorer", "fg": None, "bg": None}]},
                "content": _text("text:body", "PANEL BODY"),
            }
        }

    app.set_render(render)
    assert app.screen_contains("Explorer")
    assert app.screen_contains("PANEL BODY")


def test_clicking_a_panel_action_button_sets_the_click_s_widget_to_its_id():
    # The lone action button is right-aligned in the title bar -- TUI's own
    # 3-cell action width (`TUI_ACTION_BUTTON_WIDTH` in `quadraui::tui::panel`)
    # puts it at x in [37.0, 40.0), row 0 (the title bar). `quadraui-binding-core`'s
    # `InteractionEngine` records the panel's own chrome layout as a hit, so
    # the raw `MouseDown` carries the action's own id as `widget`.
    app = QuiApp(40, 20)

    def render():
        return {
            "panel": {
                "id": "panel:outer",
                "title": {"spans": [{"text": "Explorer", "fg": None, "bg": None}]},
                "actions": [
                    {"id": "close", "icon": "x", "tooltip": "Close", "is_active": False}
                ],
                "content": _text("text:body", "PANEL BODY"),
            }
        }

    app.set_render(render)
    # Force the first paint so the panel's chrome layout is on the hit map
    # before the click below -- `push_event`/`run` only ever re-render on a
    # handler-returned `Reaction::Redraw`, never on their own.
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 38.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    mouse_downs = [event["MouseDown"] for event in received if "MouseDown" in event]
    assert mouse_downs, received
    assert mouse_downs[0]["widget"] == "close", received


def test_clicking_a_panel_title_bar_sets_the_click_s_widget_to_the_panel_id():
    # Clicking the title bar away from any action button (the panel's
    # collapse affordance -- `PanelHit::TitleBar` is the only collapse hit
    # the primitive exposes) must set the raw `MouseDown`'s `widget` to the
    # panel's own id, same shape as the action-button click above.
    app = QuiApp(40, 20)

    def render():
        return {
            "panel": {
                "id": "panel:outer",
                "title": {"spans": [{"text": "Explorer", "fg": None, "bg": None}]},
                "content": _text("text:body", "PANEL BODY"),
            }
        }

    app.set_render(render)
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    mouse_downs = [event["MouseDown"] for event in received if "MouseDown" in event]
    assert mouse_downs, received
    assert mouse_downs[0]["widget"] == "panel:outer", received


def test_clicking_a_sidebar_panel_toolbar_button_sets_the_click_s_widget_to_its_id():
    # The lone toolbar button starts at the toolbar's own x=0, row 0.
    # `quadraui-binding-core`'s `InteractionEngine` records the sidebar
    # panel's own chrome layout as a hit, so the raw `MouseDown` carries
    # the button's own id as `widget`.
    app = QuiApp(40, 20)

    def render():
        return {
            "sidebar_panel": {
                "id": "sidebar:outer",
                "toolbar": {
                    "id": "sidebar:outer:toolbar",
                    "buttons": [{"Action": {"id": "refine", "label": "Refine"}}],
                },
                "content": _text("text:body", "SIDEBAR BODY"),
            }
        }

    app.set_render(render)
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    mouse_downs = [event["MouseDown"] for event in received if "MouseDown" in event]
    assert mouse_downs, received
    assert mouse_downs[0]["widget"] == "refine", received


def test_sidebar_panel_renders_nested_content():
    app = QuiApp(40, 20)

    def render():
        return {
            "sidebar_panel": {
                "id": "sidebar:outer",
                "content": _text("text:body", "SIDEBAR BODY"),
            }
        }

    app.set_render(render)
    assert app.screen_contains("SIDEBAR BODY")


def test_split_tree_renders_two_leaves_side_by_side():
    app = QuiApp(40, 20)

    def render():
        return {
            "split_tree": {
                "id": "tree:1",
                "root": {
                    "split": {
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "first": {"leaf": _text("text:left", "LEFT PANE")},
                        "second": {"leaf": _text("text:right", "RIGHT PANE")},
                    }
                },
            }
        }

    app.set_render(render)
    assert app.screen_contains("LEFT PANE")
    assert app.screen_contains("RIGHT PANE")


def test_dragging_a_split_tree_divider_changes_the_ratio():
    # A 40-column viewport -- the module's usual width, used everywhere else
    # in this file -- rather than a width hand-picked to land the divider on
    # an exact integer column: ratio 0.6 resolves to a fractional `position`
    # (23.4, truncated to column 23 for painting), so the `MouseDown` below
    # exercises the same truncate-to-painted-cell path the TUI rasteriser's
    # own divider glyph is placed with, instead of hiding it behind a width
    # where no truncation was ever needed. The column is read back from the
    # rendered screen (scanning for the `'|'` divider glyph, like
    # `test_split_honors_first_min_as_a_portable_minimum_pane_size` does)
    # rather than hand-derived, so the test can't drift from what was
    # actually painted.
    app = QuiApp(40, 10)

    def render():
        return {
            "split_tree": {
                "id": "tree:1",
                "root": {
                    "split": {
                        "orientation": "horizontal",
                        "ratio": 0.6,
                        "first": {"leaf": _text("text:left", "LEFT PANE")},
                        "second": {"leaf": _text("text:right", "RIGHT PANE")},
                    }
                },
            }
        }

    app.set_render(render)
    # Force the first paint so the tree's divider layout is on the hit map
    # before the drag below -- see the identical comment on the panel
    # action-click test above.
    screen = app.screen()
    divider_column = screen.splitlines()[0].index("│")

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": float(divider_column), "y": 0.0},
                "modifiers": {},
            }
        }
    )
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": float(divider_column) + 10.0, "y": 0.0},
                "buttons": {"left": True, "right": False, "middle": False},
            }
        }
    )
    app.run(2)

    dragged = [event["SplitDividerDragged"] for event in received if "SplitDividerDragged" in event]
    expected_ratio = (divider_column + 10.0) / 40.0
    assert len(dragged) == 1, received
    assert dragged[0]["tree"] == "tree:1"
    assert dragged[0]["split_index"] == 0
    assert abs(dragged[0]["new_ratio"] - expected_ratio) < 0.001, received


def test_tabs_with_empty_list_renders_without_error():
    # #1746: an empty `"tabs"` list is a valid empty state, not an error --
    # a render callback can return a `tabs` node with zero tabs (e.g.
    # before any file is open) without special-casing the empty case.
    app = QuiApp(40, 20)

    def render():
        return {"tabs": {"id": "tabbar:1", "tabs": []}}

    app.set_render(render)
    # No tab strip entries, no selected content -- just must not raise.
    app.screen()


def test_split_honors_first_min_as_a_portable_minimum_pane_size():
    # #1745: `first_min`/`second_min` are portable line-count units, not a
    # backend-native one -- at an extreme ratio that would otherwise
    # collapse the first pane to ~0, a non-zero `first_min` still reserves
    # it a minimum width. The divider's `'|'` column is found by scanning
    # the plain-text `screen()` dump (no bounds/position pymethod is
    # exposed to Python), and compared with vs. without a minimum.
    def divider_column(app):
        screen = app.screen()
        line = screen.splitlines()[0]
        return line.index("│")

    def render_with_min(first_min):
        def render():
            return {
                "split": {
                    "id": "split:1",
                    "orientation": "horizontal",
                    "ratio": 0.02,
                    "first_min": first_min,
                    "first": _text("text:left", "L"),
                    "second": _text("text:right", "R"),
                }
            }

        return render

    app_no_min = QuiApp(40, 10)
    app_no_min.set_render(render_with_min(0))
    col_no_min = divider_column(app_no_min)

    app_with_min = QuiApp(40, 10)
    app_with_min.set_render(render_with_min(10))
    col_with_min = divider_column(app_with_min)

    # Without a minimum the divider sits near ratio*width (~1); with a
    # 10-line minimum it's pushed out to at least that many columns.
    assert col_no_min < 5
    assert col_with_min >= 10


def test_row_padding_insets_content_from_the_container_edge():
    # `"padding"`/`"gap"` are siblings of the `"row"` key in the same node
    # dict, honoured by the shared layout pass. The value is a
    # surface-native-pixel measurement, exactly like `quadraui.Style.padding`
    # -- on a cell-grid backend (TUI) it converts into whole cells via
    # `ceil(px / nominal_cell_px)` (an 8px-wide, 16px-tall nominal cell), not
    # read as that many cells outright. `padding: 12` therefore insets the
    # leaf by `ceil(12 / 8) == 2` columns and `ceil(12 / 16) == 1` row from
    # the top-left corner of a 20x6 screen.
    app = QuiApp(20, 6)

    def render():
        return {
            "row": [_text("text:left", "L")],
            "padding": 12,
        }

    app.set_render(render)
    screen = app.screen()
    lines = screen.splitlines()
    assert lines[0].strip() == "", "no padding rows stay blank"
    assert lines[1][:2] == "  ", "padding columns stay blank"
    assert lines[1].strip() == "L"


def test_row_gap_separates_two_children_by_whole_cells():
    # `"gap"` inserts space between adjacent `"row"` children on the main
    # axis, in surface-native pixels -- two single-column leaves with
    # `gap: 16` land `ceil(16 / 8) == 2` whole TUI columns apart rather
    # than touching.
    app = QuiApp(20, 3)

    def render():
        return {
            "row": [_text("text:left", "L"), _text("text:right", "R")],
            "gap": 16,
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    left_col = line.index("L")
    right_col = line.index("R")
    assert right_col - left_col >= 2 + 1, line


def test_row_with_no_spacing_keys_still_decodes_the_legacy_array_shape():
    # A bare `{"row": [...]}` with no spacing keys at all -- the shape
    # every pre-existing render callback uses -- must still decode and
    # paint with zero inset, unchanged from before spacing keys existed.
    app = QuiApp(10, 3)

    def render():
        return {"row": [_text("text:left", "L")]}

    app.set_render(render)
    assert app.screen().splitlines()[0][0] == "L"


def test_panel_padding_and_margin_inset_the_whole_panel_on_tui():
    # A "panel" node's "padding"/"margin" keys are siblings of the "panel"
    # key, honoured by the same shared layout pass a "row"/"column" node's
    # own spacing keys use -- the whole panel (chrome and content both) is
    # inset before `Backend::draw_panel` paints it. `padding: 12` insets a
    # panel with no title bar by `ceil(12 / 8) == 2` columns and
    # `ceil(12 / 16) == 1` row from the corner of a 10x5 screen.
    app = QuiApp(10, 5)

    def render():
        return {
            "panel": {
                "id": "panel:1",
                "content": _text("text:body", "X"),
            },
            "padding": 12,
        }

    app.set_render(render)
    lines = app.screen().splitlines()
    assert lines[0].strip() == "", "no padding row stays blank"
    assert lines[1][:2] == "  ", "padding columns stay blank"
    assert lines[1].strip() == "X"


def test_panel_rejects_a_gap_key_it_does_not_support():
    # "gap" has no meaning on a "panel" node -- there's no second sibling
    # to space it from -- so it's a decode error, same as "padding"/
    # "margin" were before this node kind supported them.
    app = QuiApp(10, 5)

    def render():
        return {
            "panel": {
                "id": "panel:1",
                "content": _text("text:body", "X"),
            },
            "gap": 8,
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()


def test_tabs_accepts_an_optional_focused_index():
    # B2d: an optional "focused_index" key marks a tab keyboard-focused
    # (painted as reverse video on TUI, see `tui::tab_bar`'s own test for
    # the pixel-level treatment). Passing it must not raise and must not
    # change which tab's content is shown -- that's still "selected"'s job.
    app = QuiApp(10, 8)

    def render():
        return {
            "tabs": {
                "id": "tabbar:1",
                "selected": 0,
                "focused_index": 1,
                "tabs": [
                    {"id": "one", "title": "One", "content": _text("text:one", "O")},
                    {"id": "two", "title": "Two", "content": _text("text:two", "T")},
                ],
            }
        }

    app.set_render(render)
    lines = app.screen().splitlines()
    assert "One" in lines[0]
    assert "Two" in lines[0]
    assert "O" in "".join(lines[1:])


def test_tabs_rejects_an_out_of_range_focused_index():
    # Same out-of-range convention "selected" already uses.
    app = QuiApp(10, 8)

    def render():
        return {
            "tabs": {
                "id": "tabbar:1",
                "focused_index": 5,
                "tabs": [
                    {"id": "one", "title": "One", "content": _text("text:one", "O")},
                ],
            }
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()


def test_tabs_padding_and_margin_inset_the_whole_node_on_tui():
    # A "tabs" node's "padding"/"margin" keys inset the whole node -- tab
    # strip and content both -- before the strip band is reserved.
    # `padding: 12` insets by `ceil(12 / 8) == 2` columns and
    # `ceil(12 / 16) == 1` row; the 3-row-tall strip then starts at row 1
    # and the content starts at row 4 on an 8-row screen.
    app = QuiApp(10, 8)

    def render():
        return {
            "tabs": {
                "id": "tabbar:1",
                "tabs": [
                    {"id": "one", "title": "T", "content": _text("text:body", "X")},
                ],
            },
            "padding": 12,
        }

    app.set_render(render)
    lines = app.screen().splitlines()
    assert lines[0].strip() == "", "no padding row stays blank"
    assert lines[1][:2] == "  ", "padding columns stay blank in the tab strip row"
    content_row = lines[4]
    assert content_row[:2] == "  ", "padding columns stay blank in the content row"
    assert content_row[2] == "X"


def test_tabs_rejects_a_gap_key_it_does_not_support():
    # "gap" has no meaning on a "tabs" node either.
    app = QuiApp(10, 8)

    def render():
        return {
            "tabs": {"id": "tabbar:1", "tabs": []},
            "gap": 8,
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()


def test_split_padding_margin_and_gap_are_honoured_on_tui():
    # A "split" node's "padding"/"margin"/"gap" keys are siblings of the
    # "split" key -- "gap" is the one spacing key with a real meaning here:
    # extra blank space carved out between the divider and the second
    # pane, subtracted from the available width before `first_min`/
    # `second_min` are evaluated. `padding: 8` insets by one column/row
    # each (`ceil(8/8) == 1`, `ceil(8/16) == 1`); `gap: 16` then inserts
    # `ceil(16/8) == 2` blank columns after the 1-column-wide divider.
    app = QuiApp(20, 4)

    def render():
        return {
            "split": {
                "id": "split:1",
                "orientation": "horizontal",
                "ratio": 0.5,
                "first": _text("text:left", "L"),
                "second": _text("text:right", "R"),
            },
            "padding": 8,
            "gap": 16,
        }

    app.set_render(render)
    lines = app.screen().splitlines()
    assert lines[0].strip() == "", "no padding row stays blank"
    content_line = lines[1]
    assert content_line[1] == "L", "first pane starts right after the padding column"
    divider_col = content_line.index("│")
    right_col = content_line.index("R")
    assert right_col - divider_col == 1 + 2, (
        "second pane starts 1 divider column plus the 2-column gap past the divider"
    )


def test_duplicate_leaf_id_inside_panel_content_is_rejected():
    app = QuiApp(40, 20)

    def render():
        return {
            "panel": {
                "id": "panel:outer",
                "content": {
                    "row": [
                        _text("dup:1", "ONE"),
                        _text("dup:1", "TWO"),
                    ]
                },
            }
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()
