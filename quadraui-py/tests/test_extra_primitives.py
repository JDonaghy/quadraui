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


def test_render_callback_tree_split_and_panel_nest_an_extra_primitive():
    # The container primitives (`Split`, `Panel`, ...) are plain `LayoutNode`
    # recursion, same as `row`/`column` -- this nests a `split` inside a
    # `panel`'s content, with one pane holding a `scrollbar` (an
    # `EXTRA_PRIMITIVES` kind, not one of the six original fixed-slot
    # leaves) alongside an ordinary `text_display` leaf in the other pane.
    #
    # A `Scrollbar` paints from its own absolute `track` rect rather than
    # the leaf area the layout computes for it (it owns its own layout, same
    # as a direct `insert_extra` call outside any node tree), so its track
    # is placed at the second pane's known on-screen column (ratio 0.5 of a
    # 40-wide screen, i.e. column 21 onward) rather than at the pane-local
    # origin.
    app = QuiApp(40, 20)

    def render():
        return {
            "panel": {
                "id": "panel:outer",
                "content": {
                    "split": {
                        "id": "split:1",
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "first": {
                            "id": "text:left",
                            "text_display": {
                                "id": "text:left",
                                "lines": [
                                    {"spans": [{"text": "LEFT", "fg": None, "bg": None}]}
                                ],
                            },
                        },
                        "second": {
                            "id": "s:1",
                            "scrollbar": {
                                "id": "scrollbar:right",
                                "axis": "Vertical",
                                "track": {"x": 21, "y": 0, "width": 1, "height": 10},
                                "thumb_start": 0,
                                "thumb_len": 4,
                            },
                        },
                    },
                },
            },
        }

    app.set_render(render)
    assert app.screen_contains("LEFT")
    assert app.screen_contains("█")  # scrollbar thumb glyph


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


# Parametrised table proving every interactive `EXTRA_PRIMITIVE_KINDS` entry
# that has a dedicated `FormEvent` (`switch` already has its own dedicated
# test above the slot-path block below, and `progress`/`spinner`/`image`
# have no event at all) round-trips through a literal inline layout-tree
# leaf -- a `{"row": [{"id": ..., "<kind>": {...}}]}` dict, the exact shape
# a render callback writes -- not just through `insert_extra`'s flat-slot
# path. Each case pushes the events a real app would dispatch (a focusing
# click, then a key/char where the kind needs one) and asserts the final
# dispatched event the handler receives.
_INLINE_LEAF_CASES = [
    pytest.param(
        {"row": [{"id": "ni:1", "number_input": {"id": "ni:1", "min": 0.0, "max": 100.0, "value": 42.0}}]},
        [{"MouseDown": {"button": "Left", "position": {"x": 38.0, "y": 0.0}, "modifiers": {}}}],
        [
            {"FocusChanged": "ni:1"},
            ("ni:1", {"TextInputChanged": {"id": "ni:1", "value": "43"}}),
        ],
        id="number_input",
    ),
    pytest.param(
        {
            "row": [
                {
                    "id": "rg:1",
                    "radio_group": {
                        "id": "rg:1",
                        "options": [
                            {"id": "opt:a", "label": "Alpha"},
                            {"id": "opt:b", "label": "Beta"},
                        ],
                        "focused": True,
                    },
                }
            ]
        },
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 1.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "rg:1"},
            None,  # the focusing click itself falls through unclaimed
            ("rg:1", {"SegmentedControlChanged": {"id": "rg:1", "selected_idx": 0}}),
        ],
        id="radio_group",
    ),
    pytest.param(
        {
            "row": [
                {
                    "id": "dp:1",
                    "date_picker": {
                        "id": "dp:1",
                        "visible_month": {"year": 2024, "month": 3, "day": 1},
                        "focused": {"year": 2024, "month": 3, "day": 15},
                        "locale": {"first_weekday": "Sunday", "format": "Iso8601"},
                        "has_focus": True,
                    },
                }
            ]
        },
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 1.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "dp:1"},
            None,
            (
                "dp:1",
                {"DateChanged": {"id": "dp:1", "value": {"year": 2024, "month": 3, "day": 15}}},
            ),
        ],
        id="date_picker",
    ),
    pytest.param(
        {
            "row": [
                {
                    "id": "cb:1",
                    "combo_box": {
                        "id": "cb:1",
                        "options": ["alpha", "beta"],
                        "text": "",
                        "focused": True,
                        "open": False,
                    },
                }
            ]
        },
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 1.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Char": "a"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "cb:1"},
            None,
            ("cb:1", {"ComboBoxTextChanged": {"id": "cb:1", "value": "a"}}),
        ],
        id="combo_box",
    ),
    pytest.param(
        {
            "row": [
                {
                    "id": "settings",
                    "form": {
                        "id": "settings",
                        "fields": [
                            {
                                "id": "wrap",
                                "label": {"spans": [{"text": "Word wrap", "fg": None, "bg": None}]},
                                "kind": {"Toggle": {"value": False}},
                            },
                        ],
                    },
                }
            ]
        },
        [{"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}],
        [
            {"FocusChanged": "settings"},
            ("settings", {"ToggleChanged": {"id": "wrap", "value": True}}),
        ],
        id="form",
    ),
    pytest.param(
        {"row": [{"id": "ti:1", "text_input": {"id": "ti:1", "lines": ["hello"]}}]},
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}},
            {"CharTyped": "!"},
        ],
        [
            {"FocusChanged": "ti:1"},
            None,
            ("ti:1", {"TextInputChanged": {"id": "ti:1", "value": "!hello"}}),
        ],
        id="text_input",
    ),
    pytest.param(
        {"row": [{"id": "cb:1", "checkbox": {"id": "cb:1", "value": False}}]},
        [{"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}],
        [
            {"FocusChanged": "cb:1"},
            ("cb:1", {"ToggleChanged": {"id": "cb:1", "value": True}}),
        ],
        id="checkbox",
    ),
    pytest.param(
        {
            "row": [
                {
                    "id": "sl:1",
                    "slider": {"id": "sl:1", "value": 0.0, "min": 0.0, "max": 10.0, "step": 1.0},
                }
            ]
        },
        [
            # x: 0.0 lands focus on the slider without moving it -- the
            # mapped fraction is 0.0, same as the current value -- so the
            # second event (Right) is what actually exercises the change.
            {"MouseDown": {"button": "Left", "position": {"x": 0.0, "y": 0.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Named": "Right"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "sl:1"},
            None,
            ("sl:1", {"SliderChanged": {"id": "sl:1", "value": 1.0}}),
        ],
        id="slider",
    ),
    pytest.param(
        {"row": [{"id": "se:1", "select": {"id": "se:1", "options": ["One", "Two", "Three"]}}]},
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "se:1"},
            # Unlike `radio_group`'s click (unclaimed), a click on a
            # standalone `select` leaf steps the selection itself, so it
            # reaches the handler as `DropdownChanged` too, not `None`.
            ("se:1", {"DropdownChanged": {"id": "se:1", "selected_idx": 1}}),
            ("se:1", {"DropdownChanged": {"id": "se:1", "selected_idx": 1}}),
        ],
        id="select",
    ),
    pytest.param(
        {"row": [{"id": "pw:1", "password": {"id": "pw:1", "value": "hi"}}]},
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}},
            {"CharTyped": "!"},
        ],
        [
            {"FocusChanged": "pw:1"},
            None,
            ("pw:1", {"TextInputChanged": {"id": "pw:1", "value": "hi!"}}),
        ],
        id="password",
    ),
    pytest.param(
        {"row": [{"id": "ta:1", "textarea": {"id": "ta:1", "value": "hi"}}]},
        [
            {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}},
            {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}},
        ],
        [
            {"FocusChanged": "ta:1"},
            None,
            # The focusing click lands at column 0 (the default cursor
            # position for a textarea with no explicit `cursor`), so
            # Enter's inserted newline lands before "hi", not after it.
            ("ta:1", {"TextInputChanged": {"id": "ta:1", "value": "\nhi"}}),
        ],
        id="textarea",
    ),
]


@pytest.mark.parametrize("leaf_tree, events, expected", _INLINE_LEAF_CASES)
def test_inline_tree_leaf_round_trips_semantic_event_per_kind(leaf_tree, events, expected):
    app = QuiApp(40, 20)
    app.set_render(lambda: leaf_tree)
    # `set_render` itself never repaints -- force the first render so the
    # hit map actually reflects this leaf before the events below.
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    for event in events:
        app.push_event(event)
    app.run(len(events))

    assert len(received) == len(expected), received
    for actual, want in zip(received, expected):
        if want is None:
            # The focusing click itself lands as a raw, unclaimed hit --
            # only its `FocusChanged` precursor and the later key/char's
            # semantic event are asserted precisely.
            assert "Form" not in actual, actual
            continue
        if isinstance(want, tuple):
            widget_id, form_event = actual["Form"]
            assert (widget_id, form_event) == want, actual
        else:
            assert actual == want, actual


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


