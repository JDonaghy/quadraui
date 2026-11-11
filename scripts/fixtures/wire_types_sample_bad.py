"""Negative counterpart of `scripts/fixtures/wire_types_sample.py`: the same two
shapes, each with one key misspelled. `scripts/check_wire_types.sh` asserts
`mypy --strict` reports an error against this file -- proof that the
generated `quadraui.wire` stub actually catches a typo instead of quietly
accepting `Dict[str, Any]`-shaped input.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from quadraui.wire import LayoutNode, TreeViewDescriptor


def bad_misspelled_layout_key() -> "LayoutNode":
    # `"colum"` is not a `LayoutNode` union member's key.
    return {"colum": [{"id": "x", "button": {"id": "x", "label": "x"}}]}


def bad_misspelled_descriptor_field() -> "TreeViewDescriptor":
    return {
        "id": "demo:tree",
        "rows": [],
        "selection_mode": "Single",
        "selected_pathh": [0],  # typo: not a `TreeViewDescriptor` key
        "style": {
            "indent": 2,
            "show_chevrons": True,
            "chevron_expanded": "v",
            "chevron_collapsed": ">",
        },
    }
