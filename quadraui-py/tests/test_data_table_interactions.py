"""Black-box coverage for `DataTable` interaction semantics: a header click
synthesizes `DataTableEvent::HeaderClicked` (the sort request), dragging a
header divider synthesizes `ColumnResized`, and Down-arrow navigation on a
focused table synthesizes `RowSelected` -- all produced by
`quadraui-binding-core`'s `InteractionEngine` and reaching Python exactly as
`UiEvent::DataTable(id, ...)`, not as a raw `MouseDown`/`KeyPressed` the
handler would have to re-interpret itself.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_data_table_interactions.py
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}

# Two `Fixed(8.0)` columns keep every coordinate below exact: column 0 spans
# viewport x in [0, 8), its divider sits at x == 8, and column 1 spans
# [8, 16). The header occupies row 0 (TUI header height is one cell); body
# rows start at row 1.
_COLUMNS = [
    {"title": "Name", "width": {"Fixed": 8.0}},
    {"title": "Age", "width": {"Fixed": 8.0}},
]


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _row(name, age):
    return {"cells": [_cell(name), _cell(age)]}


def _insert_table(app, selected_idx=None):
    app.insert_data_table(
        {
            "id": "table",
            "columns": _COLUMNS,
            "rows": [_row("Alice", "30"), _row("Bob", "40"), _row("Cleo", "50")],
            "selected_idx": selected_idx,
        }
    )


def test_clicking_a_header_emits_header_clicked():
    app = QuiApp(24, 8)
    _insert_table(app)

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 2.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    assert {"DataTable": ("table", {"HeaderClicked": {"col": 0}})} in received


def test_dragging_a_header_divider_emits_column_resized():
    app = QuiApp(24, 8)
    _insert_table(app)

    received = []
    app.set_handler(lambda event: received.append(event))
    # The divider between column 0 and column 1 sits at viewport x == 8.0.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 8.0, "y": 0.0}, "modifiers": {}}}
    )
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": 13.0, "y": 0.0},
                "buttons": {"left": True, "right": False, "middle": False},
            }
        }
    )
    app.run(2)

    resized = [
        event["DataTable"][1]["ColumnResized"]
        for event in received
        if "DataTable" in event and "ColumnResized" in event["DataTable"][1]
    ]
    assert resized == [{"col": 0, "width": 13.0}], received


def test_single_click_on_a_row_emits_row_selected_not_row_activated():
    app = QuiApp(24, 8)
    _insert_table(app)

    received = []
    app.set_handler(lambda event: received.append(event))
    # Body rows start at viewport y == 1.0, just below the header.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 2.0, "y": 1.0}, "modifiers": {}}}
    )
    app.run(1)

    assert {"DataTable": ("table", {"RowSelected": {"idx": 0}})} in received
    assert not any(
        "DataTable" in event and "RowActivated" in event["DataTable"][1] for event in received
    ), received


def test_double_click_on_a_row_emits_row_double_clicked():
    app = QuiApp(24, 8)
    _insert_table(app)

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"DoubleClick": {"position": {"x": 2.0, "y": 1.0}}}
    )
    app.run(1)

    assert {"DataTable": ("table", {"RowDoubleClicked": {"idx": 0}})} in received


def test_down_arrow_on_a_focused_table_selects_the_next_row():
    app = QuiApp(24, 8)
    _insert_table(app, selected_idx=0)

    received = []
    app.set_handler(lambda event: received.append(event))
    # A click on row 0 (viewport y == 1.0, just below the header) both
    # focuses the table (click-to-focus) and activates the row; the
    # keypress that follows is then routed to the now-focused table.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 2.0, "y": 1.0}, "modifiers": {}}}
    )
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": _NO_MODS, "repeat": False}}
    )
    app.run(2)

    assert {"DataTable": ("table", {"RowSelected": {"idx": 1}})} in received


def test_fixed_px_column_degrades_to_a_nominal_cell_count_on_tui():
    """`ColumnWidth::FixedPx(80.0)` is a pixel width. This app always
    renders through the cell-grid TUI backend, where it degrades to
    `80.0 / 8.0 == 10.0` nominal cells rather than being read as 80 raw
    cells the way `{"FixedCells": 80.0}` would be -- so a header click at
    viewport x == 5.0 (inside the 10-cell-wide column) hits column 0, and
    a click at x == 15.0 (well past its right edge) hits column 1.
    """
    app = QuiApp(30, 8)
    app.insert_data_table(
        {
            "id": "table",
            "columns": [
                {"title": "Name", "width": {"FixedPx": 80.0}},
                {"title": "Age", "width": {"Flex": 1.0}},
            ],
            "rows": [_row("Alice", "30")],
        }
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 5.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    assert {"DataTable": ("table", {"HeaderClicked": {"col": 0}})} in received

    received.clear()
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 15.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    assert {"DataTable": ("table", {"HeaderClicked": {"col": 1}})} in received