def test_number_input_stepper_click_increments_and_decrements_the_value():
    # `NumberInput::layout` needs no backend metrics (its stepper column
    # is a fixed width), so -- unlike `RadioGroup`/`ComboBox`/`DatePicker`
    # below, which only get coarse click-to-focus -- a click precisely
    # resolving to the increment/decrement half of the stepper column
    # replaces the raw click outright with the `ValueChanged` it stands
    # for, translated into `FormEvent::TextInputChanged` (`NumberInput`
    # has no `FormEvent` slot of its own).
    app = QuiApp(40, 20)
    app.insert_extra("number_input", {"id": "ni:1", "min": 0.0, "max": 100.0, "value": 42.0})
    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 38.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    assert received[0]["FocusChanged"] == "ni:1"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "ni:1"
    assert form_event == {"TextInputChanged": {"id": "ni:1", "value": "43"}}

    received.clear()
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 38.0, "y": 2.0}, "modifiers": {}}}
    )
    app.run(1)
    assert len(received) == 1, "already focused -- no second FocusChanged"
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ni:1"
    assert form_event == {"TextInputChanged": {"id": "ni:1", "value": "41"}}


def test_number_input_up_key_increments_the_focused_value():
    app = QuiApp(40, 20)
    app.insert_extra(
        "number_input",
        {"id": "ni:1", "min": 0.0, "max": 100.0, "value": 42.0, "focused": True},
    )
    received = _click_events(app, 1.0, 1.0)
    assert received[0]["FocusChanged"] == "ni:1"

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Up"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ni:1"
    assert form_event == {"TextInputChanged": {"id": "ni:1", "value": "43"}}


def test_radio_group_arrow_key_changes_the_selection():
    # `RadioGroupEvent::SelectionChanged` has no `FormEvent` slot of its
    # own either -- it's translated into `SegmentedControlChanged`, which
    # carries the exact same `{ id, selected_idx }` shape.
    app = QuiApp(40, 20)
    app.insert_extra(
        "radio_group",
        {
            "id": "rg:1",
            "options": [
                {"id": "opt:a", "label": "Alpha"},
                {"id": "opt:b", "label": "Beta"},
            ],
            "focused": True,
        },
    )
    received = _click_events(app, 1.0, 1.0)
    assert received[0]["FocusChanged"] == "rg:1"

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "rg:1"
    assert form_event == {"SegmentedControlChanged": {"id": "rg:1", "selected_idx": 0}}


def test_combo_box_typing_filters_then_enter_commits():
    # Every `ComboBoxEvent` shape has a dedicated `FormEvent` variant
    # already (`Form`'s own `FieldKind::ComboBox` embeds the same
    # primitive), so typing and `Enter` carry straight through. A render
    # callback echoes `ComboBoxTextChanged`'s `value` back as `text` --
    # same echo-loop contract `_focused_text_input_app` exercises for
    # `text_input` -- so `Enter`'s own `ComboBoxEvent::commit` reads the
    # just-typed text rather than the field's initial, empty one.
    app = QuiApp(40, 20)
    state = {"text": ""}

    def render():
        return {
            "combo_box": {
                "id": "cb:1",
                "options": ["alpha", "beta"],
                "text": state["text"],
                "focused": True,
            }
        }

    app.set_render(render)
    app.screen()
    received = _click_events(app, 1.0, 1.0)
    assert received[0]["FocusChanged"] == "cb:1"

    received = []

    def handler(event):
        received.append(event)
        if "Form" not in event:
            return
        _, form_event = event["Form"]
        if "ComboBoxTextChanged" in form_event:
            state["text"] = form_event["ComboBoxTextChanged"]["value"]

    app.set_handler(handler)
    app.push_event(
        {"KeyPressed": {"key": {"Char": "a"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "cb:1"
    assert form_event == {"ComboBoxTextChanged": {"id": "cb:1", "value": "a"}}

    received.clear()
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "cb:1"
    assert form_event == {"ComboBoxCommitted": {"id": "cb:1", "value": "a"}}


def test_date_picker_enter_confirms_the_focused_day():
    # Only `DatePickerEvent::DateSelected` has a matching `FormEvent` slot
    # (`DateChanged`, which already carries the same `CalendarDate`
    # shape) -- `Enter` confirms the focused day.
    app = QuiApp(40, 20)
    app.insert_extra(
        "date_picker",
        {
            "id": "dp:1",
            "visible_month": {"year": 2024, "month": 3, "day": 1},
            "focused": {"year": 2024, "month": 3, "day": 15},
            "locale": {"first_weekday": "Sunday", "format": "Iso8601"},
            "has_focus": True,
        },
    )
    received = _click_events(app, 1.0, 1.0)
    assert received[0]["FocusChanged"] == "dp:1"

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "dp:1"
    assert form_event == {
        "DateChanged": {"id": "dp:1", "value": {"year": 2024, "month": 3, "day": 15}}
    }


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


def test_insert_extra_form_control_width_aligns_fields_regardless_of_mode():
    # Acceptance criterion: every field kind starts its control at one
    # shared column, regardless of each field's own `control_width`
    # mode. Both values below are the same length, so if the two
    # fields' controls start at the same column, both values start at
    # the same column in their own (different) screen row.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "fill",
                    "label": {"spans": [{"text": "Fill", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "stretches"}},
                },
                {
                    "id": "content",
                    "label": {"spans": [{"text": "Content", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "shortval1"}},
                    "control_width": "Content",
                },
            ],
        },
    )
    lines = app.screen().splitlines()
    fill_col = next(line.find("stretches") for line in lines if "stretches" in line)
    content_col = next(line.find("shortval1") for line in lines if "shortval1" in line)
    assert fill_col != -1
    assert fill_col == content_col


def test_insert_extra_form_control_width_fixed_truncates_without_a_floor():
    # A `Fixed` control narrower than the value's own length truncates
    # it (TUI draws only as many characters as fit the box).
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "clipped",
                    "label": {"spans": [{"text": "A", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "exactwidth"}},
                    "control_width": {"Fixed": 4.0},
                }
            ],
        },
    )
    assert not app.screen_contains("exactwidth")


def test_insert_extra_form_control_width_min_control_width_prevents_clipping():
    # The same too-narrow `Fixed` control from the previous test, but
    # with `min_control_width` floored above the value's own length —
    # the value now renders in full instead of being truncated.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "floored",
                    "label": {"spans": [{"text": "A", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "exactwidth"}},
                    "control_width": {"Fixed": 4.0},
                    "min_control_width": 14.0,
                }
            ],
        },
    )
    assert app.screen_contains("exactwidth")


def test_form_toggle_click_routes_through_form_controller():
    # `FormEvent::ButtonClicked` was never emitted anywhere -- `"form"`
    # painted via `Form`'s own plain, non-interactive `ExtraPaint::paint`
    # with no hit-testing/routing at all. A click on a `Toggle` field now
    # resolves through the cached `FormController`
    # (`InteractionEngine::sync_forms`/`handle`'s own dispatch), the same
    # `FormEvent::ToggleChanged` a `Form`-embedding host driving
    # `FormController` directly would get.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "wrap",
                    "label": {"spans": [{"text": "Word wrap", "fg": None, "bg": None}]},
                    "kind": {"Toggle": {"value": False}},
                },
            ],
        },
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    assert received[0]["FocusChanged"] == "settings"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "settings"
    assert form_event == {"ToggleChanged": {"id": "wrap", "value": True}}


def test_form_button_click_emits_button_clicked():
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "save",
                    "label": {"spans": [{"text": "Save", "fg": None, "bg": None}]},
                    "kind": "Button",
                },
            ],
        },
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)

    assert received[0]["FocusChanged"] == "settings"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "settings"
    assert form_event == {"ButtonClicked": {"id": "save"}}


