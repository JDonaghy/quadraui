"""Black-box coverage for the remaining in-canvas modal-overlay primitives
wired onto `InteractionEngine`'s own anchored-overlay slot (the one
`show_message_dialog` / `show_context_menu` already use):
`show_toast`/`dismiss_toast`, `show_tooltip`/`hide_tooltip`,
`show_completions`/`dismiss_completions`, `show_float`/`dismiss_float`, and
`show_rich_text_popup`/`dismiss_rich_text_popup` -- plus the generic
`show_overlay`/`dismiss_overlay` pair every one of those 13 methods is a
one-line forward onto.

Follows `test_extra_primitives.py`'s `show_message_dialog`/
`show_context_menu` tests as the pattern: a real Python `dict` decoded
straight into the matching `quadraui` primitive, painted immediately, and
routed through `push_event`/`run` exactly like any other input.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_overlay_primitives.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def _locate(app, needle):
    """Scans the plain-text `screen()` dump for `needle` and returns the
    `(row, col)` of its first occurrence -- same technique
    `test_extra_primitives.py`'s `_locate` uses."""
    for row, line in enumerate(app.screen().splitlines()):
        col = line.find(needle)
        if col != -1:
            return row, col
    raise AssertionError(f"{needle!r} not found on screen")


def _click(app, row, col):
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": float(col), "y": float(row)},
                "modifiers": {},
            }
        }
    )
    app.run(1)


def _escape(app):
    app.push_event(
        {"KeyPressed": {"key": {"Named": "Escape"}, "modifiers": {}, "repeat": False}}
    )
    app.run(1)


# ── Toast ────────────────────────────────────────────────────────────────


def test_show_toast_paints_over_base_content_and_action_click_routes():
    # The acceptance scenario: a toast painted via `show_toast` shows up
    # over whatever the app already painted (z-order/stacking above the
    # base layer), and clicking its action button dispatches
    # `ToastActionClicked` carrying that action's own id.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})
    assert app.screen_contains("OK")

    app.show_toast(
        {
            "id": "toast:saved",
            "title": "Saved",
            "body": "",
            "actions": [{"id": "undo", "label": "Undo"}],
        }
    )
    # Both the base content and the overlay painted on top of it are
    # visible at once -- the overlay never replaces the base layer.
    assert app.screen_contains("OK")
    assert app.screen_contains("Saved")
    assert app.screen_contains("Undo")

    row, col = _locate(app, "Undo")
    _click(app, row, col)

    assert received == [{"ToastActionClicked": "undo"}]
    # The action click doesn't itself close the toast -- the app's own
    # action handler decides whether to call `dismiss_toast`.
    assert app.screen_contains("Saved")


def test_toast_dismiss_click_closes_with_no_event():
    # The dismiss `x` affordance closes the toast directly -- no event to
    # route, unlike an action click.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_toast({"id": "toast:saved", "title": "Saved", "body": ""})
    row, col = _locate(app, "×")

    _click(app, row, col)

    assert received == []
    assert not app.screen_contains("Saved")


def test_dismiss_toast_by_id_closes_without_input():
    app = QuiApp(40, 20)
    app.show_toast({"id": "toast:a", "title": "A", "body": ""})
    app.show_toast({"id": "toast:b", "title": "B", "body": ""})
    assert app.screen_contains("A")
    assert app.screen_contains("B")

    assert app.dismiss_toast("toast:a") is True
    assert not app.screen_contains("A")
    assert app.screen_contains("B")

    # Dismissing an id that isn't showing is a no-op, not an error.
    assert app.dismiss_toast("toast:a") is False


def test_show_toast_rejects_malformed_descriptor():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.show_toast({"id": "toast:bad"})  # missing required `title`


# ── Tooltip ──────────────────────────────────────────────────────────────


def test_show_tooltip_paints_over_base_content_then_auto_dismisses_on_click():
    # The acceptance scenario's other half: a tooltip shows up over the
    # base layer, and -- since it's non-interactive, per
    # `Backend::draw_tooltip`'s own doc -- the next click anywhere
    # dismisses it (the dismiss routes, even with no payload to report).
    app = QuiApp(40, 20)
    app.set_handler(lambda event: None)

    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})
    app.show_tooltip({"id": "tip:hint", "text": "Hello tip"}, (0.0, 2.0, 10.0, 1.0))

    assert app.screen_contains("OK")
    assert app.screen_contains("Hello tip")

    # A click elsewhere on the screen (not on the tooltip itself) still
    # dismisses it -- the auto-dismiss is unconditional on the next
    # click, not scoped to a click landing on the tooltip's own bounds.
    _click(app, 0, 0)

    assert not app.screen_contains("Hello tip")


