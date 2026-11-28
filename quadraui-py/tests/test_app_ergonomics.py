"""`App.quit()`, the default quit keys, the optional `App` size, `Text`, and
per-child `size`/`min`/`max` on `Column`/`Row` children.

`quadraui-node/tests/app_ergonomics.test.ts` covers the same behaviour for
the Node app layer, case for case."""

import asyncio

import pytest

from quadraui.app import App, Button, Column, Form, List, Row, Text, TextDisplay, TextInputField
from quadraui.testing import harness


def _counter_app(**kwargs):
    clicks = []

    def view():
        return Column(Button("Quit", id="quit", on_click=lambda: app.quit()), Button("Count", id="count", on_click=lambda: clicks.append(1)))

    app = App(20, 4, view=view, **kwargs)
    return app, clicks


def _key(name, **mods):
    key = {"Named": name} if len(name) > 1 else {"Char": name}
    modifiers = {"shift": False, "ctrl": False, "alt": False, "cmd": False, **mods}
    return {"KeyPressed": {"key": key, "modifiers": modifiers, "repeat": False}}


# --- App.quit() ---------------------------------------------------------------


def test_quit_from_a_node_callback_ends_the_step_that_ran_it():
    app, _ = _counter_app()
    h = harness(app)
    h.find(id="quit").click()
    assert app.step() is False


def test_quit_from_on_event_ends_the_step_that_ran_it():
    def on_event(event):
        if event == {"User": "stop"}:
            app.quit()

    app = App(20, 4, view=lambda: Text("hi"), on_event=on_event)
    assert app.step() is True
    app.qui.waker().post("stop")
    assert app.step() is False


def test_quit_outside_any_callback_ends_the_next_step():
    app, _ = _counter_app()
    assert app.step() is True
    app.quit()
    assert app.step() is False


@pytest.mark.asyncio
async def test_quit_from_an_async_task_ends_run_async():
    app = App(20, 4, view=lambda: Text("hi"))

    async def later():
        await asyncio.sleep(0.05)
        app.quit()

    task = asyncio.create_task(later())
    await asyncio.wait_for(app.run_async(fallback_interval=0.02), 5)
    await task


def test_a_running_app_keeps_running_without_quit():
    app, clicks = _counter_app()
    h = harness(app)
    h.find(id="count").click()
    assert clicks == [1]
    assert app.step() is True


# --- default quit keys ----------------------------------------------------------


@pytest.mark.parametrize("event", [_key("q"), _key("Escape"), _key("c", ctrl=True)])
def test_q_escape_and_ctrl_c_quit_by_default(event):
    app, _ = _counter_app()
    assert app.step() is True
    app.push_event(event)
    assert app.step() is False


def test_ctrl_c_quits_even_inside_a_focused_widget():
    app = App(20, 6, view=lambda: List([{"text": {"spans": [{"text": "a"}]}}], id="items"))
    app.push_event({"List": ["items", {"KeyPressed": {"key": "c", "modifiers": {"ctrl": True}}}]})
    assert app.step() is False


def test_q_consumed_by_a_focused_widget_does_not_quit():
    app = App(20, 6, view=lambda: List([{"text": {"spans": [{"text": "a"}]}}], id="items"))
    app.push_event({"List": ["items", {"KeyPressed": {"key": "q", "modifiers": {}}}]})
    assert app.step() is True


def _draft_form_app():
    draft = [""]

    def edit(text):
        draft[0] = text

    def view():
        return Form([TextInputField(draft[0], id="draft", label="New", on_change=edit)], id="form")

    app = App(30, 4, view=view)
    h = harness(app)
    h.click_at(10, 0)
    return app, h, draft


def test_q_typed_into_a_focused_text_field_edits_it_and_does_not_quit():
    app, h, draft = _draft_form_app()
    h.type("aq")
    assert draft[0] == "aq"
    assert app.step() is True


def test_escape_a_focused_text_field_leaves_unconsumed_still_quits():
    app, h, _ = _draft_form_app()
    h.press("Escape")
    assert app.step() is False


def test_other_keys_and_modified_q_do_not_quit():
    app, _ = _counter_app()
    for event in (_key("x"), _key("Enter"), _key("q", alt=True), _key("q", ctrl=True)):
        app.push_event(event)
        assert app.step() is True


def test_on_event_returning_true_keeps_a_quit_key():
    seen = []

    def on_event(event):
        seen.append(event)
        return True

    app = App(20, 4, view=lambda: Text("hi"), on_event=on_event)
    app.push_event(_key("q"))
    assert app.step() is True
    assert _key("q") in seen


def test_on_event_sees_a_quit_key_before_the_app_quits():
    seen = []
    app = App(20, 4, view=lambda: Text("hi"), on_event=seen.append)
    app.push_event(_key("Escape"))
    assert app.step() is False
    assert _key("Escape") in seen