def test_form_text_field_click_then_typing_routes_through_form_controller():
    # Clicking a `TextInput` field focuses it inside the form
    # (`FormEvent::FocusChanged`, which the host echoes back onto
    # `Form.focused_field` the same way `FormController`'s own module doc
    # describes); typing afterwards must then resolve against THAT
    # field, not fall through as a raw `KeyPressed`.
    app = QuiApp(40, 20)
    state = {"name": "", "focused_field": None}

    def render():
        return {
            "form": {
                "id": "settings",
                "focused_field": state["focused_field"],
                "fields": [
                    {
                        "id": "name",
                        "label": {"spans": [{"text": "Name", "fg": None, "bg": None}]},
                        "kind": {"TextInput": {"value": state["name"]}},
                    },
                ],
            }
        }

    app.set_render(render)
    app.screen()

    received = []

    def handler(event):
        received.append(event)
        if "Form" not in event:
            return
        _, form_event = event["Form"]
        if "TextInputChanged" in form_event:
            state["name"] = form_event["TextInputChanged"]["value"]
        if "FocusChanged" in form_event:
            state["focused_field"] = form_event["FocusChanged"]["id"]

    app.set_handler(handler)
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 6.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    assert received[0]["FocusChanged"] == "settings"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "settings"
    assert form_event == {"FocusChanged": {"id": "name"}}

    received.clear()
    app.push_event(
        {"KeyPressed": {"key": {"Char": "A"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "settings"
    assert form_event == {"TextInputChanged": {"id": "name", "value": "A"}}
    assert state["name"] == "A"


def test_form_descriptor_has_focus_seeds_the_controller_highlight():
    # `FormController::build_form` unconditionally overwrites `has_focus`
    # with its own cached state -- without `sync_forms` seeding that cached
    # state from the descriptor first, a form painted with `has_focus:
    # True` directly (no click, no engine focus at all) would silently
    # lose its focused-row highlight the moment it's routed through
    # `FormController` instead of `Form`'s own plain `ExtraPaint::paint`.
    # `(50, 60, 90)` is `Theme::default()`'s `selected_bg`, paired with the
    # focused row; an unfocused row uses `tab_bar_bg` (`(20, 22, 30)`).
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "has_focus": True,
            "focused_field": "name",
            "fields": [
                {
                    "id": "name",
                    "label": {"spans": [{"text": "Name", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "Alice"}},
                },
            ],
        },
    )
    assert app.cell_colors(0, 0)[1] == (50, 60, 90)


def test_form_descriptor_scroll_offset_seeds_the_controller_view():
    # Same silent-overwrite risk as `has_focus` above, for
    # `FormController`'s own `scroll_offset` -- a descriptor-set
    # `scroll_offset` must still skip straight to the right field with no
    # click/scroll of any kind. The 3-line band fits exactly 3 rows, so
    # `scroll_offset: 2` over 5 one-row `Label` fields should show
    # `Field2`..`Field4` and hide `Field0`/`Field1`.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "scroll_offset": 2,
            "fields": [
                {
                    "id": f"f{i}",
                    "label": {"spans": [{"text": f"Field{i}", "fg": None, "bg": None}]},
                    "kind": "Label",
                }
                for i in range(5)
            ],
        },
    )
    assert app.screen_contains("Field2")
    assert app.screen_contains("Field3")
    assert app.screen_contains("Field4")
    assert not app.screen_contains("Field0")
    assert not app.screen_contains("Field1")


def test_form_focus_highlight_tracks_engine_focus_in_both_directions():
    # Compounding half of the same bug: `set_has_focus(true)` used to be
    # called from exactly one site (the `KeyPressed` block), so (a) a click
    # that moved engine focus onto the form showed no highlight at all
    # until the first keystroke, and (b) once set, it never cleared again
    # even after focus moved elsewhere, for as long as the host kept
    # echoing the same `focused_field`. `sync_forms` now ORs the
    # descriptor's own `has_focus` with whatever `backend`'s focus manager
    # currently names, every frame, so this round-trips both ways with no
    # keystroke involved.
    app = QuiApp(40, 20)
    state = {"name": "Alice", "focused_field": None}

    def render():
        return {
            "form": {
                "id": "settings",
                "focused_field": state["focused_field"],
                "fields": [
                    {
                        "id": "name",
                        "label": {"spans": [{"text": "Name", "fg": None, "bg": None}]},
                        "kind": {"TextInput": {"value": state["name"]}},
                    },
                ],
            },
            # A second focusable extra to move engine focus onto, banded
            # right after the form (`EXTRA_PRIMITIVE_KINDS` puts `"form"`
            # before `"switch"`), so the row below proves the highlight
            # clears once focus actually leaves the form.
            "switch": {"id": "sw:1", "value": False},
        }

    def handler(event):
        if "Form" not in event:
            return
        _, form_event = event["Form"]
        if "FocusChanged" in form_event:
            state["focused_field"] = form_event["FocusChanged"]["id"]

    app.set_render(render)
    app.set_handler(handler)

    # Unfocused: plain row background.
    assert app.cell_colors(0, 0)[1] == (20, 22, 30)

    # Clicking the field moves engine focus onto the form and highlights
    # the row on the very next render -- no keystroke needed.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 6.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    assert state["focused_field"] == "name"
    assert app.cell_colors(0, 0)[1] == (50, 60, 90)

    # Clicking the switch (a different focusable extra) moves engine focus
    # off the form -- the highlight clears even though the host keeps
    # echoing `focused_field: "name"` right back.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 3.0}, "modifiers": {}}}
    )
    app.run(1)
    assert state["focused_field"] == "name"
    assert app.cell_colors(0, 0)[1] == (20, 22, 30)


def test_form_wheel_scroll_advances_the_controller_scroll_offset():
    # `InteractionEngine::handle` routes a wheel `Scroll` landing inside a
    # `"form"` extra's own rect directly through its cached
    # `FormController`, rather than leaving it a dead raw event the way it
    # was before the `MouseUp`/`MouseMoved`/`Scroll` routing this test
    # covers. `FormController`'s own `scroll_offset` is internal state
    # (same as a text cursor) with no `FormEvent` of its own, so this
    # resolves silently -- the assertion is on what's painted, not on a
    # dispatched event.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": f"f{i}",
                    "label": {"spans": [{"text": f"Field{i}", "fg": None, "bg": None}]},
                    "kind": "Label",
                }
                for i in range(10)
            ],
        },
    )
    app.set_handler(lambda event: None)
    assert app.screen_contains("Field0")
    assert not app.screen_contains("Field3")

    app.push_event(
        {
            "Scroll": {
                "delta": {"x": 0.0, "y": -1.0},
                "position": {"x": 1.0, "y": 1.0},
            }
        }
    )
    app.run(1)

    assert app.screen_contains("Field1")
    assert app.screen_contains("Field2")
    assert app.screen_contains("Field3")
    assert not app.screen_contains("Field0")


