"""Black-box coverage for the additive primitive mechanism: `insert_progress`,
`insert_spinner`, `insert_image`, the generic `insert_extra` and `clear_extra`
build a `ProgressBar`, `Spinner` and `Image` descriptor from a plain Python
dict, each rendering alongside (not instead of) the six fixed slots, and the
same three kinds also work as a layout-tree leaf (in a `"row"` or `"column"`
container) behind a render callback -- the one other path a descriptor can
reach the screen through.

These three primitives prove the additive mechanism described in the
`EXTRA_PRIMITIVES` doc comment in `quadraui-py/src/lib.rs`: registering a new
one costs an `ExtraPaint` impl (one line for a whole-area primitive like
these three, a little layout derivation for one that needs a measure pass)
and one line appended to the `EXTRA_PRIMITIVES` registry slice -- never an
edit to the six-slot machinery (`SLOT_*`, `is_known_slot`/`clear_slot`,
`LeafDescriptor`'s six-variant set) that `test_descriptors.py` exercises, and
never a new pymethod, since `insert_extra(kind, descriptor)` already covers
any registered kind by name.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_extra_primitives.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp, SlotOccupiedError


def test_insert_progress_renders_label():
    app = QuiApp(40, 20)
    app.insert_progress(
        {"id": "progress:download", "label": "Downloading", "value": 0.5}
    )
    assert app.screen_contains("Downloading")


def test_insert_spinner_renders_label():
    app = QuiApp(40, 20)
    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Indexing"})
    assert app.screen_contains("Indexing")


def test_insert_image_renders_fallback_text():
    app = QuiApp(40, 20)
    app.insert_image(
        {
            "id": "image:logo",
            "source": {"Path": "assets/logo.png"},
            "intrinsic_size": None,
            "fit": "Contain",
            "fallback_text": "[LOGO]",
        }
    )
    assert app.screen_contains("[LOGO]")


def test_extra_primitives_coexist_with_fixed_slots():
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:ok", "label": "OK"})
    app.insert_progress({"id": "progress:download", "label": "Downloading"})
    assert app.screen_contains("OK")
    assert app.screen_contains("Downloading")


def test_insert_progress_rejects_when_already_occupied():
    app = QuiApp(40, 20)
    app.insert_progress({"id": "progress:download", "label": "Downloading"})
    with pytest.raises(SlotOccupiedError):
        app.insert_progress({"id": "progress:other", "label": "Other"})


def test_clear_extra_then_reinsert():
    app = QuiApp(40, 20)
    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Indexing"})
    assert app.clear_extra("spinner") is True
    assert not app.screen_contains("Indexing")

    app.insert_spinner({"id": "spinner:lsp_boot", "label": "Booting"})
    assert app.screen_contains("Booting")


def test_clear_extra_on_empty_slot_returns_false():
    app = QuiApp(40, 20)
    assert app.clear_extra("progress") is False


def test_clear_extra_on_unknown_kind_returns_false():
    # Unlike `slot_clear`, an unrecognized kind has nothing to clear rather
    # than raising, even though `EXTRA_PRIMITIVES` does validate kinds on
    # insert -- `clear_extra` only ever looks a kind up in the occupied-slots
    # map, so a kind that was never registered is indistinguishable from one
    # that is registered but simply not currently occupied.
    app = QuiApp(40, 20)
    assert app.clear_extra("not_a_real_kind") is False


def test_insert_image_invalid_descriptor_raises():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.insert_image({"id": "image:logo"})


def test_insert_extra_renders_by_kind_name():
    # The generic entry point: a future additive primitive reaches the
    # screen through this one method with no new pymethod required.
    app = QuiApp(40, 20)
    app.insert_extra("progress", {"id": "progress:download", "label": "Downloading"})
    assert app.screen_contains("Downloading")


def test_insert_extra_unknown_kind_raises():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.insert_extra("not_a_real_kind", {"id": "x"})


def test_insert_extra_rejects_when_already_occupied():
    app = QuiApp(40, 20)
    app.insert_extra("spinner", {"id": "spinner:lsp_boot", "label": "Indexing"})
    with pytest.raises(SlotOccupiedError):
        app.insert_extra("spinner", {"id": "spinner:other", "label": "Other"})


def test_render_callback_tree_leaf_uses_extra_primitive():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {"id": "p:1", "progress": {"id": "progress:download", "label": "Downloading"}},
                {"id": "s:1", "spinner": {"id": "spinner:lsp_boot", "label": "Indexing"}},
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Downloading")
    assert app.screen_contains("Indexing")


def test_render_callback_tree_leaf_uses_extra_primitive_in_column():
    app = QuiApp(40, 20)

    def render():
        return {
            "column": [
                {"id": "p:1", "progress": {"id": "progress:download", "label": "Downloading"}},
                {"id": "i:1", "image": {
                    "id": "image:logo",
                    "source": {"Path": "assets/logo.png"},
                    "intrinsic_size": None,
                    "fit": "Contain",
                    "fallback_text": "[LOGO]",
                }},
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Downloading")
    assert app.screen_contains("[LOGO]")


def test_render_callback_tree_leaf_rejects_fixed_and_extra_descriptor_together():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "mixed:1",
                    "button": {"id": "demo:ok", "label": "OK"},
                    "progress": {"id": "progress:download", "label": "Downloading"},
                },
            ]
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()


def test_render_callback_tree_leaf_rejects_two_extra_descriptors_together():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "mixed:2",
                    "progress": {"id": "progress:download", "label": "Downloading"},
                    "spinner": {"id": "spinner:lsp_boot", "label": "Indexing"},
                },
            ]
        }

    app.set_render(render)
    with pytest.raises(InvalidDescriptorError):
        app.screen()


def test_insert_extra_text_input_renders_value():
    app = QuiApp(40, 20)
    app.insert_extra("text_input", {"id": "ti:1", "lines": ["hello"]})
    assert app.screen_contains("hello")


def test_insert_extra_number_input_renders_value():
    app = QuiApp(40, 20)
    app.insert_extra(
        "number_input", {"id": "ni:1", "min": 0.0, "max": 100.0, "value": 42.0}
    )
    assert app.screen_contains("42")


def test_insert_extra_radio_group_renders_options():
    app = QuiApp(40, 20)
    app.insert_extra(
        "radio_group",
        {
            "id": "rg:1",
            "options": [
                {"id": "opt:a", "label": "Alpha"},
                {"id": "opt:b", "label": "Beta"},
            ],
        },
    )
    assert app.screen_contains("Alpha")
    assert app.screen_contains("Beta")


def test_insert_extra_switch_renders_knob_and_responds_to_value():
    app_on = QuiApp(40, 20)
    app_on.insert_extra("switch", {"id": "sw:1", "value": True})

    app_off = QuiApp(40, 20)
    app_off.insert_extra("switch", {"id": "sw:1", "value": False})

    # Switch has no label text -- it paints only a 4-cell `[`/knob/`]`
    # track with the knob on the right when `value` is true and on the
    # left when false, so the black-box assertion pins the knob's
    # position directly instead of just diffing the two screens.
    assert app_on.screen_contains("[ ●]")
    assert app_off.screen_contains("[● ]")


def test_insert_extra_combo_box_renders_options_when_open():
    app = QuiApp(40, 20)
    app.insert_extra(
        "combo_box",
        {"id": "combo:1", "options": ["alpha", "beta", "gamma"], "open": True},
    )
    assert app.screen_contains("alpha")


def test_insert_extra_date_picker_renders_header():
    app = QuiApp(40, 20)
    app.insert_extra(
        "date_picker",
        {
            "id": "date:1",
            "visible_month": {"year": 2024, "month": 3, "day": 1},
            "focused": {"year": 2024, "month": 3, "day": 15},
            "locale": {"first_weekday": "Sunday", "format": "Iso8601"},
            "has_focus": True,
        },
    )
    assert app.screen_contains("2024-03")


def test_insert_extra_form_renders_mixed_fields_with_label_position_and_validation():
    # Acceptance criterion: a `Form` with mixed field kinds plus a
    # `label_position` override and a field `validation` state decodes
    # and renders without raising.
    #
    # Every standalone extra gets a fixed 3-row band (see `next_band` in
    # `lib.rs`), so this stays at `LabelPosition::Left` (one row per field,
    # label and input share a row) with exactly three fields -- a `Top`
    # override would cost each field a second row for its label and only
    # the first field or two would fit in the band.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "label_position": "Left",
            "focused_field": None,
            "fields": [
                {
                    "id": "header",
                    "label": {"spans": [{"text": "Settings", "fg": None, "bg": None}]},
                    "kind": "Label",
                },
                {
                    "id": "name",
                    "label": {"spans": [{"text": "Name", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "Alice"}},
                },
                {
                    "id": "enabled",
                    "label": {"spans": [{"text": "Enabled", "fg": None, "bg": None}]},
                    "kind": {"Toggle": {"value": True}},
                    "validation": {"Error": "Required"},
                },
            ],
        },
    )
    assert app.screen_contains("Settings")
    assert app.screen_contains("Name")
    assert app.screen_contains("Alice")
    assert app.screen_contains("Enabled")
    assert app.screen_contains("[x]")


def test_render_callback_flat_dict_still_works_with_extra_key():
    app = QuiApp(40, 20)

    def render():
        return {
            "button": {"id": "demo:ok", "label": "OK"},
            "image": {
                "id": "image:logo",
                "source": {"Path": "assets/logo.png"},
                "intrinsic_size": None,
                "fit": "Contain",
                "fallback_text": "[LOGO]",
            },
        }

    app.set_render(render)
    assert app.screen_contains("OK")
    assert app.screen_contains("[LOGO]")


def test_message_list_renders_row_text():
    app = QuiApp(40, 20)
    app.insert_extra(
        "message_list",
        {
            "id": "messages:1",
            "rows": [
                {"text": "Build succeeded", "fg": {"r": 0, "g": 200, "b": 0, "a": 255}},
            ],
        },
    )
    assert app.screen_contains("Build succeeded")


def test_board_renders_column_title():
    app = QuiApp(40, 20)
    app.insert_extra(
        "board",
        {
            "id": "board:1",
            "columns": [
                {
                    "id": "col:backlog",
                    "title": "Backlog",
                    "cards": [
                        {"id": "card:1", "title": "Fix flaky test"},
                    ],
                },
            ],
        },
    )
    assert app.screen_contains("Backlog")


def test_diff_view_renders_unified_hunk_header():
    app = QuiApp(40, 20)
    app.insert_extra(
        "diff_view",
        {
            "id": "diff:1",
            "left": "foo\n",
            "right": "foo\nbar\n",
            "left_label": None,
            "right_label": None,
            "hunks": [
                {
                    "left_start": 1,
                    "right_start": 1,
                    "rows": [
                        {"left": "foo", "right": "foo", "kind": "Same"},
                        {"left": None, "right": "bar", "kind": "Added"},
                    ],
                },
            ],
            "mode": "Unified",
            "editability": "ReadOnly",
            "scroll_offset": 0,
            "focused_pane": "Left",
            "has_focus": False,
        },
    )
    assert app.screen_contains("@@")


def test_minimap_renders_braille_cells():
    app = QuiApp(40, 20)
    app.insert_extra(
        "minimap",
        {
            "id": "minimap:1",
            "lines": [{"text": "fn main() {}", "line_idx": 0}],
            "syntax_spans": [],
            "visible_row_start": 0,
            "visible_row_count": 1,
            "total_buffer_lines": 1,
        },
    )
    assert app.screen_contains("⠀")


def test_multi_section_view_renders_section_header_title():
    app = QuiApp(40, 20)
    app.insert_extra(
        "multi_section_view",
        {
            "id": "sections:1",
            "sections": [
                {
                    "id": "section:explorer",
                    "header": {
                        "icon": None,
                        "title": {"spans": [{"text": "Explorer", "fg": None, "bg": None}]},
                        "badge": None,
                    },
                    "body": {"Text": []},
                    "size": "EqualShare",
                },
            ],
            "axis": "Vertical",
            "scroll_mode": "PerSection",
        },
    )
    assert app.screen_contains("Explorer")


def test_pipeline_view_renders_stage_labels_as_tree_leaf():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "pl:1",
                    "pipeline_view": {
                        "id": "pipeline:1",
                        "stages": [
                            {"label": "Build", "status": "Done"},
                            {"label": "Deploy", "status": "Pending"},
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Build")
    assert app.screen_contains("Deploy")


def test_chart_renders_x_label_as_tree_leaf():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "c:1",
                    "chart": {
                        "id": "chart:cpu",
                        "kind": "Line",
                        "series": [
                            {"label": "cpu", "data": [1.0, 2.0, 3.0]},
                        ],
                        "x_label": "Time",
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Time")
