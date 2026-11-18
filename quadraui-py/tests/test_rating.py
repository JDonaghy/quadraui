"""Acceptance test for `examples/rating.py` -- the `quadraui.app.widget.Widget`
demo. Drives clicks/keys through `App.push_event`/`run` directly, not
`quadraui.testing.harness`: the harness's `find(role=...)` only recognizes
the fixed built-in kinds (`"button"`, `"list"`, ...), and a `Widget`'s own
wire placeholder paints as a plain `"canvas"` -- the same reason
`test_settings_form_demo.py` stays off the harness for an unsupported
role, and the same low-level `push_event` shape
`quadraui-py/tests/test_extra_primitives.py`'s own `canvas` region-click
tests already use.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_rating.py
"""

import importlib.util
import pathlib

_EXAMPLE_PATH = pathlib.Path(__file__).resolve().parent.parent / "examples" / "rating.py"


def _load_rating():
    spec = importlib.util.spec_from_file_location("rating", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _click(app, x, y):
    app.push_event({"MouseDown": {"button": "Left", "position": {"x": x, "y": y}, "modifiers": {}}})
    app.run(1)


def _key(app, name):
    app.push_event({"KeyPressed": {"key": {"Named": name}, "modifiers": {}, "repeat": False}})
    app.run(1)


def _star_x(rating, index):
    star_width = rating.WIDTH / rating.MAX_STARS
    return star_width * index + star_width / 2


def test_renders_the_initial_rating_as_filled_and_empty_stars():
    rating = _load_rating()
    app, demo = rating.build()

    screen = app.screen()

    assert screen.count("★") == demo.rating
    assert screen.count("☆") == rating.MAX_STARS - demo.rating


def test_clicking_a_star_rates_it_and_calls_on_rate():
    rating = _load_rating()
    app, demo = rating.build()

    _click(app, _star_x(rating, 4), 0.0)

    assert demo.rating == 5
    assert app.screen().count("★") == 5


def test_clicking_a_star_twice_is_idempotent_and_re_renders_from_app_state():
    """The widget itself holds no state across frames -- a repeated click on
    the same star re-sends the same value, and the painted stars always
    reflect `demo.rating`, never a stale value left over on some widget
    instance from an earlier frame (a fresh `RatingControl` is built every
    `view()` call, per its own docstring)."""
    rating = _load_rating()
    app, demo = rating.build()

    _click(app, _star_x(rating, 1), 0.0)
    assert demo.rating == 2
    _click(app, _star_x(rating, 1), 0.0)
    assert demo.rating == 2
    assert app.screen().count("★") == 2


def test_arrow_keys_change_the_rating_while_the_control_holds_focus():
    rating = _load_rating()
    app, demo = rating.build()

    # The control is the app's only tab stop, so a plain `MouseDown`
    # inside it (on a star) focuses it before the click is routed.
    _click(app, _star_x(rating, 2), 0.0)
    assert demo.rating == 3

    _key(app, "Right")
    assert demo.rating == 4

    _key(app, "Left")
    _key(app, "Left")
    assert demo.rating == 2


def test_rating_cannot_go_below_zero_or_above_max_stars():
    rating = _load_rating()
    app, demo = rating.build()
    demo.rating = 0
    _click(app, _star_x(rating, 0), 0.0)
    assert demo.rating == 1

    for _ in range(10):
        _key(app, "Left")
    assert demo.rating == 0

    for _ in range(10):
        _key(app, "Right")
    assert demo.rating == rating.MAX_STARS


def test_a11y_reports_a_slider_role_and_the_current_value():
    """No Python-level a11y query API exists yet for a running `App` (see
    `quadraui.app.widget.Widget.a11y`'s own doc) -- this exercises the
    same method the native extension calls every frame, directly, the way
    a native accessibility backend (macOS/Windows) would read it."""
    rating = _load_rating()
    control = rating.RatingControl(id="rating:stars", value=3)

    node = control.a11y(control.props)

    assert node["info"]["a11y_role"] == "Slider"
    assert node["info"]["value_range"] == {"min": 0.0, "max": 5.0, "current": 3.0}
    assert "3" in node["info"]["value"]