def test_tab_cycles_focus_across_every_newly_wired_extra_in_paint_order():
    # Acceptance criterion: `form`/`number_input`/`combo_box`/`date_picker`/
    # `radio_group` all newly return `tab_stop() -> true`
    # (`ExtraPaint::tab_stop`), which changes `Tab` behaviour for any app
    # painting them -- `runtime::preprocess_event`'s own step 3 cycles
    # `app.tab_stops()`, built from `InteractionEngine::tab_stops`'s own
    # paint-order traversal of the hit map. One `Tab` per extra, in
    # `EXTRA_PRIMITIVE_KINDS`' registration order (`form` < `number_input`
    # < `combo_box` < `date_picker` < `radio_group`), must focus it.
    app = QuiApp(40, 20)
    app.insert_extra(
        "form",
        {
            "id": "settings",
            "fields": [
                {
                    "id": "name",
                    "label": {"spans": [{"text": "Name", "fg": None, "bg": None}]},
                    "kind": {"TextInput": {"value": "A"}},
                },
            ],
        },
    )
    app.insert_extra(
        "number_input", {"id": "ni:1", "min": 0.0, "max": 100.0, "value": 42.0}
    )
    app.insert_extra(
        "combo_box", {"id": "combo:1", "options": ["alpha", "beta"], "open": False}
    )
    app.insert_extra(
        "date_picker",
        {
            "id": "date:1",
            "visible_month": {"year": 2024, "month": 3, "day": 1},
            "focused": {"year": 2024, "month": 3, "day": 15},
            "locale": {"first_weekday": "Sunday", "format": "Iso8601"},
        },
    )
    app.insert_extra(
        "radio_group",
        {"id": "rg:1", "options": [{"id": "opt:a", "label": "Alpha"}]},
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    for _ in range(5):
        app.push_event(
            {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": {}, "repeat": False}}
        )
        app.run(1)

    assert received == [
        {"FocusChanged": "settings"},
        {"FocusChanged": "ni:1"},
        {"FocusChanged": "combo:1"},
        {"FocusChanged": "date:1"},
        {"FocusChanged": "rg:1"},
    ]


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


def test_activity_bar_renders_item_fallback_glyph():
    # The TUI rasteriser paints only the first character of the resolved
    # glyph/fallback string (see activity_bar.rs), so the fallback here is
    # a single character.
    app = QuiApp(40, 20)
    app.insert_extra(
        "activity_bar",
        {
            "id": "activity_bar:main",
            "top_items": [
                {
                    "id": "activity:explorer",
                    "icon": {"glyph": "", "fallback": "E"},
                    "tooltip": "Explorer",
                    "is_active": True,
                },
            ],
        },
    )
    assert app.screen_contains("E")


def test_toolbar_renders_action_label():
    app = QuiApp(40, 20)
    app.insert_extra(
        "toolbar",
        {
            "id": "toolbar:main",
            "buttons": [
                {"Action": {"id": "toolbar:refresh", "label": "Refresh"}},
            ],
        },
    )
    assert app.screen_contains("Refresh")


def test_menu_bar_renders_item_label():
    app = QuiApp(40, 20)
    app.insert_extra(
        "menu_bar",
        {
            "id": "menu_bar:main",
            "items": [
                {"id": "menu:file", "label": "&File"},
            ],
        },
    )
    assert app.screen_contains("File")


def test_status_bar_renders_segment_text():
    app = QuiApp(40, 20)
    app.insert_extra(
        "status_bar",
        {
            "id": "status_bar:main",
            "left_segments": [
                {
                    "text": "main",
                    "fg": {"r": 255, "g": 255, "b": 255, "a": 255},
                    "bg": {"r": 0, "g": 0, "b": 0, "a": 255},
                },
            ],
            "right_segments": [],
        },
    )
    assert app.screen_contains("main")


def test_clicking_a_toolbar_button_emits_toolbar_button_clicked():
    # A click on a toolbar `Action` button reaches the handler as a typed
    # `Toolbar` event: `(bar_id, {"ButtonClicked": {"id", "is_active"}})`,
    # where `is_active` is the button's flipped state. The clicked button's
    # id (not the bar's) is what resolves against the hit map, and only the
    # bar is a tab stop, so no `FocusChanged` precedes it and `_click`
    # (which asserts exactly one event) applies directly.
    app = QuiApp(40, 20)
    app.insert_extra(
        "toolbar",
        {
            "id": "toolbar:main",
            "buttons": [
                {"Action": {"id": "toolbar:refresh", "label": "Refresh"}},
            ],
        },
    )

    event = _click(app, 1.0, 0.0)

    assert "MouseDown" not in event, event
    bar, payload = event["Toolbar"]
    assert bar == "toolbar:main"
    assert payload == {"ButtonClicked": {"id": "toolbar:refresh", "is_active": True}}


def test_clicking_an_active_toolbar_button_reports_it_toggled_off():
    app = QuiApp(40, 20)
    app.insert_extra(
        "toolbar",
        {
            "id": "toolbar:main",
            "buttons": [
                {"Action": {"id": "toolbar:wrap", "label": "Wrap", "is_active": True}},
            ],
        },
    )

    event = _click(app, 1.0, 0.0)

    bar, payload = event["Toolbar"]
    assert bar == "toolbar:main"
    assert payload == {"ButtonClicked": {"id": "toolbar:wrap", "is_active": False}}


def test_enter_on_a_focused_toolbar_activates_its_focused_button():
    # Tab focuses the toolbar (its only tab stop); Enter then activates the
    # button `focused_index` names, emitting the same `Toolbar` event shape
    # a mouse click does.
    app = QuiApp(40, 20)
    app.insert_extra(
        "toolbar",
        {
            "id": "toolbar:main",
            "focused_index": 1,
            "buttons": [
                {"Action": {"id": "toolbar:filter", "label": "Filter"}},
                {"Action": {"id": "toolbar:reset", "label": "Reset"}},
            ],
        },
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    for key in ("Tab", "Enter"):
        app.push_event(
            {"KeyPressed": {"key": {"Named": key}, "modifiers": {}, "repeat": False}}
        )
        app.run(1)

    toolbar_events = [event["Toolbar"] for event in received if "Toolbar" in event]
    assert toolbar_events, received
    bar, payload = toolbar_events[0]
    assert bar == "toolbar:main"
    assert payload == {"ButtonClicked": {"id": "toolbar:reset", "is_active": True}}


def test_clicking_a_status_bar_segment_emits_segment_clicked():
    app = QuiApp(40, 20)
    app.insert_extra(
        "status_bar",
        {
            "id": "status_bar:main",
            "left_segments": [
                {
                    "text": "main",
                    "fg": {"r": 255, "g": 255, "b": 255, "a": 255},
                    "bg": {"r": 0, "g": 0, "b": 0, "a": 255},
                    "action_id": "status:branch",
                },
            ],
            "right_segments": [],
        },
    )

    event = _click(app, 1.0, 0.0)

    widget_id, status_event = event["StatusBar"]
    assert widget_id == "status_bar:main"
    assert status_event == {"SegmentClicked": {"id": "status:branch"}}


def test_clicking_an_activity_bar_item_emits_item_clicked():
    app = QuiApp(40, 20)
    app.insert_extra(
        "activity_bar",
        {
            "id": "activity_bar:main",
            "top_items": [
                {
                    "id": "activity:explorer",
                    "icon": {"glyph": "", "fallback": "E"},
                    "tooltip": "Explorer",
                    "is_active": True,
                },
            ],
        },
    )

    event = _click(app, 0.0, 0.0)

    widget_id, activity_event = event["ActivityBar"]
    assert widget_id == "activity_bar:main"
    assert activity_event == {"ItemClicked": {"id": "activity:explorer"}}


# The chrome kinds below (`toolbar`, `status_bar`, `activity_bar`,
# `menu_bar`) already have dedicated flat-slot click tests above this
# comment; these four mirror each one through a literal inline
# `{"row": [{"id", "<kind>": {...}}]}` leaf -- the shape a render callback
# writes -- instead of `insert_extra`, proving the chrome-bar leaf path
# (`ExtraPaint::paint_chrome`'s `chrome_hits`, and for `menu_bar`,
# `render_overlays`'s `sync_menu_bar`) carries the same semantic event all
# the way to the handler, not just its paint.
def test_inline_tree_leaf_toolbar_click_emits_toolbar_button_clicked():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "toolbar:main",
                    "toolbar": {
                        "id": "toolbar:main",
                        "buttons": [
                            {"Action": {"id": "toolbar:refresh", "label": "Refresh"}},
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    event = _click(app, 1.0, 0.0)

    assert "MouseDown" not in event, event
    bar, payload = event["Toolbar"]
    assert bar == "toolbar:main"
    assert payload == {"ButtonClicked": {"id": "toolbar:refresh", "is_active": True}}


def test_inline_tree_leaf_status_bar_click_emits_segment_clicked():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "status_bar:main",
                    "status_bar": {
                        "id": "status_bar:main",
                        "left_segments": [
                            {
                                "text": "main",
                                "fg": {"r": 255, "g": 255, "b": 255, "a": 255},
                                "bg": {"r": 0, "g": 0, "b": 0, "a": 255},
                                "action_id": "status:branch",
                            },
                        ],
                        "right_segments": [],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    event = _click(app, 1.0, 0.0)

    widget_id, status_event = event["StatusBar"]
    assert widget_id == "status_bar:main"
    assert status_event == {"SegmentClicked": {"id": "status:branch"}}


def test_inline_tree_leaf_activity_bar_click_emits_item_clicked():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "activity_bar:main",
                    "activity_bar": {
                        "id": "activity_bar:main",
                        "top_items": [
                            {
                                "id": "activity:explorer",
                                "icon": {"glyph": "", "fallback": "E"},
                                "tooltip": "Explorer",
                                "is_active": True,
                            },
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    event = _click(app, 0.0, 0.0)

    widget_id, activity_event = event["ActivityBar"]
    assert widget_id == "activity_bar:main"
    assert activity_event == {"ItemClicked": {"id": "activity:explorer"}}


def test_inline_tree_leaf_menu_bar_alt_activation_emits_menu_activated():
    # `render_overlays`'s `sync_menu_bar` call reads this frame's hit map
    # generically -- it never distinguishes a flat-slot `"menu_bar"` from
    # one painted through an inline leaf -- so an `Alt+F` opens the bar's
    # one top-level menu and `Enter` activates its sole leaf item exactly
    # as it would for a slot-painted bar.
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "menu_bar:main",
                    "menu_bar": {
                        "id": "menu_bar:main",
                        "items": [
                            {
                                "id": "menu:file",
                                "label": "&File",
                                "submenu": [
                                    {
                                        "id": "menu:file:save",
                                        "label": {
                                            "spans": [
                                                {"text": "Save", "fg": None, "bg": None}
                                            ]
                                        },
                                    },
                                ],
                            },
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {"KeyPressed": {"key": {"Char": "f"}, "modifiers": {"alt": True}, "repeat": False}}
    )
    app.run(1)
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert {"MenuActivated": "menu:file:save"} in received, received


def test_inline_tree_leaf_markdown_renders_formatting_and_link_click_emits_event():
    # `"markdown"` reuses `render_markdown_to_styled` (the same adapter
    # `push_turn_markdown`/`RichTextPopup` content already goes through),
    # not a second parser -- asserted here by checking the raw syntax is
    # gone from the painted screen (a renderer bypass that printed `source`
    # verbatim would still pass a plain `screen_contains` check on each
    # substring, since every one of them is also present in the raw text).
    # The link click resolves through the same `ExtraChrome` chrome-hit
    # mechanism `toolbar`/`status_bar`/`activity_bar` above use, into
    # `RichTextLinkClicked` -- the event `RichTextPopup`'s own link click
    # already dispatches -- so this gets its own test rather than a row in
    # `_INLINE_LEAF_CASES`, matching those three.
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "md:1",
                    "markdown": {
                        "id": "md:1",
                        "source": "# Title\n**bold** *italic* `code`\n- item\n[docs](https://example.com/docs)",
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    assert app.screen_contains("Title")
    assert app.screen_contains("bold")
    assert app.screen_contains("italic")
    assert app.screen_contains("code")
    assert app.screen_contains("item")
    assert app.screen_contains("docs")
    assert not app.screen_contains("# Title")
    assert not app.screen_contains("**bold**")
    assert not app.screen_contains("*italic*")
    assert not app.screen_contains("`code`")
    assert not app.screen_contains("[docs]")
    assert app.screen_contains("•")

    # `QuiApp` (unlike `QuiShellApp`) has no `find` helper, so the link's
    # cell position is located the same way `test_insert_extra_form_control_width_*`
    # above locates a column: scan `screen()`'s own rows/columns directly.
    lines = app.screen().splitlines()
    y = next(i for i, line in enumerate(lines) if "docs" in line)
    x = lines[y].find("docs")
    event = _click(app, float(x), float(y))

    widget_id, url = event["RichTextLinkClicked"]
    assert widget_id == "md:1"
    assert url == "https://example.com/docs"


def test_inline_tree_leaf_find_replace_renders_query_text():
    # `FindReplacePanel` carries no `id` field of its own (see
    # `ExtraPaint::id`'s fixed-sentinel doc); `test_missing_primitives.py`
    # already covers it via `insert_extra`, so this proves the same
    # painter also works from an inline layout leaf. `hit_regions` drives
    # both click routing AND which content the TUI rasteriser paints
    # (`tui::draw_find_replace` walks it to place the find/replace input
    # text) -- a real host builds it via `compute_find_replace_hit_regions`;
    # this test supplies just the one entry it needs directly. An inline
    # leaf gets the full 40-column viewport rather than `insert_extra`'s
    # narrower band, so a `width` of 20 keeps the region inside it.
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "fr:1",
                    "find_replace": {
                        "query": "needle",
                        "group_bounds": {"x": 0.0, "y": 0.0, "width": 40.0, "height": 20.0},
                        "hit_regions": [[{"col": 0, "row": 0, "width": 20}, {"FindInput": 0}]],
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("needle")


def test_command_line_renders_text():
    app = QuiApp(40, 20)
    app.insert_extra("command_line", {"id": "command_line:main", "text": ":wq"})
    assert app.screen_contains(":wq")


def test_inline_tree_leaf_command_line_renders_text():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "command_line:main",
                    "command_line": {"id": "command_line:main", "text": ":wq"},
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains(":wq")


def test_command_center_renders_search_label():
    app = QuiApp(40, 20)
    app.insert_extra(
        "command_center",
        {
            "id": "command_center:main",
            "back_enabled": True,
            "forward_enabled": False,
            "search_label": "my-project",
        },
    )
    assert app.screen_contains("my-project")


def test_inline_tree_leaf_command_center_renders_search_label():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "command_center:main",
                    "command_center": {
                        "id": "command_center:main",
                        "back_enabled": True,
                        "forward_enabled": False,
                        "search_label": "my-project",
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("my-project")


def test_palette_renders_item_text():
    # Palette's TUI rasteriser refuses to paint anything below a 4x4 area,
    # and `insert_extra` only ever gives an additive primitive a fixed
    # 3-row band, so this one routes through a render-callback `row` leaf
    # instead, which hands the leaf the full viewport.
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "palette:1",
                    "palette": {
                        "id": "palette:commands",
                        "title": "Commands",
                        "query": "",
                        "items": [
                            {"text": {"spans": [{"text": "Save File", "fg": None, "bg": None}]}},
                        ],
                        "selected_idx": 0,
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Save File")


# The data-view kinds below (`message_list`, `board`, `diff_view`, `minimap`,
# `multi_section_view`) already have a dedicated flat-slot `insert_extra`
# paint test above this comment, the same way `chart`/`pipeline_view` do
# just above `test_activity_bar_renders_item_fallback_glyph`. Each one here
# mirrors that same descriptor through a literal inline
# `{"row": [{"id", "<kind>": {...}}]}` leaf instead, proving
# `layout_leaf_from_value`'s generic `EXTRA_PRIMITIVE_KINDS` decode (and
# `paint_leaf`'s matching `LeafDescriptor::Extra` paint) covers every
# data-view kind, not just the two already proven this way.
def test_inline_tree_leaf_message_list_renders_row_text():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "messages:1",
                    "message_list": {
                        "id": "messages:1",
                        "rows": [
                            {
                                "text": "Build succeeded",
                                "fg": {"r": 0, "g": 200, "b": 0, "a": 255},
                            },
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Build succeeded")


def test_inline_tree_leaf_board_renders_column_title():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "board:1",
                    "board": {
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
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Backlog")


def test_inline_tree_leaf_diff_view_renders_unified_hunk_header():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "diff:1",
                    "diff_view": {
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
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("@@")


def test_inline_tree_leaf_minimap_renders_braille_cells():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "minimap:1",
                    "minimap": {
                        "id": "minimap:1",
                        "lines": [{"text": "fn main() {}", "line_idx": 0}],
                        "syntax_spans": [],
                        "visible_row_start": 0,
                        "visible_row_count": 1,
                        "total_buffer_lines": 1,
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("⠀")


def test_inline_tree_leaf_multi_section_view_renders_section_header_title():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "sections:1",
                    "multi_section_view": {
                        "id": "sections:1",
                        "sections": [
                            {
                                "id": "section:explorer",
                                "header": {
                                    "icon": None,
                                    "title": {
                                        "spans": [
                                            {"text": "Explorer", "fg": None, "bg": None}
                                        ]
                                    },
                                    "badge": None,
                                },
                                "body": {"Text": []},
                                "size": "EqualShare",
                            },
                        ],
                        "axis": "Vertical",
                        "scroll_mode": "PerSection",
                    },
                },
            ]
        }

    app.set_render(render)
    assert app.screen_contains("Explorer")


# `editor`/`terminal`/`canvas` already have their own `insert_extra`
# flat-slot paint-and-click coverage in `test_missing_primitives.py` (C17).
# These three mirror that same click behaviour (click-to-focus for the two
# tab stops, a bare routed hit for the non-focusable `Canvas`) through a
# literal inline leaf instead, proving the generic `LeafDescriptor::Extra`
# path carries `ExtraPaint::hit_test`/`tab_stop` the same way a flat-slot
# descriptor does.
def test_inline_tree_leaf_editor_focuses_it_and_routes_the_hit():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "editor:main",
                    "editor": {
                        "id": "editor:main",
                        "rect": {"x": 0.0, "y": 0.0, "width": 40.0, "height": 10.0},
                        "lines": [
                            {
                                "raw_text": "fn main() {}",
                                "gutter_text": "1",
                                "spans": [],
                                "line_idx": 0,
                            },
                        ],
                        "scroll_top": 0,
                        "total_lines": 1,
                        "is_active": True,
                        "show_active_bg": False,
                        "has_git_diff": False,
                        "has_breakpoints": False,
                        "tabstop": 4,
                        "cursorline": False,
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "editor:main"
    assert received[1]["MouseDown"]["widget"] == "editor:main"


def test_inline_tree_leaf_terminal_focuses_it_and_routes_the_hit():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "terminal:shell",
                    "terminal": {
                        "id": "terminal:shell",
                        "cells": [
                            [
                                {
                                    "text": "$",
                                    "fg": {"r": 200, "g": 200, "b": 200, "a": 255},
                                    "bg": {"r": 0, "g": 0, "b": 0, "a": 255},
                                }
                            ]
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "terminal:shell"
    assert received[1]["MouseDown"]["widget"] == "terminal:shell"


def test_inline_tree_leaf_canvas_routes_the_hit_without_focusing():
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {
                    "id": "canvas:sparkline",
                    "canvas": {
                        "id": "canvas:sparkline",
                        "ops": [
                            {
                                "Rect": {
                                    "rect": {
                                        "x": 0.0,
                                        "y": 0.0,
                                        "width": 10.0,
                                        "height": 1.0,
                                    },
                                    "color": {"r": 0, "g": 200, "b": 0, "a": 255},
                                }
                            }
                        ],
                    },
                },
            ]
        }

    app.set_render(render)
    app.screen()

    event = _click(app, 1.0, 1.0)

    assert event["MouseDown"]["widget"] == "canvas:sparkline"


def _click(app, x, y):
    """Dispatches a left `MouseDown` at `(x, y)` and runs one step, returning
    the single event the registered handler captured.

    Asserts there was exactly one event: no focusable widget sits under
    `(x, y)`, so click-to-focus has nothing to do and no `FocusChanged`
    precedes the click.
    """
    received = _click_events(app, x, y)
    assert len(received) == 1
    return received[0]


def _click_events(app, x, y):
    """Dispatches a left `MouseDown` at `(x, y)` and runs one step, returning
    every event the registered handler captured, in order."""
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


def test_clicking_a_switch_routes_a_hit_with_widget_set():
    # `insert_extra`'s own render already populated the hit map, so the
    # switch -- the first (and only) extra, painted into the top 3-line
    # band -- is hit-testable the moment it's inserted. `Switch` is also a
    # tab stop, so the click-to-focus step fires a `FocusChanged` ahead of
    # the click itself, and `semantic_event_for_hit` rewrites the click into
    # a `Form`/`ToggleChanged` event rather than leaving it as a raw
    # `MouseDown` with a `"hit"` dict.
    app = QuiApp(40, 20)
    app.insert_extra("switch", {"id": "settings:wrap"})

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2, (
        "the click must emit both a FocusChanged (nothing was focused yet) "
        "and the toggle itself"
    )
    assert received[0]["FocusChanged"] == "settings:wrap"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "settings:wrap"
    assert form_event == {"ToggleChanged": {"id": "settings:wrap", "value": True}}


def _focused_text_input_app(initial_value, click_x=1.0):
    """Builds a `QuiApp` with a single, already-focused `text_input`
    extra wired to the "echo loop" a real host runs: a render callback
    always reflects `state["value"]`, and the returned `handler` (already
    registered) writes every `TextInputChanged`/`TextInputCommitted`
    event's `value` back into it. `TuiDriver::dispatch` re-renders inline
    whenever a handler's `Reaction` says to (promoted to `Redraw` on every
    non-exit return once a render callback is registered), so this happens
    before the *next* pushed event is dispatched -- exactly the timing a
    live app gets.

    The focusing click itself now also places the caret (click-to-caret),
    so the cursor a subsequent keystroke edits from is whatever column
    `click_x` resolves to against the field's own layout, not a value the
    host could set on the descriptor -- real `FormEvent::TextInputChanged`/
    `Committed` carry no cursor for a host to echo, so there is nothing
    else for it to seed from. Returns `(app, state, received)`; `received`
    accumulates every event the handler saw, in order.
    """
    app = QuiApp(40, 20)
    state = {"value": initial_value}

    def render():
        return {"text_input": {"id": "ti:1", "lines": [state["value"]]}}

    app.set_render(render)
    # `set_render` itself never repaints -- force the first render so the
    # hit map actually reflects this descriptor before the click below.
    app.screen()
    focus_and_click = _click_events(app, click_x, 1.0)
    assert focus_and_click[0]["FocusChanged"] == "ti:1"

    received = []

    def handler(event):
        received.append(event)
        if "Form" not in event:
            return
        _, form_event = event["Form"]
        changed = form_event.get("TextInputChanged") or form_event.get(
            "TextInputCommitted"
        )
        if changed is not None:
            state["value"] = changed["value"]

    app.set_handler(handler)
    return app, state, received


def test_text_input_char_typed_and_paste_produce_form_events():
    # The focusing click (at `click_x=7.0`) places the caret after the
    # 'o' of "hello" via click-to-caret; the host then echoes each
    # `TextInputChanged` back as `lines` but never learns the resulting
    # cursor position (the event carries none). The paste must still land
    # after the "!" the first edit produced ("hello! world"), not back at
    # the click position -- proving the cursor used for the second edit
    # comes from `apply_text_input_edit`'s own cache, not the descriptor.
    app, state, received = _focused_text_input_app("hello", click_x=7.0)

    app.push_event({"CharTyped": "!"})
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hello!"}}
    assert state["value"] == "hello!"

    received.clear()
    app.push_event({"ClipboardPaste": " world"})
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hello! world"}}


def test_text_input_multiple_char_typed_appends_in_order():
    # Typing "a", "b", "c" in sequence -- each landing after the host's
    # echo of the previous keystroke re-renders -- must append each
    # character after the last: "a", "ab", "abc". Without a cursor cache,
    # every keystroke lands at column 0 of whatever the host just echoed,
    # producing "a", "ba", "cba" instead.
    app, state, received = _focused_text_input_app("")

    for ch in "abc":
        app.push_event({"CharTyped": ch})
    app.run(3)

    values = [
        form_event["TextInputChanged"]["value"]
        for _, form_event in (e["Form"] for e in received)
    ]
    assert values == ["a", "ab", "abc"]
    assert state["value"] == "abc"


def test_text_input_backspace_deletes_at_cached_cursor():
    # The focusing click (at `click_x=4.0`) places the caret after the
    # 'i' of "hi" via click-to-caret. `Backspace` right after a
    # `CharTyped` must delete the character the cached cursor says it's
    # after (the "!" just inserted), landing back on "hi" -- not on a
    # stale pre-edit cursor position.
    app, state, received = _focused_text_input_app("hi", click_x=4.0)

    app.push_event({"CharTyped": "!"})
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Named": "Backspace"},
                "modifiers": {},
                "repeat": False,
            }
        }
    )
    app.run(2)

    values = [
        form_event["TextInputChanged"]["value"]
        for _, form_event in (e["Form"] for e in received)
    ]
    assert values == ["hi!", "hi"]
    assert state["value"] == "hi"


def test_text_input_ctrl_letter_passes_through_unclaimed():
    # A Ctrl+letter chord on a focused text input is a host accelerator,
    # not a character to insert -- it must reach Python as the raw
    # `KeyPressed`, not a `Form`/`TextInputChanged`.
    app, _state, received = _focused_text_input_app("hi")

    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "s"},
                "modifiers": {"ctrl": True},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    assert "KeyPressed" in received[0]
    assert received[0]["KeyPressed"]["key"] == {"Char": "s"}


def test_text_input_arrow_key_falls_through_as_raw_key_pressed():
    # `Left`/`Right`/etc move the cursor but never change the text, so
    # `apply_text_input_edit` reports no change and the caller falls back
    # to the plain `KeyPressed` -- the cursor move itself is still cached
    # (exercised by the backspace-after-typing test above landing in the
    # right place), but nothing `Form`-shaped reaches Python for it.
    app, _state, received = _focused_text_input_app("hi")

    app.push_event(
        {
            "KeyPressed": {
                "key": {"Named": "Left"},
                "modifiers": {},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    assert "KeyPressed" in received[0]
    assert received[0]["KeyPressed"]["key"] == {"Named": "Left"}


def test_text_input_click_places_the_caret_at_the_clicked_column():
    # Clicking a focused `text_input` is no longer focus-only -- the same
    # click that focuses it also resolves a real `(line, col)` via
    # `TextInput::layout`'s own metrics-aware hit-test and seeds the
    # cache with it (`EditOp::SetCursor`), so typing right after a click
    # inserts under the click rather than always at column 0. `click_x =
    # 4.0` resolves to column 2 -- the end of "hi" -- proving the typed
    # "!" lands there ("hi!"), not at the start ("!hi").
    app, state, received = _focused_text_input_app("hi", click_x=4.0)

    app.push_event({"CharTyped": "!"})
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hi!"}}
    assert state["value"] == "hi!"


def test_text_input_shift_left_selects_then_backspace_deletes_the_selection():
    # `Shift+Left` extends a selection from the cached cursor rather than
    # just moving it (`EditOp::from_key`'s `extend` flag is `modifiers.shift`)
    # -- it produces no `Form` event itself (the text is unchanged), but
    # the selection it leaves behind in the cache must still be there for
    # `Backspace` to delete as a unit afterwards, landing on "hel" rather
    # than deleting just the one character before the cursor ("hell").
    app, state, received = _focused_text_input_app("hello", click_x=7.0)

    for _ in range(2):
        app.push_event(
            {
                "KeyPressed": {
                    "key": {"Named": "Left"},
                    "modifiers": {"shift": True},
                    "repeat": False,
                }
            }
        )
    app.run(2)
    assert all("KeyPressed" in e for e in received), (
        "a pure selection extend changes no text, so it falls through as "
        "the raw KeyPressed -- never a Form event"
    )

    received.clear()
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Named": "Backspace"},
                "modifiers": {},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hel"}}
    assert state["value"] == "hel"


def test_text_input_ctrl_a_selects_all_then_typing_replaces_it():
    # `Ctrl+A` on a focused `text_input` resolves to this primitive's own
    # built-in `EditOp::SelectAll`, not a literal "a". `SelectAll` changes
    # no text itself, so -- same as a pure cursor move -- it produces no
    # `Form` event and falls through as the raw `KeyPressed`; what matters
    # is that the selection it leaves behind in the cache makes the next
    # typed character replace the whole buffer instead of being inserted
    # alongside it.
    app, state, received = _focused_text_input_app("hello", click_x=7.0)

    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "a"},
                "modifiers": {"ctrl": True},
                "repeat": False,
            }
        }
    )
    app.run(1)
    assert len(received) == 1
    assert received[0]["KeyPressed"]["key"] == {"Char": "a"}

    received.clear()
    app.push_event({"CharTyped": "X"})
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "X"}}
    assert state["value"] == "X"


def test_text_input_cmd_a_selects_all_on_macos_chord():
    # Regression (#1910 fix-round review): on macOS the SelectAll/Undo/Redo
    # chords arrive with `cmd`, not `ctrl` (`ns_modifier_flags_to_quadraui`
    # maps a real Cmd into `Modifiers { cmd: true, ctrl: false, .. }`).
    # `edit_op_from_binding` compares against `parse_binding`'s hard-coded
    # Ctrl-primary chord, so it must treat Ctrl and Cmd as one interchangeable
    # primary modifier -- otherwise Cmd+A/Z/Shift+Z silently stop working on
    # macOS. Mirror of the `ctrl` SelectAll test above with `cmd` in place of
    # `ctrl`: the SelectAll must still fire, so the next typed char replaces
    # the whole buffer rather than inserting alongside it.
    app, state, received = _focused_text_input_app("hello", click_x=7.0)

    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "a"},
                "modifiers": {"cmd": True},
                "repeat": False,
            }
        }
    )
    app.run(1)
    assert len(received) == 1
    assert received[0]["KeyPressed"]["key"] == {"Char": "a"}

    received.clear()
    app.push_event({"CharTyped": "X"})
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "X"}}
    assert state["value"] == "X"


def test_text_input_ctrl_z_undoes_then_ctrl_shift_z_redoes():
    # `Ctrl+Z`/`Ctrl+Shift+Z` resolve to this primitive's own built-in
    # `EditOp::Undo`/`Redo`, served from the cached `TextEditor`'s own
    # history -- which now survives across calls instead of starting
    # empty every keystroke (see `apply_text_input_edit`'s doc for why a
    # fresh `TextEditor` per call could never undo anything).
    app, state, received = _focused_text_input_app("hi", click_x=4.0)

    app.push_event({"CharTyped": "!"})
    app.run(1)
    assert state["value"] == "hi!"

    received.clear()
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "z"},
                "modifiers": {"ctrl": True},
                "repeat": False,
            }
        }
    )
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hi"}}
    assert state["value"] == "hi"

    received.clear()
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "z"},
                "modifiers": {"ctrl": True, "shift": True},
                "repeat": False,
            }
        }
    )
    app.run(1)
    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputChanged": {"id": "ti:1", "value": "hi!"}}
    assert state["value"] == "hi!"


