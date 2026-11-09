"""Black-box coverage proving the B3a `Button` fields (`variant`, `icon`,
`icon_position`, `icon_gap`, `tooltip`) and the B3b `ButtonSpec` reach this
binding through `quadraui-binding-core`'s wire decode with **no binding
code of their own** -- both are plain `#[serde(default)]` fields, so a
dict missing them decodes to the same thing `Button::new`/`ButtonSpec::new`
build, and a dict naming them just works.

Three scenarios:

- `insert_button` with a plain dict naming `variant`/`icon`/`tooltip`
  directly, asserting the rendered result (label + icon fallback text on
  screen, and the `Danger` variant's `Theme.error_fg` foreground actually
  painted -- not just accepted without raising).
- `show_toast`'s action buttons, which carry an optional `ButtonSpec`
  override (`ToastButton::spec`) painted by `tui::toast::paint_toast`
  with no stripping in between -- the sibling of `DialogButton::spec`
  that is actually wired end to end, so this is where the B3b vocabulary's
  *colour* can be proven on screen via `cell_colors`.
- `show_message_dialog`'s buttons, which decode as `DialogButton` --
  including its own `spec` field -- through the exact same
  `pythonize::depythonize` call the two scenarios above use. A dialog
  button's `spec` decodes without raising (this is the literal "dialog
  button with a Danger spec" scenario), but `message_dialog_options_from_buttons`
  (`quadraui-binding-core/src/engine.rs`) narrows every `DialogButton`
  down to `quadraui::backend::MessageDialogButton` -- a shape shared with
  real native-OS message boxes (GTK/macOS/Win) that cannot render a
  custom `spec` at all -- before `MessageDialogController::new`
  (`quadraui/src/compose/message_dialog.rs`) ever sees it, so `spec` has
  no visible effect on this particular surface yet. That stripping lives
  in the `quadraui` crate itself (not `quadraui-binding-core`'s decode
  glue) and touches every native backend's message-dialog construction
  site, so wiring it through is out of this issue's scope -- the toast
  scenario above is the one that proves the B3b vocabulary paints.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_button_model.py
"""

from quadraui import QuiApp


def _locate(app, needle):
    """Scans the plain-text `screen()` dump for `needle` and returns the
    `(row, col)` of its first occurrence -- same technique
    `test_extra_primitives.py`'s `_locate` uses."""
    for row, line in enumerate(app.screen().splitlines()):
        col = line.find(needle)
        if col != -1:
            return row, col
    raise AssertionError(f"{needle!r} not found on screen")


def _rgb(color_dict):
    return (color_dict["r"], color_dict["g"], color_dict["b"])


def test_insert_button_decodes_variant_icon_and_tooltip_from_a_plain_dict():
    app = QuiApp(40, 20)
    app.insert_button(
        {
            "id": "demo:danger",
            "label": "Delete",
            "variant": "Danger",
            "icon": {"glyph": "", "fallback": "!", "color": None},
            "tooltip": "This cannot be undone",
        }
    )

    # The label and the icon's plain-text fallback both paint -- the
    # headless backend defaults nerd fonts off, so `Icon::resolve` picks
    # `fallback` over `glyph` (see `tui::button`'s own module doc).
    assert app.screen_contains("Delete")
    assert app.screen_contains("!")

    # `variant: Danger` colours the label text in `Theme.error_fg` on
    # TUI -- there is no dedicated `danger_fg` field, the label
    # foreground *is* the intent colour (see `tui::button`'s module
    # doc). `tooltip` only shows once the pointer hovers it for
    # `tooltip_delay_ms` (see `test_tooltips.py`), so there is nothing
    # further to assert on it from this one-shot decode-and-paint check.
    row, col = _locate(app, "Delete")
    label_fg, _ = app.cell_colors(col, row)
    assert label_fg == _rgb(app.theme()["error_fg"])


def test_insert_button_without_variant_keeps_the_default_primary_look():
    # Same dict shape, minus `variant`/`icon`/`tooltip` entirely -- proves
    # the `Danger` colouring above comes from the field actually being
    # set, not from some unrelated default already matching `error_fg`.
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:plain", "label": "Delete"})

    row, col = _locate(app, "Delete")
    label_fg, _ = app.cell_colors(col, row)
    assert label_fg == _rgb(app.theme()["foreground"])
    assert label_fg != _rgb(app.theme()["error_fg"])


def test_show_toast_danger_spec_action_colours_its_row_in_error_fg():
    # `Toast.actions` decodes straight to `Vec<ToastButton>`, and
    # `ToastButton::spec` -- the same `ButtonSpec` `Button::icon`'s own
    # sibling fields live on -- is honoured by `tui::toast::paint_toast`
    # with no stripping in between, unlike `DialogButton::spec` (see this
    # module's own doc). A `Danger` spec recolours the whole action row's
    # foreground to `Theme.error_fg` and drops any `accent_bg`/`link_fg`
    # look the legacy `primary` flag would otherwise give it.
    app = QuiApp(40, 20)
    app.show_toast(
        {
            "id": "toast:confirm",
            "title": "Delete file?",
            "body": "",
            "actions": [
                {"id": "keep", "label": "Keep"},
                {
                    "id": "discard",
                    "label": "Discard",
                    "spec": {"label": "Discard", "variant": "Danger"},
                },
            ],
        }
    )

    assert app.screen_contains("Keep")
    assert app.screen_contains("Discard")

    keep_row, keep_col = _locate(app, "Keep")
    discard_row, discard_col = _locate(app, "Discard")

    keep_fg, _ = app.cell_colors(keep_col, keep_row)
    discard_fg, _ = app.cell_colors(discard_col, discard_row)

    assert discard_fg == _rgb(app.theme()["error_fg"])
    assert keep_fg != discard_fg


def test_show_message_dialog_accepts_a_dialog_button_with_a_danger_spec():
    # `DialogButton::spec` decodes fine through the same
    # `pythonize::depythonize` call the other two scenarios use -- a
    # dialog button naming `spec` never raises `InvalidDescriptorError`.
    # See this module's own doc for why `spec` has no visible rendering
    # effect on *this* surface yet (`message_dialog_options_from_buttons`
    # narrows every button down to `quadraui::backend::MessageDialogButton`
    # before painting, the same native-OS-compatible shape that already
    # rejects `tint` for the identical reason).
    app = QuiApp(60, 20)
    app.show_message_dialog(
        "Unsaved changes",
        "Delete the file permanently?",
        [
            {
                "id": "discard",
                "label": "Delete",
                "is_default": True,
                "spec": {"label": "Delete", "variant": "Danger"},
            },
            {"id": "cancel", "label": "Cancel", "is_cancel": True},
        ],
    )

    assert app.screen_contains("Delete")
    assert app.screen_contains("Cancel")
