"""Black-box coverage for pointer hover reaching a painted widget through
the binding: a `MouseMoved` pushed into `QuiApp` feeds the ambient pointer
state, and the next frame paints the hovered part with its hover tint.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_hover_channel.py
"""

from quadraui import QuiApp


def _locate(app, needle):
    for row, line in enumerate(app.screen().splitlines()):
        col = line.find(needle)
        if col != -1:
            return row, col
    raise AssertionError(f"{needle!r} not found on screen")


def _tab_bar_app():
    app = QuiApp(40, 20)
    app.set_render(
        lambda: {
            "tab_bar": {
                "id": "tabs:hover",
                "tabs": [
                    {"label": "alpha", "is_active": True},
                    {"label": "beta"},
                ],
            }
        }
    )
    return app


def _move(app, col, row):
    app.push_event(
        {"MouseMoved": {"position": {"x": float(col), "y": float(row)}, "buttons": {}}}
    )
    app.run(1)


def test_moving_over_an_inactive_tab_tints_it_and_moving_off_restores_it():
    app = _tab_bar_app()
    row, col = _locate(app, "beta")
    _, resting_bg = app.cell_colors(col, row)

    _move(app, col, row)
    _, hovered_bg = app.cell_colors(col, row)
    assert hovered_bg != resting_bg

    # Row 10 is below the bar, on no painted widget.
    _move(app, col, 10)
    _, after_bg = app.cell_colors(col, row)
    assert after_bg == resting_bg


def test_hovering_one_tab_leaves_the_other_at_rest():
    app = _tab_bar_app()
    alpha_row, alpha_col = _locate(app, "alpha")
    _, alpha_resting = app.cell_colors(alpha_col, alpha_row)

    row, col = _locate(app, "beta")
    _move(app, col, row)
    _, alpha_after = app.cell_colors(alpha_col, alpha_row)
    assert alpha_after == alpha_resting