def test_disabled_switch_click_falls_through_with_hit_attached():
    # `Switch::tab_stop` is unconditionally `true` (disabled means inert,
    # not unfocusable), so the click still focuses it first -- but
    # `Switch::click` returns `None` when `disabled`, so
    # `semantic_event_for_hit` yields no semantic event for the click
    # itself: the raw `MouseDown` (with its `"hit"` dict) reaches Python
    # unchanged instead of being replaced by a `Form` event.
    app = QuiApp(40, 20)
    app.insert_extra("switch", {"id": "sw:1", "disabled": True})

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "sw:1"
    click_event = received[1]
    assert "Form" not in click_event
    assert click_event["MouseDown"]["widget"] == "sw:1"
    assert click_event["hit"] == {"kind": "extra_body"}


def test_clicking_a_switch_inside_a_layout_tree_leaf_uses_the_leaf_id():
    # A render-callback leaf's own tree-address `id` ("leaf:1") and the
    # `Switch` descriptor it wraps' embedded `id` ("sw:embedded") can
    # legitimately differ -- `paint_leaf` keys `PaintedHit::Extra` by the
    # leaf id specifically because a render callback may reuse the same
    # descriptor id across leaves. Both the click-to-focus `FocusChanged`
    # and the `Form`/`ToggleChanged` it emits must therefore agree on
    # "leaf:1", not fall back to the descriptor's own "sw:embedded".
    app = QuiApp(40, 20)

    def render():
        return {
            "row": [
                {"id": "leaf:1", "switch": {"id": "sw:embedded", "value": False}},
            ]
        }

    app.set_render(render)
    # `set_render` itself never repaints -- force the first render so the
    # hit map actually reflects this leaf before the click below.
    app.screen()

    received = _click_events(app, 1.0, 1.0)

    assert len(received) == 2
    assert received[0]["FocusChanged"] == "leaf:1"
    widget_id, form_event = received[1]["Form"]
    assert widget_id == "leaf:1"
    assert form_event == {"ToggleChanged": {"id": "leaf:1", "value": True}}


