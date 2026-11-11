"""Type-checking fixture for `scripts/gen_wire_types.py`'s generated
`quadraui.wire` stub -- not a runtime script (it is never executed; `python3
-c "import ast; ast.parse(...)"` isn't even run against it in CI). Checked
with:

    MYPYPATH=quadraui-py/python python3 -m mypy --strict \\
        --ignore-missing-imports scripts/fixtures/wire_types_sample.py

Builds a tree + split + data_table layout tree, and matches on a tree-row
click the same way `quadraui-py/examples/file_manager.py`'s own handler
does -- the two acceptance scenarios from the "generated .pyi / TypeScript
types" issue. `build_editor`/`draw_cursor_line` additionally construct an
`EditorDescriptor` (whose stub references `Dict`) and a `DrawOp::Line`
(whose `from` field renders as a functional `TypedDict` instead of a
class), so a regression in either generator path shows up here. This
file must type-check with zero errors;
`scripts/fixtures/wire_types_sample_bad.py` is its negative counterpart -- the same
shapes with one key misspelled each -- which `scripts/check_wire_types.sh`
asserts *fails* to type-check.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from quadraui.wire import (
        ColumnDescriptor,
        DataRowDescriptor,
        DataTableDescriptor,
        DrawOp,
        EditorDescriptor,
        LayoutNode,
        StyledText,
        TreeRowDescriptor,
        TreeViewDescriptor,
        UiEvent,
    )


def _styled_text(text: str) -> "StyledText":
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def build_tree() -> "LayoutNode":
    row: "TreeRowDescriptor" = {
        "path": [0],
        "indent": 0,
        "icon": None,
        "text": _styled_text("src/"),
        "badge": None,
        "is_expanded": True,
    }
    tree: "TreeViewDescriptor" = {
        "id": "demo:tree",
        "rows": [row],
        "selection_mode": "Single",
        "selected_path": [0],
        "style": {
            "indent": 2,
            "show_chevrons": True,
            "chevron_expanded": "v",
            "chevron_collapsed": ">",
        },
    }
    return {"id": "demo:tree", "tree": tree}


def build_table() -> "LayoutNode":
    columns: list["ColumnDescriptor"] = [
        {"title": "Name", "width": {"Flex": 2.0}, "align": "Left"},
        {"title": "Size", "width": {"FixedCells": 10.0}},
    ]
    rows: list["DataRowDescriptor"] = [{"cells": [_styled_text("main.rs"), _styled_text("4 KB")]}]
    table: "DataTableDescriptor" = {"id": "demo:table", "columns": columns, "rows": rows}
    return {"id": "demo:table", "data_table": table}


def build_editor() -> "LayoutNode":
    editor: "EditorDescriptor" = {
        "id": "demo:editor",
        "rect": {"x": 0.0, "y": 0.0, "width": 80.0, "height": 24.0},
        "lines": [
            {"raw_text": "fn main() {}", "gutter_text": "1", "spans": [], "line_idx": 0},
        ],
        "scroll_top": 0,
        "total_lines": 1,
        "is_active": True,
        "show_active_bg": True,
        "has_git_diff": False,
        "has_breakpoints": False,
        "tabstop": 4,
        "cursorline": True,
    }
    return {"id": "demo:editor", "editor": editor}


def draw_cursor_line(x0: float, y0: float, x1: float, y1: float) -> "DrawOp":
    # `DrawOp::Line` has a `from` field -- a Python keyword, which is why
    # `_DrawOpLine` renders as a functional `TypedDict` instead of a class.
    return {
        "Line": {
            "from": {"x": x0, "y": y0},
            "to": {"x": x1, "y": y1},
            "color": {"r": 255, "g": 255, "b": 255, "a": 255},
            "stroke_width": 1.0,
        }
    }


def build_layout() -> "LayoutNode":
    return {
        "split": {
            "id": "demo:split",
            "orientation": "horizontal",
            "ratio": 0.3,
            "first": build_tree(),
            "second": build_table(),
        }
    }


def handle(event: "UiEvent") -> str | None:
    """Mirrors `file_manager.py::FileManager._handle`'s `Tree`/`DataTable`
    dispatch shape."""
    if isinstance(event, str):
        return None
    if "Tree" in event:
        _widget_id, activation = event["Tree"]
        if "RowClicked" in activation:
            path = activation["RowClicked"]["path"]
            return f"clicked {path}"
        if "RowToggleExpand" in activation:
            return f"toggled {activation['RowToggleExpand']['path']}"
    elif "DataTable" in event:
        _widget_id, table_activation = event["DataTable"]
        if "RowDoubleClicked" in table_activation:
            return f"opened row {table_activation['RowDoubleClicked']['idx']}"
    elif "SplitDividerDragged" in event:
        drag = event["SplitDividerDragged"]
        return f"{drag['tree']} -> {drag['new_ratio']}"
    return None
