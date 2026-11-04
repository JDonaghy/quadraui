"""Black-box coverage for `QuiApp`'s theme/style/font configuration surface:
`theme`/`set_theme`, `style`/`set_style`, `set_nerd_fonts`/
`nerd_fonts_enabled`, the font setters (`set_editor_font`, `set_ui_font`,
`register_font_from_memory`, `set_nerd_font_fallback`, `has_font_family`),
and `set_caret_shape`.

`cell_colors` (RGB readback of one headless-backend cell) is how these
tests prove `set_theme`/`set_style` actually change what gets *painted*,
not just what `theme()`/`style()` read back -- an empty `status_bar` fills
its whole row with `Theme.background` (see `status_bar.rs`'s own doc), so
inserting one with no segments gives a one-cell probe for the background
colour with no other setup needed.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_theme.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp


def _insert_empty_status_bar(app: QuiApp) -> None:
    app.insert_extra(
        "status_bar",
        {
            "id": "status_bar:main",
            "left_segments": [],
            "right_segments": [],
        },
    )


def test_set_theme_changes_the_painted_background_colour():
    app = QuiApp(20, 5)
    _insert_empty_status_bar(app)

    # An empty `StatusBar` fills its row with `Theme::default()`'s
    # background (`20, 22, 30` from `theme.rs`) for both halves of the
    # cell -- the fallback fill paints a blank glyph, so fg and bg are the
    # same colour (see `draw_status_bar`'s own fallback-fill code).
    assert app.cell_colors(0, 0) == ((20, 22, 30), (20, 22, 30))

    app.set_theme({"background": "#112233"})

    assert app.cell_colors(0, 0) == ((17, 34, 51), (17, 34, 51))


def test_set_theme_only_touches_the_named_fields():
    app = QuiApp(20, 5)
    before = app.theme()

    app.set_theme({"background": "#112233"})

    after = app.theme()
    assert after["background"] == {"r": 17, "g": 34, "b": 51, "a": 255}
    # Every other field is untouched.
    assert after["foreground"] == before["foreground"]
    assert after["accent_fg"] == before["accent_fg"]


def test_set_theme_accepts_an_rgb_dict_and_a_list():
    app = QuiApp(20, 5)

    app.set_theme({"foreground": {"r": 1, "g": 2, "b": 3, "a": 255}})
    assert app.theme()["foreground"] == {"r": 1, "g": 2, "b": 3, "a": 255}

    app.set_theme({"foreground": [9, 8, 7, 255]})
    assert app.theme()["foreground"] == {"r": 9, "g": 8, "b": 7, "a": 255}


def test_set_theme_rejects_an_unknown_field():
    app = QuiApp(20, 5)
    with pytest.raises(InvalidDescriptorError):
        app.set_theme({"not_a_real_field": "#000000"})


def test_set_theme_rejects_a_malformed_hex_colour():
    app = QuiApp(20, 5)
    with pytest.raises(InvalidDescriptorError):
        app.set_theme({"background": "not-a-colour"})


def test_set_style_overrides_a_single_token_and_round_trips():
    app = QuiApp(20, 5)
    default_padding = app.style()["padding"]

    app.set_style({"focus_ring_width": 9.5})

    style = app.style()
    assert style["focus_ring_width"] == 9.5
    # Unrelated token is untouched.
    assert style["padding"] == default_padding


def test_set_style_rejects_an_unknown_field():
    app = QuiApp(20, 5)
    with pytest.raises(InvalidDescriptorError):
        app.set_style({"not_a_real_token": 1.0})


def test_set_style_overrides_the_chrome_metric_tokens_and_round_trips():
    """`Style`'s five chrome-metric tokens (`tab_height`, `status_bar_height`,
    `activity_bar_width`, `row_height`, `toolbar_height`) reach `QuiApp`
    through the same `#[serde(default)]`-decoded `set_style` dict every other
    `Style` field already does -- no binding-side change needed to carry a
    new descriptor field, matching `quadraui-binding-core`'s wire decode.
    """
    app = QuiApp(20, 5)
    before = app.style()
    assert before["tab_height"] == 35.0
    assert before["status_bar_height"] == 22.0
    assert before["activity_bar_width"] == 48.0
    assert before["row_height"] == 22.0
    assert before["toolbar_height"] == 35.0

    app.set_style(
        {
            "tab_height": 50.0,
            "status_bar_height": 40.0,
            "activity_bar_width": 96.0,
            "row_height": 30.0,
            "toolbar_height": 50.0,
        }
    )

    after = app.style()
    assert after["tab_height"] == 50.0
    assert after["status_bar_height"] == 40.0
    assert after["activity_bar_width"] == 96.0
    assert after["row_height"] == 30.0
    assert after["toolbar_height"] == 50.0
    # Unrelated token is untouched.
    assert after["padding"] == before["padding"]


def test_set_style_overrides_the_scrollbar_width_token_and_round_trips():
    """`Style::scrollbar_width` reaches `QuiApp` through the same
    `#[serde(default)]`-decoded `set_style` dict every other `Style` field
    already does -- no binding-side change needed to carry a new
    descriptor field, matching `quadraui-binding-core`'s wire decode.
    """
    app = QuiApp(20, 5)
    before = app.style()
    assert before["scrollbar_width"] == 10.0

    app.set_style({"scrollbar_width": 14.0})

    after = app.style()
    assert after["scrollbar_width"] == 14.0
    # Unrelated token is untouched.
    assert after["padding"] == before["padding"]


def test_nerd_fonts_round_trips_through_set_and_get():
    app = QuiApp(20, 5)
    # TUI starts disabled -- see `Backend::set_nerd_fonts`'s own doc.
    assert app.nerd_fonts_enabled() is False

    app.set_nerd_fonts(True)
    assert app.nerd_fonts_enabled() is True

    app.set_nerd_fonts(False)
    assert app.nerd_fonts_enabled() is False


def test_font_setters_are_a_no_op_but_do_not_raise_on_tui():
    app = QuiApp(20, 5)
    # Fixed-cell TUI has no font concept -- see each setter's own doc on
    # `Backend`. These must not raise even though they paint nothing.
    app.set_editor_font("Monospace", 12.0)
    app.set_ui_font("Sans 11")
    app.set_nerd_font_fallback("Nerd Font Fallback")


def test_register_font_from_memory_returns_none_on_tui():
    app = QuiApp(20, 5)
    assert app.register_font_from_memory(b"not a real font") is None


def test_has_font_family_returns_none_on_tui():
    app = QuiApp(20, 5)
    # TUI can't answer this question -- `None` means "don't know", never
    # "false" (see `Backend::has_font_family`'s own doc).
    assert app.has_font_family("Monospace") is None


def test_set_caret_shape_accepts_every_documented_shape():
    app = QuiApp(20, 5)
    app.set_caret_shape("block")
    app.set_caret_shape("bar")
    app.set_caret_shape("underline")


def test_set_caret_shape_rejects_an_unknown_shape():
    app = QuiApp(20, 5)
    with pytest.raises(InvalidDescriptorError):
        app.set_caret_shape("not-a-shape")


def test_app_can_react_to_a_system_theme_changed_event():
    # `UiEvent::SystemThemeChanged` already arrives through the generic
    # `push_event`/handler path. A handler cannot call back into this same
    # `QuiApp` while it's running (it's already borrowed for the duration
    # of `step`/`run` -- the same restriction `render_callback`'s own doc
    # notes for `insert_*`), so a real app reacts by recording the event
    # and calling `set_theme` afterwards, once `step`/`run` has returned --
    # exactly what this test does.
    received = []
    app = QuiApp(20, 5, on_event=lambda event: received.append(event))
    _insert_empty_status_bar(app)

    app.push_event(
        {"SystemThemeChanged": {"dark": True, "accent": None, "high_contrast": False}}
    )
    app.step()

    assert "SystemThemeChanged" in received[0]
    dark = received[0]["SystemThemeChanged"]["dark"]
    app.set_theme({"background": "#000000" if dark else "#ffffff"})

    assert app.cell_colors(0, 0)[1] == (0, 0, 0)


# ─── `QuiAppActions.set_theme` (deferred counterpart) ───────────────────


def test_actions_set_theme_queued_from_a_handler_does_not_raise_already_borrowed():
    # A handler reacting to `SystemThemeChanged` holds this app's one
    # mutable borrow for the whole dispatch that invoked it -- a direct
    # `app.set_theme(...)` call from inside it would hit PyO3's "already
    # borrowed" wall instead. `app.actions().set_theme(...)` queues the
    # repaint instead, applied by `flush_deferred_actions` right after this
    # dispatch finishes.
    app = QuiApp(20, 5)
    _insert_empty_status_bar(app)
    actions = app.actions()

    def on_event(event):
        if "SystemThemeChanged" in event:
            dark = event["SystemThemeChanged"]["dark"]
            actions.set_theme({"background": "#000000" if dark else "#ffffff"})

    app.set_handler(on_event)
    app.push_event(
        {"SystemThemeChanged": {"dark": True, "accent": None, "high_contrast": False}}
    )
    app.step()

    assert app.cell_colors(0, 0)[1] == (0, 0, 0)


def test_actions_set_theme_only_touches_the_named_fields():
    app = QuiApp(20, 5)
    before = app.theme()
    actions = app.actions()

    def on_event(_event):
        actions.set_theme({"background": "#112233"})

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    after = app.theme()
    assert after["background"] == {"r": 17, "g": 34, "b": 51, "a": 255}
    assert after["foreground"] == before["foreground"]


def test_actions_set_theme_rejects_an_unknown_field():
    app = QuiApp(20, 5)
    actions = app.actions()
    with pytest.raises(InvalidDescriptorError):
        actions.set_theme({"not_a_real_field": "#000000"})


def test_actions_set_theme_rejects_a_malformed_hex_colour():
    app = QuiApp(20, 5)
    actions = app.actions()
    with pytest.raises(InvalidDescriptorError):
        actions.set_theme({"background": "not-a-colour"})