def test_hide_tooltip_closes_explicitly():
    app = QuiApp(40, 20)
    app.show_tooltip({"id": "tip:hint", "text": "Hello tip"}, (0.0, 2.0, 10.0, 1.0))
    assert app.screen_contains("Hello tip")

    assert app.hide_tooltip() is True
    assert not app.screen_contains("Hello tip")
    assert app.hide_tooltip() is False


# ── Completions ──────────────────────────────────────────────────────────


def _completion_items():
    return [
        {"label": {"spans": [{"text": "map", "fg": None, "bg": None}]}},
        {"label": {"spans": [{"text": "filter", "fg": None, "bg": None}]}},
    ]


def test_show_completions_then_clicking_an_item_activates_it():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_completions(
        {"id": "completions:editor", "selected_idx": 0, "items": _completion_items()},
        (2.0, 2.0),
    )
    assert app.screen_contains("map")
    assert app.screen_contains("filter")

    row, col = _locate(app, "filter")
    _click(app, row, col)

    assert received == [{"CompletionItemActivated": ("completions:editor", 1)}]
    assert not app.screen_contains("filter")


def test_completions_escape_dismisses_without_activation():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_completions(
        {"id": "completions:editor", "selected_idx": 0, "items": _completion_items()},
        (2.0, 2.0),
    )
    _escape(app)

    assert received == []
    assert not app.screen_contains("map")


def test_dismiss_completions_closes_without_input():
    app = QuiApp(40, 20)
    app.show_completions(
        {"id": "completions:editor", "selected_idx": 0, "items": _completion_items()},
        (2.0, 2.0),
    )
    assert app.dismiss_completions() is True
    assert not app.screen_contains("map")
    assert app.dismiss_completions() is False


# ── Float ────────────────────────────────────────────────────────────────


def _anchor(x, y, width=1.0, height=1.0, side="Bottom"):
    return {
        "rect": {"x": x, "y": y, "width": width, "height": height},
        "preferred": side,
        "margin": 0.0,
    }


def test_show_float_paints_chrome_then_escape_dismisses():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})
    app.show_float({"id": "float:hints", "anchor": _anchor(2.0, 2.0)}, 10.0, 4.0)

    # Chrome-only -- no app content of its own, but the border it paints
    # shows up on top of the base layer.
    assert app.screen_contains("OK")
    assert app.screen_contains("┌")

    _escape(app)

    assert received == []
    assert not app.screen_contains("┌")


def test_dismiss_float_closes_without_input():
    app = QuiApp(40, 20)
    app.show_float({"id": "float:hints", "anchor": _anchor(2.0, 2.0)}, 10.0, 4.0)
    assert app.dismiss_float() is True
    assert app.dismiss_float() is False


def test_non_focusable_float_does_not_claim_escape():
    # `focusable: False` is the "transient hint popup" case
    # (`quadraui.Float`'s own doc) -- it must not steal Escape from
    # whatever else would otherwise handle it.
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    float_descriptor = {"id": "float:hint", "anchor": _anchor(2.0, 2.0), "focusable": False}
    app.show_float(float_descriptor, 10.0, 4.0)

    _escape(app)

    # Escape fell through to the registered handler as a plain
    # `KeyPressed` instead of being swallowed as a dismissal.
    assert any("KeyPressed" in event for event in received)
    assert app.screen_contains("┌")


# ── RichTextPopup ────────────────────────────────────────────────────────


def test_show_rich_text_popup_then_clicking_a_link_routes():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_rich_text_popup(
        {
            "id": "popup:hover",
            "lines": [{"spans": [{"text": "See docs", "fg": None, "bg": None}]}],
            "line_text": ["See docs"],
            "max_visible_rows": 3,
            "links": [
                {"line": 0, "start_byte": 4, "end_byte": 8, "url": "https://example.com"}
            ],
        },
        (2.0, 2.0),
    )
    assert app.screen_contains("See docs")

    row, col = _locate(app, "docs")
    _click(app, row, col)

    assert received == [{"RichTextLinkClicked": ("popup:hover", "https://example.com")}]


def test_rich_text_popup_escape_dismisses():
    app = QuiApp(40, 20)
    app.set_handler(lambda event: None)
    app.show_rich_text_popup(
        {
            "id": "popup:hover",
            "lines": [{"spans": [{"text": "Hello", "fg": None, "bg": None}]}],
            "line_text": ["Hello"],
            "max_visible_rows": 3,
        },
        (2.0, 2.0),
    )
    assert app.screen_contains("Hello")

    _escape(app)

    assert not app.screen_contains("Hello")


def test_dismiss_rich_text_popup_closes_without_input():
    app = QuiApp(40, 20)
    app.show_rich_text_popup(
        {
            "id": "popup:hover",
            "lines": [{"spans": [{"text": "Hello", "fg": None, "bg": None}]}],
            "line_text": ["Hello"],
            "max_visible_rows": 3,
        },
        (2.0, 2.0),
    )
    assert app.dismiss_rich_text_popup() is True
    assert app.dismiss_rich_text_popup() is False


