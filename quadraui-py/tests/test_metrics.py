"""Black-box coverage for `QuiApp`'s metrics/capabilities surface:
`viewport`, `line_height`, `char_width`, `list_char_width`,
`chrome_line_height`, `scrollbar_reserve`, `scales_text_rows`,
`snap_height`, `measure_text`, and `backend_caps`. All of these read the
headless `TuiBackend` this `QuiApp` wraps, so the values asserted here are
the fixed-cell-grid truth: a terminal cell is `1.0` wide and `1.0` tall by
construction, no overlay scrollbar chrome, no per-row font scaling.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_metrics.py
"""

import pytest

from quadraui import InvalidFontRoleError, QuiApp


def test_viewport_matches_the_constructor_dimensions():
    app = QuiApp(42, 17)
    assert app.viewport() == (42.0, 17.0, 1.0)


def test_viewport_follows_a_resize_once_rendered():
    # `resize` only resizes the physical buffer; `TuiBackend`'s cached
    # `Viewport` resyncs on the next render (`screen()` here), same as a
    # real terminal resize -- see `TuiDriver::resize`'s doc.
    app = QuiApp(10, 10)
    app.resize(30, 8)
    app.screen()
    assert app.viewport() == (30.0, 8.0, 1.0)


def test_line_height_and_char_width_are_one_terminal_cell():
    app = QuiApp(10, 10)
    assert app.line_height() == 1.0
    assert app.char_width() == 1.0
    assert app.list_char_width() == 1.0
    assert app.chrome_line_height() == 1.0


def test_scrollbar_reserve_and_scales_text_rows_are_tui_defaults():
    app = QuiApp(10, 10)
    assert app.scrollbar_reserve() == 0.0
    assert app.scales_text_rows() is False


def test_snap_height_rounds_to_a_whole_cell():
    app = QuiApp(10, 10)
    assert app.snap_height(2.3) == 2.0
    assert app.snap_height(2.7) == 3.0


def test_measure_text_counts_display_width_in_cells():
    app = QuiApp(10, 10)
    width, height = app.measure_text("hello")
    assert (width, height) == (5.0, 1.0)
    # The default role ("chrome") and the explicit "editor" role measure
    # the same on a terminal, which has one font by definition.
    assert app.measure_text("hello", "editor") == (width, height)


def test_measure_text_rejects_an_unknown_role():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidFontRoleError):
        app.measure_text("hello", "bogus")


def test_backend_caps_reports_the_headless_tui_vocabulary():
    app = QuiApp(10, 10)
    caps = app.backend_caps()
    # Input-delivery capabilities are real on the headless driver.
    assert caps["mouse"] is True
    assert caps["scroll"] is True
    assert caps["drag"] is True
    # No OS chrome / native dialogs on a terminal.
    assert caps["native_menu"] is False
    assert caps["window_chrome"] is False
    assert caps["file_dialogs"] is False
    # The three fields `BackendCaps::vocabulary` excludes are still present.
    assert isinstance(caps["kitty_keyboard"], bool)
    assert isinstance(caps["sgr_pixel_mouse"], bool)
    assert caps["color_depth"] in ("ansi16", "indexed256", "true_color")
