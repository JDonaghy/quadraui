"""Black-box coverage for the shared binding host's hover-tooltip driving
(`quadraui_binding_core::BindingHost::drive_hover_tooltip`): a `Button`'s
own `tooltip` text shows up as a real `show_tooltip`-equivalent overlay
once the pointer has hovered it continuously for `tooltip_delay_ms`, and
hides again on pointer leave, the next click, or the auto-dismiss timeout
-- with no Python-side wiring beyond
`insert_button`/`set_handler`/`push_event`/`step`, since the *host*
(`BindingHost::drive_hover_tooltip`) drives `HoverTooltipTracker` itself.

`set_test_clock_ms` pins the headless clock `BindingHost::now_ms()` reads
to an absolute value this file picks, so every assertion below is driven
by an exact, deterministic millisecond count rather than a real
`time.sleep` racing against `tooltip_delay_ms`.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_tooltips.py
"""

from quadraui import QuiApp

# Small values are fine -- nothing here sleeps in real time, so the clock
# jumps straight to whatever absolute millisecond each test needs.
DELAY_MS = 20
AUTO_DISMISS_MS = 30


def _locate(app, needle):
    """Scans the plain-text `screen()` dump for `needle` and returns the
    `(row, col)` of its first occurrence -- same technique
    `test_overlay_primitives.py`'s `_locate` uses."""
    for row, line in enumerate(app.screen().splitlines()):
        col = line.find(needle)
        if col != -1:
            return row, col
    raise AssertionError(f"{needle!r} not found on screen")


def _move_to(app, row, col):
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": float(col), "y": float(row)},
                "buttons": {"left": False, "right": False, "middle": False},
            }
        }
    )
    app.step()


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
    app.step()


def _make_app(auto_dismiss_ms=0):
    app = QuiApp(40, 10)
    app.set_handler(lambda event: None)
    app.set_style(
        {"tooltip_delay_ms": DELAY_MS, "tooltip_auto_dismiss_ms": auto_dismiss_ms}
    )
    app.set_test_clock_ms(0)
    app.insert_button({"id": "save", "label": "Save", "tooltip": "Save the file"})
    assert app.screen_contains("Save")
    return app


def test_tooltip_appears_only_after_the_hover_delay():
    app = _make_app()
    row, col = _locate(app, "Save")

    # Landing on the button is not enough by itself -- the delay hasn't
    # elapsed yet, so nothing shows.
    _move_to(app, row, col)
    assert not app.screen_contains("Save the file")

    # One millisecond short of the delay: still nothing.
    app.set_test_clock_ms(DELAY_MS - 1)
    app.step()
    assert not app.screen_contains("Save the file")

    # No further event queued: the next `step()` ticks the app instead of
    # dispatching, which is what notices the delay has elapsed with the
    # pointer held still.
    app.set_test_clock_ms(DELAY_MS)
    app.step()
    assert app.screen_contains("Save the file")


def test_tooltip_hides_on_pointer_leave():
    app = _make_app()
    row, col = _locate(app, "Save")

    _move_to(app, row, col)
    app.set_test_clock_ms(DELAY_MS)
    app.step()
    assert app.screen_contains("Save the file")

    # Moving off the button (to the far bottom-right corner, well clear
    # of a single lone button painted near the top-left) hides it
    # immediately -- no further delay.
    _move_to(app, 9, 39)
    assert not app.screen_contains("Save the file")


def test_tooltip_hides_on_click_then_reappears_if_the_pointer_holds_still():
    app = _make_app()
    row, col = _locate(app, "Save")

    _move_to(app, row, col)
    app.set_test_clock_ms(DELAY_MS)
    app.step()
    assert app.screen_contains("Save the file")

    # A click dismisses it immediately, same as `show_tooltip`'s own
    # explicit-dismiss contract.
    _click(app, row, col)
    assert not app.screen_contains("Save the file")

    # Still hovering the very same button, unmoved: holding still past
    # another full delay re-shows it rather than leaving the tracker
    # stuck believing a tooltip only it still thinks is open.
    app.set_test_clock_ms(2 * DELAY_MS)
    app.step()
    assert app.screen_contains("Save the file")


def test_tooltip_hides_on_auto_dismiss_timeout():
    app = _make_app(auto_dismiss_ms=AUTO_DISMISS_MS)
    row, col = _locate(app, "Save")

    _move_to(app, row, col)
    app.set_test_clock_ms(DELAY_MS)
    app.step()
    assert app.screen_contains("Save the file")

    # One millisecond short of the auto-dismiss deadline: still showing.
    app.set_test_clock_ms(DELAY_MS + AUTO_DISMISS_MS - 1)
    app.step()
    assert app.screen_contains("Save the file")

    # At the deadline, with the pointer never having moved: it auto-hides.
    app.set_test_clock_ms(DELAY_MS + AUTO_DISMISS_MS)
    app.step()
    assert not app.screen_contains("Save the file")


def test_a_button_with_no_tooltip_never_shows_one():
    app = QuiApp(40, 10)
    app.set_handler(lambda event: None)
    app.set_style({"tooltip_delay_ms": DELAY_MS})
    app.set_test_clock_ms(0)
    app.insert_button({"id": "plain", "label": "Plain"})
    row, col = _locate(app, "Plain")

    before = app.screen()

    _move_to(app, row, col)
    app.set_test_clock_ms(DELAY_MS)
    app.step()

    # A widget with no `tooltip` set never arms the tracker at all, so the
    # screen is pixel-for-pixel identical to before the hover -- not just
    # "the button's own label still paints somewhere", which would also
    # pass if a spurious tooltip appeared.
    assert app.screen() == before