# ── Generic show_overlay/dismiss_overlay ──────────────────────────────────
#
# All 13 kind-specific methods above are one-line forwards onto these two;
# this section exercises the generic pair directly rather than re-covering
# every behavior already pinned above.


def test_show_overlay_toast_returns_its_own_id_and_dismiss_overlay_closes_it():
    app = QuiApp(40, 20)
    returned_id = app.show_overlay(
        "toast", {"id": "toast:saved", "title": "Saved", "body": ""}, None
    )
    assert returned_id == "toast:saved"
    assert app.screen_contains("Saved")

    assert app.dismiss_overlay(returned_id) is True
    assert not app.screen_contains("Saved")
    # Dismissing the same id again is a no-op, not an error.
    assert app.dismiss_overlay(returned_id) is False


def test_show_overlay_message_dialog_returns_sentinel_and_dismiss_overlay_closes_it():
    app = QuiApp(40, 20)
    returned_id = app.show_overlay(
        "message_dialog",
        {"title": "Quit?", "body": "", "buttons": [{"id": "ok", "label": "OK"}]},
        None,
    )
    assert returned_id == "message_dialog"
    assert app.screen_contains("Quit?")

    assert app.dismiss_overlay("message_dialog") is True
    assert not app.screen_contains("Quit?")


def test_show_overlay_context_menu_then_dismiss_overlay_closes_it():
    # `dismiss_overlay("context_menu")` is a new capability `dismiss_*`
    # never had: there was no `dismiss_context_menu` among the original 13.
    app = QuiApp(40, 20)
    returned_id = app.show_overlay(
        "context_menu",
        {
            "items": [
                {"id": "copy", "label": {"spans": [{"text": "Copy", "fg": None, "bg": None}]}}
            ]
        },
        (2.0, 2.0, 0.0, 0.0),
    )
    assert returned_id == "context_menu"
    assert app.screen_contains("Copy")

    assert app.dismiss_overlay("context_menu") is True
    assert not app.screen_contains("Copy")
    assert app.dismiss_overlay("context_menu") is False


def test_show_overlay_tooltip_matches_show_tooltip_screen_output():
    # `show_tooltip` is itself a one-line forward onto `show_overlay`; this
    # pins that the generic entry point paints identically to the
    # kind-specific one it replaces.
    direct = QuiApp(40, 20)
    direct.show_tooltip({"id": "tip:hint", "text": "Hello tip"}, (0.0, 2.0, 10.0, 1.0))

    generic = QuiApp(40, 20)
    returned_id = generic.show_overlay(
        "tooltip", {"id": "tip:hint", "text": "Hello tip"}, (0.0, 2.0, 10.0, 1.0)
    )

    assert returned_id == "tip:hint"
    assert generic.screen() == direct.screen()


def test_show_overlay_rejects_unknown_kind():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.show_overlay("not_a_real_kind", {}, None)


def test_show_overlay_rejects_missing_anchor_when_required():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.show_overlay("tooltip", {"id": "tip:hint", "text": "Hello tip"}, None)


def test_dismiss_overlay_unknown_id_is_a_no_op():
    app = QuiApp(40, 20)
    assert app.dismiss_overlay("not-a-known-overlay-id") is False


# ── Content-bearing overlays (popover / layout dialog) ───────────────────


def _toggle_form_leaf(leaf_id="settings", field_id="wrap"):
    """A `"form"` leaf holding one toggle field -- the same shape
    `test_extra_primitives.py`'s own `"form"` case uses, reused here as an
    overlay's content."""
    return {
        "id": leaf_id,
        "form": {
            "id": leaf_id,
            "fields": [
                {
                    "id": field_id,
                    "label": {"spans": [{"text": "Word wrap", "fg": None, "bg": None}]},
                    "kind": {"Toggle": {"value": False}},
                }
            ],
        },
    }


def _popover_descriptor():
    """A popover whose content is a `Button` above a `"form"` leaf.

    The button comes first deliberately: opening a content-bearing
    overlay seeds keyboard focus onto its first tab stop, and the TUI
    focus-ring convention paints a border box over the focused widget's
    own edge rows (`quadraui/src/tui/focus_ring.rs`). A single-row form
    field as the *first* stop would have its only row covered by that
    ring, which would make the assertions below read as "the content
    never painted" when it did.
    """
    return {
        "float": {"id": "popover:menu", "anchor": _anchor(2.0, 2.0)},
        "content": {
            "column": [
                {"id": "submit", "button": {"id": "submit", "label": "Submit"}},
                _toggle_form_leaf(),
            ]
        },
    }