def test_text_input_enter_commits_current_value():
    # `Enter` on a focused `text_input` always commits -- this primitive has
    # no multiline mode to special-case -- carrying whatever value the
    # current descriptor snapshot holds.
    app = QuiApp(40, 20)
    app.insert_extra("text_input", {"id": "ti:1", "lines": ["hello"]})

    _click_events(app, 1.0, 1.0)

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Named": "Enter"},
                "modifiers": {},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "ti:1"
    assert form_event == {"TextInputCommitted": {"id": "ti:1", "value": "hello"}}


def test_switch_enter_toggles_once_host_echoes_focus_onto_the_descriptor():
    # `Switch::handle_key` gates on the switch's OWN `focused` field, not on
    # the app-level `FocusManager`'s notion of "what has focus" -- so
    # keyboard activation needs the host to have echoed the `FocusChanged`
    # it received back onto the descriptor (`"focused": True`) before the
    # `Enter`/`Space` arrives, same as any other per-frame extra state.
    app = QuiApp(40, 20)
    app.insert_extra("switch", {"id": "sw:1", "focused": True})

    focus_and_click = _click_events(app, 1.0, 1.0)
    assert focus_and_click[0]["FocusChanged"] == "sw:1"

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Named": "Enter"},
                "modifiers": {},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    widget_id, form_event = received[0]["Form"]
    assert widget_id == "sw:1"
    assert form_event == {"ToggleChanged": {"id": "sw:1", "value": True}}


