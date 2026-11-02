"""Black-box coverage for the five additive primitive kinds C17 adds:
`editor`, `terminal`, `canvas`, `scroll_view`, and `find_replace`. Each
proves the same three things `test_extra_primitives.py` proves for the
earlier kinds -- it decodes from a plain Python dict via `insert_extra`, it
paints, and (where interactive) a click lands on it and resolves the
enriched event's `widget` to its id -- without adding anything to the
six-slot machinery (`SLOT_*`, `LeafDescriptor`'s six-variant set).

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_missing_primitives.py
"""

from quadraui import QuiApp


def _click_events(app, x, y):
    """Dispatches a left `MouseDown` at `(x, y)` and runs one step,
    returning every event the registered handler captured, in order.
    Mirrors `test_extra_primitives.py`'s own helper of the same name.
    """
    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": x, "y": y},
                "modifiers": {},
            }
        }
    )
    app.run(1)
    return received


def _editor_line(text, line_idx=0):
    return {
        "raw_text": text,
        "gutter_text": str(line_idx + 1),
        "spans": [],
        "line_idx": line_idx,
    }


def _editor_descriptor(widget_id="editor:main"):
    return {
        "id": widget_id,
        "rect": {"x": 0.0, "y": 0.0, "width": 40.0, "height": 10.0},
        "lines": [_editor_line("fn main() {}")],
        "scroll_top": 0,
        "total_lines": 1,
        "is_active": True,
        "show_active_bg": False,
        "has_git_diff": False,
        "has_breakpoints": False,
        "tabstop": 4,
        "cursorline": False,
    }


def test_insert_extra_editor_renders_line_text():
    app = QuiApp(40, 20)
    app.insert_extra("editor", _editor_descriptor())
    assert app.screen_contains("fn main() {}")


def test_clicking_an_editor_focuses_it_and_routes_the_hit():
    # `Editor` is a tab stop, so the click-to-focus step fires a
    # `FocusChanged` ahead of the raw `MouseDown` -- there's no typed
    # semantic event for an editor click yet, so it reaches the handler
    # unchanged, with `widget` resolved to the editor's own id.
    app = QuiApp(40, 20)
    app.insert_extra("editor", _editor_descriptor())

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "editor:main"
    assert received[1]["MouseDown"]["widget"] == "editor:main"
    assert received[1]["hit"] == {"kind": "extra_body"}


def _terminal_cell(text):
    return {
        "text": text,
        "fg": {"r": 200, "g": 200, "b": 200, "a": 255},
        "bg": {"r": 0, "g": 0, "b": 0, "a": 255},
    }


def test_insert_extra_terminal_renders_cell_text():
    app = QuiApp(40, 20)
    app.insert_extra(
        "terminal",
        {"id": "terminal:shell", "cells": [[_terminal_cell("$")]]},
    )
    assert app.screen_contains("$")


def test_clicking_a_terminal_focuses_it_and_routes_the_hit():
    app = QuiApp(40, 20)
    app.insert_extra(
        "terminal",
        {"id": "terminal:shell", "cells": [[_terminal_cell("$")]]},
    )

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "terminal:shell"
    assert received[1]["MouseDown"]["widget"] == "terminal:shell"
    assert received[1]["hit"] == {"kind": "extra_body"}


def test_insert_extra_canvas_decodes_and_paints_without_error():
    # `Canvas` has no sub-regions and is not a tab stop (module doc: no
    # hit-testing of individual ops), so a click still resolves `widget`
    # to it but never focuses it -- a single event, not a `FocusChanged`
    # pair.
    app = QuiApp(40, 20)
    app.insert_extra(
        "canvas",
        {
            "id": "canvas:sparkline",
            "ops": [
                {
                    "Rect": {
                        "rect": {"x": 0.0, "y": 0.0, "width": 10.0, "height": 1.0},
                        "color": {"r": 0, "g": 200, "b": 0, "a": 255},
                    }
                }
            ],
        },
    )

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 1
    assert received[0]["MouseDown"]["widget"] == "canvas:sparkline"


def test_insert_extra_scroll_view_decodes_and_paints_a_scrollbar():
    # 50 content units in a 10-unit viewport overflows, so `ScrollView`'s
    # own `scrollbar()` returns `Some` and the leaf paints a scrollbar
    # thumb -- `app.screen_contains` can't see cell colour, but the insert
    # must not raise and the widget must still be hit-testable/focusable.
    app = QuiApp(40, 20)
    app.insert_extra(
        "scroll_view",
        {
            "id": "scroll:panel",
            "viewport": {"x": 0.0, "y": 0.0, "width": 40.0, "height": 3.0},
            "content_len": 50.0,
        },
    )

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "scroll:panel"
    assert received[1]["MouseDown"]["widget"] == "scroll:panel"


def _find_replace_descriptor():
    # `group_bounds` positions the overlay at its own top-right corner
    # (module doc) within the 3-row band `insert_extra` allots an
    # additive primitive: a narrow `width` here keeps the panel's
    # resolved `x` inside the 40-column screen `QuiApp(40, 20)` below
    # paints into. `hit_regions` drives both click routing AND which
    # content the TUI rasteriser paints (`tui::draw_find_replace` walks
    # it to place the find/replace input text, toggles, and nav
    # buttons) -- a real host builds it via `compute_find_replace_hit_regions`;
    # this test supplies just the one entry it needs directly.
    return {
        "query": "needle",
        "group_bounds": {"x": 0.0, "y": 0.0, "width": 20.0, "height": 24.0},
        "hit_regions": [[{"col": 0, "row": 0, "width": 20}, {"FindInput": 0}]],
    }


def test_insert_extra_find_replace_renders_query():
    app = QuiApp(40, 20)
    app.insert_extra("find_replace", _find_replace_descriptor())
    assert app.screen_contains("needle")


def test_clicking_find_replace_focuses_it_and_routes_the_hit():
    app = QuiApp(40, 20)
    app.insert_extra("find_replace", _find_replace_descriptor())

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "find_replace"
    assert received[1]["MouseDown"]["widget"] == "find_replace"
    assert received[1]["hit"] == {"kind": "extra_body"}
