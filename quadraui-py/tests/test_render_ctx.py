"""Black-box coverage for the render context a script `view()` callback
gets through `quadraui.app.App` -- `quadraui.RenderCtx`'s `measure_text`/
`line_height`/`char_width`/`cell_grid`/`scale`/`theme_token`/`style_token`,
and the `App` wrapper's own arity detection that forwards it only to a
`view` that actually declares a slot for it, so a pre-#2613 zero-argument
`view()` keeps working unchanged.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_render_ctx.py
"""

from quadraui import RenderCtx, current_ctx
from quadraui.app import App, Button, Component


def test_view_with_no_ctx_parameter_still_renders_unchanged():
    """The original zero-argument `view()` shape every script written
    before #2613 uses must keep working exactly as before -- `App` must
    not force a `ctx` argument on a callback that never asked for one."""

    def view():
        return Button("Save", on_click=lambda: None)

    app = App(20, 5, view=view)
    assert app.screen_contains("Save")


def test_view_declaring_ctx_receives_a_render_ctx_instance():
    captured = []

    def view(ctx):
        captured.append(ctx)
        return Button("Save", on_click=lambda: None)

    # `App.__init__` already forces one eager render (see its own doc), so
    # by the time `screen_contains` triggers a second one, `captured` has
    # two entries -- assert on the fact that matters (every entry is a
    # real `RenderCtx`) rather than an exact count tied to that detail.
    app = App(20, 5, view=view)
    assert app.screen_contains("Save")
    assert captured
    assert all(isinstance(ctx, RenderCtx) for ctx in captured)


def test_view_declaring_ctx_with_a_default_also_receives_it():
    captured = []

    def view(ctx=None):
        captured.append(ctx)
        return Button("Save", on_click=lambda: None)

    App(20, 5, view=view).screen()
    assert captured and captured[0] is not None


def test_ctx_reports_the_headless_tui_backend_as_a_cell_grid():
    # Every `ctx.*` call below runs *inside* `view` -- a `RenderCtx` is
    # only valid for the duration of the render call that handed it out
    # (see `quadraui.RenderCtx`'s own doc), so `captured` stashes results,
    # not the `ctx` object itself, for the assertions after `screen()`.
    captured = {}

    def view(ctx):
        captured["cell_grid"] = ctx.cell_grid()
        captured["scale"] = ctx.scale()
        return {}

    App(20, 5, view=view).screen()
    assert captured["cell_grid"] is True
    assert captured["scale"] == 1.0


def test_ctx_measure_text_counts_display_width_in_cells():
    captured = {}

    def view(ctx):
        captured["measured"] = ctx.measure_text("hello")
        captured["line_height"] = ctx.line_height()
        captured["char_width"] = ctx.char_width()
        return {}

    App(20, 5, view=view).screen()
    assert captured["measured"] == (5.0, 1.0)
    assert captured["line_height"] == 1.0
    assert captured["char_width"] == 1.0


def test_ctx_theme_token_and_style_token_look_up_fields_by_name():
    captured = {}

    def view(ctx):
        captured["accent_fg"] = ctx.theme_token("accent_fg")
        captured["unknown_theme"] = ctx.theme_token("not_a_real_field")
        captured["padding"] = ctx.style_token("padding")
        captured["unknown_style"] = ctx.style_token("not_a_real_field")
        return {}

    App(20, 5, view=view).screen()
    assert set(captured["accent_fg"].keys()) == {"r", "g", "b", "a"}
    assert captured["unknown_theme"] is None
    assert isinstance(captured["padding"], float)
    assert captured["unknown_style"] is None


def test_component_view_also_receives_ctx():
    """A `Component`'s own `view(state)` is unaffected -- only `App`'s
    top-level `view` callback is ctx-aware -- but a top-level `view(ctx)`
    that itself renders a `Component` still sees a real `RenderCtx`."""

    captured = []

    class Counter(Component):
        def initial_state(self):
            return {"n": 0}

        def view(self, state):
            return Button(str(state["n"]), on_click=lambda: None)

    def view(ctx):
        captured.append(ctx)
        return Counter()

    app = App(20, 5, view=view)
    assert app.screen_contains("0")
    assert isinstance(captured[0], RenderCtx)


def test_current_ctx_is_none_outside_a_render_call():
    assert current_ctx() is None