def test_clicking_a_display_only_extra_attaches_no_hit():
    # `progress` paints but never overrides `ExtraPaint::hit_test` or
    # `tab_stop`, so a click lands nowhere and focuses nothing: no
    # `FocusChanged`, no `widget`, no `hit` -- the "no behavior change for
    # display-only extras" half of the interaction seam.
    app = QuiApp(40, 20)
    app.insert_extra(
        "progress", {"id": "progress:download", "label": "Downloading", "value": 0.5}
    )

    event = _click(app, 1.0, 1.0)

    assert event["MouseDown"]["widget"] is None
    assert "hit" not in event


def _locate(app, needle):
    """Scans the plain-text `screen()` dump for `needle` and returns the
    `(row, col)` of its first occurrence -- no bounds/position pymethod is
    exposed to Python, so this is the black-box way to turn a painted
    label into click coordinates (same technique
    `test_layout_containers.py`'s `divider_column` uses)."""
    for row, line in enumerate(app.screen().splitlines()):
        col = line.find(needle)
        if col != -1:
            return row, col
    raise AssertionError(f"{needle!r} not found on screen")


def _popup_items():
    return [
        {"id": "copy", "label": {"spans": [{"text": "Copy", "fg": None, "bg": None}]}},
        {"id": "paste", "label": {"spans": [{"text": "Paste", "fg": None, "bg": None}]}},
    ]


def _dialog_buttons():
    return [
        {"id": "save", "label": "Save", "is_default": True},
        {"id": "cancel", "label": "Cancel", "is_cancel": True},
    ]


def test_show_message_dialog_called_from_python_then_clicking_a_button_round_trips():
    # Exercises `QuiApp.show_message_dialog` through PyO3's own argument
    # conversion (real Python `list`s of `dict`s), not a direct in-process
    # Rust call -- the dialog opens, paints its buttons, and clicking one
    # dispatches `MessageDialogResolved` carrying the clicked button's id,
    # then dismisses.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_message_dialog("Unsaved changes", "Do you want to save?", _dialog_buttons())
    assert app.screen_contains("Save")

    row, col = _locate(app, "Save")
    # Only `col` gets the usual `+ 0.5` nudge: the dialog box is vertically
    # centered in this (even-height) viewport against an (odd-height) box,
    # so every row's continuous-coordinate button rect sits a half-cell
    # above the integer `row` `_locate` reports -- `row` itself (the
    # rect's own top edge) is already inside it, but `row + 0.5` would
    # land exactly on that rect's exclusive bottom edge and miss.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": col + 0.5, "y": row}, "modifiers": {}}}
    )
    app.run(1)

    assert received == [{"MessageDialogResolved": "save"}]
    assert not app.screen_contains("Save")


