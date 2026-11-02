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
    # `InteractionEngine` now records the panel's own chrome layout as a
    # hit, so the raw `MouseDown` carries the action's own id as `widget`
    # instead of being silently dropped.
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
    # A 41-column viewport with the TUI's 1-cell-thick divider puts a
    # ratio-0.5 divider's own band at x in [20.0, 21.0) -- (41 - 1) * 0.5
    # == 20.0 -- so a `MouseDown` there arms the drag `quadraui-binding-core`'s
    # `InteractionEngine` turns a following `MouseMoved` into
    # `SplitDividerDragged`, same shape dragging a `DataTable` header
    # divider turns into `ColumnResized`.
    app = QuiApp(41, 10)

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
    # Force the first paint so the tree's divider layout is on the hit map
    # before the drag below -- see the identical comment on the panel
    # action-click test above.
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 20.0, "y": 0.0}, "modifiers": {}}}
    )
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": 30.0, "y": 0.0},
                "buttons": {"left": True, "right": False, "middle": False},
            }
        }
    )
    app.run(2)

    dragged = [event["SplitDividerDragged"] for event in received if "SplitDividerDragged" in event]
    assert dragged == [{"tree": "tree:1", "split_index": 0, "new_ratio": 0.75}], received


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