def test_show_overlay_popover_paints_its_content_and_routes_a_button_click():
    app = QuiApp(60, 24)
    received = []
    app.set_handler(lambda event: received.append(event))
    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})

    returned_id = app.show_overlay("popover", _popover_descriptor(), (24.0, 8.0, 0.0, 0.0))

    assert returned_id == "popover:menu"
    # Unlike a chrome-only `"float"`, the popover's own content paints.
    assert app.screen_contains("Submit")
    assert app.screen_contains("Word wrap")

    row, col = _locate(app, "Submit")
    _click(app, row, col)

    assert any("Form" in event for event in received), received


def test_show_overlay_popover_toggle_emits_the_normal_semantic_event():
    app = QuiApp(60, 24)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_overlay("popover", _popover_descriptor(), (24.0, 8.0, 0.0, 0.0))
    row, col = _locate(app, "Word wrap")
    _click(app, row, col)

    toggles = [
        event
        for event in received
        if isinstance(event, dict)
        and "Form" in event
        and "ToggleChanged" in event["Form"][1]
    ]
    assert toggles, received
    assert toggles[0]["Form"][1]["ToggleChanged"]["value"] is True


def test_show_overlay_popover_is_dismissed_by_an_outside_click():
    app = QuiApp(60, 24)
    # A registered handler is what routes events through the engine at
    # all (`PyAppLogic::handle` skips the whole engine without one), so
    # the dismissal below needs one even though it asserts on the screen.
    app.set_handler(lambda event: None)
    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})
    app.show_overlay("popover", _popover_descriptor(), (24.0, 8.0, 0.0, 0.0))
    assert app.screen_contains("Submit")

    # The viewport's own bottom-right corner, well clear of a popover
    # anchored at (2, 2).
    _click(app, 23, 59)

    assert not app.screen_contains("Submit")


def _layout_dialog_descriptor():
    return {
        "dialog": {
            "id": "dialog:prefs",
            "title": "Preferences",
            "buttons": [
                {"id": "save", "label": "Save", "is_default": True},
                {"id": "cancel", "label": "Cancel", "is_cancel": True},
            ],
        },
        # Same first-stop reasoning as `_popover_descriptor`: the button
        # takes the seeded focus (and its ring), leaving the form's own
        # single row painted in full.
        "content": {
            "column": [
                {"id": "apply", "button": {"id": "apply", "label": "Apply"}},
                _toggle_form_leaf(),
            ]
        },
    }


def test_show_overlay_layout_dialog_paints_chrome_plus_content():
    app = QuiApp(60, 24)
    app.set_render(lambda: {"button": {"id": "ok", "label": "OK"}})

    returned_id = app.show_overlay("layout_dialog", _layout_dialog_descriptor(), (40.0, 6.0, 0.0, 0.0))

    assert returned_id == "dialog:prefs"
    assert app.screen_contains("Preferences")
    assert app.screen_contains("Cancel")
    assert app.screen_contains("Word wrap")


def test_show_overlay_layout_dialog_toggle_emits_the_normal_semantic_event():
    app = QuiApp(60, 24)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_overlay("layout_dialog", _layout_dialog_descriptor(), (40.0, 6.0, 0.0, 0.0))
    row, col = _locate(app, "Word wrap")
    _click(app, row, col)

    toggles = [
        event
        for event in received
        if isinstance(event, dict)
        and "Form" in event
        and "ToggleChanged" in event["Form"][1]
    ]
    assert toggles, received


def test_show_overlay_layout_dialog_escape_resolves_its_cancel_button():
    app = QuiApp(60, 24)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.show_overlay("layout_dialog", _layout_dialog_descriptor(), (40.0, 6.0, 0.0, 0.0))
    _escape(app)

    resolved = [
        event
        for event in received
        if isinstance(event, dict) and "MessageDialogResolved" in event
    ]
    assert resolved, received
    assert resolved[0]["MessageDialogResolved"] == "cancel"
    assert not app.screen_contains("Preferences")


def test_dismiss_overlay_closes_a_popover_and_a_layout_dialog():
    app = QuiApp(60, 24)
    app.show_overlay("popover", _popover_descriptor(), (24.0, 8.0, 0.0, 0.0))
    assert app.dismiss_overlay("popover") is True
    assert app.dismiss_overlay("popover") is False

    app.show_overlay("layout_dialog", _layout_dialog_descriptor(), (40.0, 6.0, 0.0, 0.0))
    assert app.dismiss_overlay("layout_dialog") is True
    assert app.dismiss_overlay("layout_dialog") is False


def test_show_overlay_popover_rejects_a_descriptor_missing_its_content():
    app = QuiApp(60, 24)
    with pytest.raises(InvalidDescriptorError):
        app.show_overlay(
            "popover", {"float": {"id": "popover:menu", "anchor": _anchor(2.0, 2.0)}}, (24.0, 8.0, 0.0, 0.0)
        )
