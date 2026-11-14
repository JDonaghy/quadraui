"""Black-box coverage for the container/layout primitives bound to Python:
`Panel`, `SidebarPanel`, `SplitTree` and `Scroll`, each a `LayoutNode`
variant exactly like the pre-existing `row`/`column`/`split`/`tabs` --
constructed from a plain Python dict behind a `set_render` callback and
recursively holding any other `LayoutNode` as content/children.

`Scroll` additionally covers an uncontrolled scroll position actually
moving: a wheel notch over its own body/scrollbar (or a page key on a
focused child) must both dispatch `ScrollOffsetChanged` through the
handler *and* show up in the next render -- the two are driven by separate
code paths (`InteractionEngine::handle`'s own wheel/key translation writes
the new offset into `WidgetState`; `resolve_layout_node_state`'s own
`LayoutNode::Scroll` arm has to read it back before the next
`paint_layout`), so a test asserting only the dispatched event, or only
the resulting screen, would each miss half of the round trip.

Also covers the two render-callback layout-node gaps closed alongside this
binding work:

- an empty `"tabs"` list renders without error (no tab strip entries, no
  selected content) -- a render callback that starts with zero tabs (e.g.
  before any file is open) doesn't need to special-case the empty case
  itself.
- a `"split"` node's `"first_min"`/`"second_min"` are honoured as a minimum
  pane size, specified in the same portable surface-native-pixel unit
  `"padding"`/`"margin"`/`"gap"` use (quantised against whichever axis the
  split's own `"orientation"` actually bounds -- width for `"horizontal"`,
  height for `"vertical"`), rather than a backend-native one.
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


def test_clicking_a_sidebar_panel_toolbar_button_emits_toolbar_button_clicked():
    # The lone toolbar button starts at the toolbar's own x=0, row 0.
    # `quadraui-binding-core`'s `InteractionEngine` records the sidebar
    # panel's own chrome layout (and its embedded toolbar) as a hit, so the
    # click is delivered as a typed `Toolbar` event naming the bar and button.
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

    toolbar_events = [event["Toolbar"] for event in received if "Toolbar" in event]
    assert toolbar_events, received
    bar, payload = toolbar_events[0]
    assert bar == "sidebar:outer:toolbar", received
    assert payload == {"ButtonClicked": {"id": "refine", "is_active": True}}, received


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
    # `first_min`/`second_min` are in the same portable surface-native-pixel
    # unit `"padding"`/`"margin"`/`"gap"` use, not a backend-native one -- at
    # an extreme ratio that would otherwise collapse the first pane to ~0, a
    # non-zero `first_min` still reserves it a minimum width. `80` quantises
    # to `ceil(80 / 8) == 10` TUI columns, the same 8px-wide nominal cell
    # `"padding"` converts against. The divider's `'|'` column is found by
    # scanning the plain-text `screen()` dump (no bounds/position pymethod
    # is exposed to Python), and compared with vs. without a minimum.
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
    app_with_min.set_render(render_with_min(80))
    col_with_min = divider_column(app_with_min)

    # Without a minimum the divider sits near ratio*width (~1); with an
    # 80px (10-column) minimum it's pushed out to at least that many
    # columns.
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
    # An optional "focused_index" key marks a tab keyboard-focused
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


def test_row_sidebar_fixed_content_weight_inspector_percent_layout():
    # The acceptance scenario: a sidebar (`"Fixed"`), a content pane
    # (`"Weight"`), and an inspector (`"Percent"`) sharing one row. `size`
    # is a sibling of whatever key names a child's own node kind, same as
    # `"padding"` is a sibling of `"row"` -- here that's the leaf dict's
    # own `"id"`/`"text_display"` pair. `Fixed(80)` quantises to `ceil(80
    # / 8) == 10` TUI columns; `Percent(0.2)` of the 40-column screen is
    # 8 columns; the only `Weight` child (there's no `EqualShare` sibling
    # here) absorbs the entire 22-column remainder.
    app = QuiApp(40, 1)

    def render():
        return {
            "row": [
                {**_text("text:sidebar", "S"), "size": {"Fixed": 80}},
                {**_text("text:content", "C"), "size": {"Weight": 1.0}},
                {**_text("text:inspector", "I"), "size": {"Percent": 0.2}},
            ],
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("S") == 0
    assert line.index("C") == 10
    assert line.index("I") == 32


def test_row_child_min_floors_a_fixed_size_below_its_natural_width():
    # A child's `"min"` key is in the same portable pixel unit as `"size"`
    # `Fixed`/`padding`/`margin`/`gap` -- `"size": {"Fixed": 8}` alone
    # would quantise to 1 column, but `"min": 80` floors it to 10.
    app = QuiApp(40, 1)

    def render():
        return {
            "row": [
                {**_text("text:left", "L"), "size": {"Fixed": 8}, "min": 80},
                _text("text:right", "R"),
            ],
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("L") == 0
    assert line.index("R") == 10


def test_row_justify_space_between_spreads_leftover_between_fixed_children():
    # Two 80px (10-column) `Fixed` children in a 40-column row leave 20
    # columns of leftover; `"justify": "space_between"` puts all of it
    # between them rather than after the last one (the `"start"` default).
    app = QuiApp(40, 1)

    def render():
        return {
            "row": [
                {**_text("text:left", "L"), "size": {"Fixed": 80}},
                {**_text("text:right", "R"), "size": {"Fixed": 80}},
            ],
            "justify": "space_between",
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("L") == 0
    assert line.index("R") == 30


def test_row_justify_end_with_a_nonzero_gap_still_reaches_the_far_edge():
    # Two 80px (10-column-wide) `Fixed` children plus an 8px (1-column)
    # `"gap"` in a 40-column row: `"justify": "end"` must push the second
    # child's 10-column-wide rect flush with the row's right edge (so it
    # starts at column 30, filling 30..40), not one column short of it --
    # the gap between children is reserved once, not twice.
    app = QuiApp(40, 1)

    def render():
        return {
            "row": [
                {**_text("text:left", "L"), "size": {"Fixed": 80}},
                {**_text("text:right", "R"), "size": {"Fixed": 80}},
            ],
            "gap": 8,
            "justify": "end",
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("R") == 30


def test_column_align_center_centers_a_narrower_than_stretch_child():
    # `"align": "center"` sizes a child to its own natural cross-axis
    # extent (here, a one-character button's measured width) and centers
    # it, instead of stretching it across the full row width -- the
    # `"stretch"` default every pre-`align` column used.
    app = QuiApp(10, 1)

    def render():
        return {
            "column": [
                {"id": "btn", "button": {"id": "btn", "label": "X"}},
            ],
            "align": "center",
        }

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("X") > 0, "a centered 1-column-wide button is not flush with the left edge"


def test_row_with_no_size_key_still_decodes_the_legacy_leaf_shape():
    # A bare leaf dict with no `"size"`/`"min"`/`"max"` keys -- the shape
    # every pre-existing render callback uses -- still decodes to
    # `EqualShare`, unchanged from before per-child sizing existed.
    app = QuiApp(10, 1)

    def render():
        return {"row": [_text("text:left", "L"), _text("text:right", "R")]}

    app.set_render(render)
    line = app.screen().splitlines()[0]
    assert line.index("L") == 0
    assert line.index("R") == 5


def _buttons(count):
    return [
        {"id": f"btn:{i}", "button": {"id": f"btn:{i}", "label": f"Item{i:02d}"}}
        for i in range(count)
    ]


def test_scroll_renders_its_content_subtree_and_clips_whatever_overflows_it():
    app = QuiApp(40, 10)

    def render():
        return {"scroll": {"id": "scroll:main", "axis": "vertical", "content": {"column": _buttons(40)}}}

    app.set_render(render)
    assert app.screen_contains("Item00")
    assert app.screen_contains("Item09")
    assert not app.screen_contains(
        "Item10"
    ), "content past the viewport must not paint until the scroll actually moves"


def test_scroll_wheel_notch_on_the_scrollbar_dispatches_and_persists_the_new_offset():
    app = QuiApp(40, 10)

    def render():
        return {"scroll": {"id": "scroll:main", "axis": "vertical", "content": {"column": _buttons(40)}}}

    app.set_render(render)
    # Force the first paint so the scrollbar's own track is on the hit map
    # before the wheel notch below -- see the `panel` click test's own
    # comment above for why `push_event`/`run` alone would miss it.
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    # x=39 lands on the vertical scrollbar's own track/thumb (the
    # rightmost column the viewport reserves once content overflows it),
    # so the wheel resolves through `HitDetail::ScrollScrollbarThumb`
    # directly rather than needing a child leaf's own id to look up its
    # enclosing scroll. `ScrollView::handle_wheel`'s own convention: a
    # positive `delta.y` is forward/down.
    app.push_event(
        {"Scroll": {"delta": {"x": 0.0, "y": 3.0}, "position": {"x": 39.0, "y": 1.0}}}
    )
    app.run(1)

    assert received == [{"ScrollOffsetChanged": {"widget": "scroll:main", "new_offset": 3}}]
    assert not app.screen_contains(
        "Item00"
    ), "the resolved offset must reach the next paint, not stay pinned at 0"
    assert app.screen_contains("Item04")

    # The hit map itself must have moved with the paint, not just the
    # pixels: clicking row 0 -- Item00's old screen position -- now hits
    # "btn:3", the child the scroll brought to the top, proving the click
    # router (not only the repainted text) is offset-aware end to end.
    received.clear()
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    mouse_downs = [event["MouseDown"] for event in received if "MouseDown" in event]
    assert mouse_downs, received
    assert mouse_downs[0]["widget"] == "btn:3", received


def test_scroll_page_down_on_a_focused_child_scrolls_the_whole_container():
    app = QuiApp(40, 10)

    def render():
        return {"scroll": {"id": "scroll:main", "axis": "vertical", "content": {"column": _buttons(40)}}}

    app.set_render(render)
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    # Row 5, not row 1 -- far enough from the row-0 click below (more than
    # `DoubleClickDetector`'s 1.5-cell radius) that the two don't fold
    # into a single `DoubleClick`, which would make that second click's
    # own `MouseDown` assertion vacuous.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 5.0}, "modifiers": {}}}
    )
    app.run(1)
    received.clear()
    app.push_event(
        {"KeyPressed": {"key": {"Named": "PageDown"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == [{"ScrollOffsetChanged": {"widget": "scroll:main", "new_offset": 10}}]
    assert app.screen_contains("Item10")
    assert not app.screen_contains("Item00")

    # Clicking the viewport's top row now resolves to "btn:10" -- the
    # button PageDown brought there -- not "btn:0", proving the hit map
    # scrolled along with the paint.
    received.clear()
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    mouse_downs = [event["MouseDown"] for event in received if "MouseDown" in event]
    assert mouse_downs, received
    assert mouse_downs[0]["widget"] == "btn:10", received


def test_scroll_with_a_controlled_offset_pins_it_from_the_very_first_paint():
    # A controlled `Scroll` (`offset` set on every render) must paint
    # pinned on the FIRST frame, before this id has ever appeared in the
    # hit map -- `resolve_layout_node_state`'s `Scroll` arm has no prior
    # frame's `ScrollView` to measure a real `max_offset` from yet, and a
    # `0.0` fallback there would clamp the controlled 15.0 straight back
    # down to the top, exactly the frame this mode exists to pin.
    app = QuiApp(40, 10)

    def render():
        return {
            "scroll": {
                "id": "scroll:main",
                "axis": "vertical",
                "offset": 15.0,
                "content": {"column": _buttons(40)},
            }
        }

    app.set_render(render)
    assert not app.screen_contains(
        "Item00"
    ), "a controlled offset must pin the very first paint, not clamp to 0 for lack of a prior hit"
    assert app.screen_contains("Item15")


def test_scroll_node_helper_builds_the_expected_wire_shape_and_omits_defaults():
    from quadraui.app import Scroll, TextDisplay

    node = Scroll(TextDisplay([]))
    assert node.kind == "scroll"
    assert node.id is None
    built = node.build("scroll:1", {"content": {"text_display": {"id": "x", "lines": []}}})
    assert built == {"scroll": {"id": "scroll:1", "content": {"text_display": {"id": "x", "lines": []}}}}

    pinned = Scroll(
        TextDisplay([]),
        id="scroll:pinned",
        axis="horizontal",
        offset=4.0,
        padding=2,
        margin=1.0,
    )
    assert pinned.id == "scroll:pinned"
    built_pinned = pinned.build("scroll:pinned", {"content": {"ignored": True}})
    assert built_pinned == {
        "scroll": {
            "id": "scroll:pinned",
            "axis": "horizontal",
            "offset": 4.0,
            "content": {"ignored": True},
        },
        "padding": 2,
        "margin": 1.0,
    }