def test_quit_keys_false_turns_every_default_quit_key_off():
    app, _ = _counter_app(quit_keys=False)
    for event in (_key("q"), _key("Escape"), _key("c", ctrl=True)):
        app.push_event(event)
        assert app.step() is True


# --- optional size ---------------------------------------------------------------


def test_app_size_defaults_to_80_by_24():
    app = App(view=lambda: Text("hi"))
    rows = app.screen().rstrip("\n").split("\n")
    assert len(rows) == 24
    assert all(len(row) == 80 for row in rows)


# --- Text ------------------------------------------------------------------------


def _wire(node):
    from quadraui.app.runtime import _walk

    return _walk(node, (), {}, {}, {})


def test_text_builds_a_text_display_with_one_plain_line_per_line():
    wire = _wire(Text("one\ntwo", id="t"))
    assert wire["text_display"]["lines"] == [{"spans": [{"text": "one"}]}, {"spans": [{"text": "two"}]}]
    assert wire["size"] == "Content"


def test_text_renders_and_takes_one_row_in_a_column():
    app = App(20, 6, view=lambda: Column(Text("Count: 3"), Text("below", size=None)))
    rows = app.screen().split("\n")
    assert "Count: 3" in rows[0]
    assert "below" in rows[1]


# --- per-child size / min / max ------------------------------------------------


def _row_of(screen, needle):
    return next(i for i, row in enumerate(screen.split("\n")) if needle in row)


def test_column_children_share_space_equally_by_default():
    app = App(20, 8, view=lambda: Column(TextDisplay([{"spans": [{"text": "top"}]}]), TextDisplay([{"spans": [{"text": "bottom"}]}])))
    assert _row_of(app.screen(), "bottom") == 4


# An `int` size, `min` and `max` are portable pixels; a TUI cell is 8 px
# wide and 16 px tall.


def test_a_fixed_size_child_takes_exactly_that_many_pixels():
    app = App(20, 8, view=lambda: Column(TextDisplay([{"spans": [{"text": "top"}]}], size=32), TextDisplay([{"spans": [{"text": "bottom"}]}])))
    assert _row_of(app.screen(), "bottom") == 2


def test_max_clamps_an_equal_share_child():
    app = App(20, 8, view=lambda: Column(TextDisplay([{"spans": [{"text": "top"}]}], max=48), TextDisplay([{"spans": [{"text": "bottom"}]}])))
    assert _row_of(app.screen(), "bottom") == 3


def test_min_floors_a_child():
    app = App(20, 8, view=lambda: Column(Text("top", min=80), TextDisplay([{"spans": [{"text": "bottom"}]}])))
    assert _row_of(app.screen(), "bottom") == 5


def test_row_children_take_their_size_along_the_width():
    app = App(20, 3, view=lambda: Row(Text("ab", size=32), Text("cd", size=None)))
    assert app.screen().split("\n")[0].index("cd") == 4


def test_a_content_sized_row_takes_its_tallest_child_height():
    app = App(20, 6, view=lambda: Column(Row(Text("ab"), Text("cd"), size="content"), Text("below", size=None)))
    assert _row_of(app.screen(), "ab") == 0
    assert _row_of(app.screen(), "cd") == 0
    assert _row_of(app.screen(), "below") == 1


def test_content_sized_text_in_a_row_shows_its_whole_line():
    app = App(30, 2, view=lambda: Row(Text("Name:"), Text("Ada Lovelace")))
    assert "Name:" in app.screen().split("\n")[0]
    assert "Ada Lovelace" in app.screen().split("\n")[0]


def test_a_content_sized_column_adds_its_children_and_gaps():
    app = App(20, 8, view=lambda: Column(Column(Text("a"), Text("b"), gap=16, size="content"), Text("below", size=None)))
    assert _row_of(app.screen(), "b") == 2
    assert _row_of(app.screen(), "below") == 3


@pytest.mark.parametrize(
    "size, wire",
    [(3, {"Fixed": 3}), ("30%", {"Percent": 0.3}), ("2fr", {"Weight": 2.0}), ("content", "Content"), ({"Weight": 1.5}, {"Weight": 1.5})],
)
def test_size_spellings_map_to_the_wire_section_size(size, wire):
    built = _wire(Column(Button("b", id="b", size=size, min=1, max=9)))
    child = built["column"][0]
    assert child["size"] == wire
    assert (child["min"], child["max"]) == (1, 9)


@pytest.mark.parametrize("size", ["wide", "x%", True])
def test_an_unknown_size_spelling_raises(size):
    with pytest.raises((ValueError, TypeError)):
        Button("b", size=size)


def test_a_child_without_size_carries_no_layout_keys():
    built = _wire(Column(Button("b", id="b")))
    assert set(built["column"][0]) == {"id", "button"}
