"""Black-box coverage for `QuiApp.run_shell`: a headless `ShellApp`/
`ShellConfig`-backed app frame (activity bar + sidebar + optional
bottom-panel tab strip) driven from Python, with `AppShellEvent`/
`BottomPanelEvent` routed to registered callbacks.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_run_shell.py
"""

from quadraui import QuiApp

CONFIG = {
    "title": "Demo",
    "panels": [
        {"id": "panel:files", "icon": "@", "tooltip": "Files", "title": "FILES"},
    ],
    "bottom_panel": {
        "tabs": [
            {"id": "bp:a", "label": "ALPHA"},
            {"id": "bp:b", "label": "BETA"},
        ],
        "active_tab_id": "bp:a",
    },
}


def _new_shell(**kwargs):
    return QuiApp.run_shell(80, 24, CONFIG, **kwargs)


def test_initial_frame_shows_sidebar_and_active_tab_content():
    app = _new_shell()
    screen = app.screen()
    assert "FILES" in screen
    assert "ALPHA" in screen and "BETA" in screen
    # `StaticTabContent` paints the active tab's own id into the content
    # area, so the initially-active tab ("bp:a") is visible and the other
    # ("bp:b") is not.
    assert "bp:a" in screen
    assert "bp:b" not in screen


def test_clicking_the_active_panels_icon_toggles_the_sidebar():
    events = []
    app = _new_shell(on_shell_event=lambda event: events.append(event))

    x, y = app.find("@")
    app.push_click(x, y)
    app.run(2)  # drain both the MouseDown and MouseUp `push_click` queues

    assert not app.screen_contains("FILES"), (
        f"clicking the already-active panel's icon must hide the sidebar:\n{app.screen()}"
    )
    assert events == [{"SidebarHidden": None}]

    # Clicking the same icon again must bring it back.
    app.push_click(x, y)
    app.run(2)

    assert app.screen_contains("FILES")
    assert events[-1] == {"PanelChanged": {"panel_id": "panel:files"}}


def test_switching_a_bottom_panel_tab_changes_the_rendered_frame():
    events = []
    app = _new_shell(on_bottom_panel_event=lambda event: events.append(event))

    before = app.screen()
    assert "bp:a" in before
    assert "bp:b" not in before

    x, y = app.bottom_panel_tab_center(1)
    app.push_click(x, y)
    app.run(2)

    after = app.screen()
    assert "bp:b" in after, f"switching to the second tab must show its content:\n{after}"
    assert "bp:a" not in after, f"the first tab's content must no longer be painted:\n{after}"
    assert events == [{"TabActivated": "bp:b"}]


def test_render_content_callback_paints_into_the_main_content_area():
    app = _new_shell(render_content=lambda: ["hello from python"])
    app.run(1)
    assert app.screen_contains("hello from python")


def test_handler_receives_events_the_shell_chrome_did_not_consume():
    received = []
    app = _new_shell(on_event=lambda event: received.append(event))

    app.push_key_event(ord("x"))
    app.run(1)

    assert len(received) == 1
    assert "KeyPressed" in received[0]


def test_handler_exception_surfaces_out_of_run():
    def boom(_event):
        raise RuntimeError("boom")

    app = _new_shell(on_event=boom)
    app.push_key_event(ord("x"))

    try:
        app.run(1)
    except RuntimeError as err:
        assert str(err) == "boom"
    else:
        raise AssertionError("expected the handler's exception to propagate")