def test_show_message_dialog_enter_resolves_the_default_button():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_message_dialog("Unsaved changes", "Do you want to save?", _dialog_buttons())
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == [{"MessageDialogResolved": "save"}]


def test_show_message_dialog_escape_resolves_the_cancel_button():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_message_dialog("Unsaved changes", "Do you want to save?", _dialog_buttons())
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Escape"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == [{"MessageDialogResolved": "cancel"}]


def test_show_message_dialog_with_severity_paints_and_rejects_unknown_severity():
    app = QuiApp(40, 20)
    app.show_message_dialog(
        "Unsaved changes", "Do you want to save?", _dialog_buttons(), "warning"
    )
    assert app.screen_contains("Save")

    with pytest.raises(InvalidDescriptorError):
        app.show_message_dialog("Title", "Body", _dialog_buttons(), "critical")


def test_show_context_menu_called_from_python_then_clicking_an_item_round_trips():
    # Exercises `QuiApp.show_context_menu` through PyO3's own argument
    # conversion (a real Python `list` of `dict`s and a `tuple` position),
    # not a direct in-process Rust call -- the popup opens, paints its
    # first item, and clicking it dispatches `ContextMenuItemActivated`
    # carrying the clicked item's id.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_context_menu(_popup_items(), (2.0, 2.0))
    assert app.screen_contains("Copy")

    row, col = _locate(app, "Copy")
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": col + 0.5, "y": row + 0.5}, "modifiers": {}}}
    )
    app.run(1)

    assert received == [{"ContextMenuItemActivated": "copy"}]


def test_show_context_menu_accepts_integer_position():
    # `position` is typed `(f32, f32)`; a Python caller passing a tuple of
    # `int`s (the natural thing to write for a `MouseDown`'s integer-ish
    # cell coordinates) must convert cleanly rather than raising.
    app = QuiApp(40, 20)
    app.show_context_menu(_popup_items(), (2, 2))
    assert app.screen_contains("Copy")


def test_right_click_then_show_context_menu_activation_round_trip():
    # The acceptance path: a raw right-click `MouseDown` reaches the
    # registered handler undisturbed (there is no built-in right-click ->
    # popup wiring), top-level script code sees it and calls
    # `show_context_menu` itself -- per the method's own doc, never from
    # inside the handler -- and the resulting popup's item-click round
    # trips through `ContextMenuItemActivated` exactly like the
    # directly-opened case above.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_event(
        {"MouseDown": {"button": "Right", "position": {"x": 3.0, "y": 3.0}, "modifiers": {}}}
    )
    app.run(1)
    assert received == [
        {
            "MouseDown": {
                "widget": None,
                "button": "Right",
                "position": {"x": 3.0, "y": 3.0},
                "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            }
        }
    ]
    received.clear()

    app.show_context_menu(_popup_items(), (3.0, 3.0))
    row, col = _locate(app, "Copy")
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": col + 0.5, "y": row + 0.5}, "modifiers": {}}}
    )
    app.run(1)

    assert received == [{"ContextMenuItemActivated": "copy"}]


def test_show_context_menu_escape_dismisses_without_item_activation():
    # Escape closes the popup via `ContextMenuDismissed` -- the other half
    # of the method's documented dismissal contract, distinct from an item
    # activation and carrying no payload (pythonizes to the bare variant
    # name, like any other unit `UiEvent`).
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_context_menu(_popup_items(), (2.0, 2.0))
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Escape"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == ["ContextMenuDismissed"]


def test_show_context_menu_with_nested_submenu_opens_by_right_arrow_and_activates():
    # `ContextMenuItem.submenu` nests through the same `show_context_menu`
    # call -- opening the parent's child popup by Right arrow and
    # activating a nested leaf reports that leaf's own id via
    # `ContextMenuItemActivated`, the same shape a flat menu's activation
    # already carries (`nesting depth is transparent to the caller`).
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    items = [
        {
            "id": "view",
            "label": {"spans": [{"text": "View", "fg": None, "bg": None}]},
            "submenu": [
                {
                    "id": "toggle_sidebar",
                    "label": {
                        "spans": [{"text": "Toggle Sidebar", "fg": None, "bg": None}]
                    },
                },
            ],
        },
    ]
    app.show_context_menu(items, (2.0, 2.0))
    assert app.screen_contains("View")

    app.push_event(
        {"KeyPressed": {"key": {"Named": "Right"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)
    assert app.screen_contains("Toggle Sidebar"), "the submenu must now be painted"
    assert received == [], "pure navigation dispatches nothing to the app's handler"

    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == [{"ContextMenuItemActivated": "toggle_sidebar"}]
    assert not app.screen_contains("Toggle Sidebar"), "activation must close the popup"


def test_show_context_menu_item_with_codicon_icon_renders_fallback():
    # `ContextMenuItem::icon` accepts the same `{"codicon": "...",
    # "fallback": "..."}` wire shape a tree row's `icon` does (see
    # `test_descriptors.py::test_insert_tree_with_codicon_icon`), but
    # paints into the single-cell gutter `checked`'s `✓` shares (only
    # the fallback's first character renders) rather than a
    # measured-width slot -- `QuiApp`'s headless driver defaults to nerd
    # fonts off, so that's the fallback's plain-text first character,
    # directly left of the label with no gap.
    app = QuiApp(40, 20)
    items = [
        {
            "id": "new-file",
            "label": {"spans": [{"text": "New File", "fg": None, "bg": None}]},
            "icon": {"codicon": "new-file", "fallback": "NF"},
        },
    ]
    app.show_context_menu(items, (2.0, 2.0))
    assert app.screen_contains("NNew File")


def test_show_context_menu_item_with_unknown_codicon_name_raises():
    app = QuiApp(40, 20)
    items = [
        {
            "id": "new-file",
            "label": {"spans": [{"text": "New File", "fg": None, "bg": None}]},
            "icon": {"codicon": "not-a-real-codicon-name", "fallback": "?"},
        },
    ]
    with pytest.raises(InvalidDescriptorError):
        app.show_context_menu(items, (2.0, 2.0))


def test_show_message_dialog_rejects_empty_buttons():
    # A dialog with no buttons can never be resolved by Enter or Escape,
    # and would swallow every click forever with no dismiss surface.
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.show_message_dialog("Title", "Body", [])


def test_show_message_dialog_rejects_button_tint():
    # `tint` is part of the in-canvas `Dialog.buttons` shape this surface
    # otherwise mirrors, but `show_message_dialog` doesn't honor it yet --
    # a caller setting it must get a loud error, not a silently ignored
    # field.
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.show_message_dialog(
            "Title",
            "Body",
            [
                {
                    "id": "delete",
                    "label": "Delete",
                    "is_default": True,
                    "tint": {"r": 255, "g": 0, "b": 0, "a": 255},
                }
            ],
        )


def test_dismiss_message_dialog_closes_without_resolving():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    assert app.dismiss_message_dialog() is False

    app.show_message_dialog("Title", "Body", _dialog_buttons())
    assert app.screen_contains("Save")

    assert app.dismiss_message_dialog() is True
    assert not app.screen_contains("Save")
    assert received == []


def test_show_message_dialog_closes_an_open_menu_bar_dropdown():
    # Paint order puts the dialog on top of the menu bar's dropdown
    # (`render_dialog` runs after `sync_menu_bar`); `handle` now routes
    # the dialog ahead of the menu bar too, and `show_message_dialog`
    # closes any open dropdown so nothing is left underneath holding
    # input precedence it no longer has paint precedence for.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))
    app.insert_extra(
        "menu_bar",
        {
            "id": "bar:main",
            "items": [
                {
                    "id": "menu:file",
                    "label": "&File",
                    "disabled": False,
                    "submenu": [
                        {
                            "id": "item:open",
                            "label": {"spans": [{"text": "Open", "fg": None, "bg": None}]},
                        }
                    ],
                }
            ],
            "open_item": None,
            "focused_item": None,
        },
    )

    row, col = _locate(app, "File")
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": col + 0.5, "y": row + 0.5}, "modifiers": {}}}
    )
    app.run(1)
    assert app.screen_contains("Open"), "the dropdown must be open before the dialog opens"

    app.show_message_dialog("Heads up", "Something happened.", [
        {"id": "ok", "label": "OK", "is_default": True},
    ])

    assert not app.screen_contains("Open"), "the dialog must close the dropdown underneath it"
    assert app.screen_contains("OK")

    app.push_event(
        {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)

    assert received == [{"MessageDialogResolved": "ok"}]
