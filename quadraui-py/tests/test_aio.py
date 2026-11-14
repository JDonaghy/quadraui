"""Black-box coverage for `quadraui.aio` (exposed as `quadraui.asyncio`):
`run`/`QuiApp.run_async` driving `step()`/render from a running asyncio
loop, and `async_handler` adapting an `async def` handler onto it -- see
`quadraui/aio.py`'s own module doc for the full contract each covers.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_aio.py
"""

import asyncio

import pytest

from quadraui import QuiApp
from quadraui.aio import async_handler, run


def _render_from(state):
    def render():
        return {
            "text_display": {
                "id": "aio-demo:status",
                "lines": [{"spans": [{"text": state["text"], "fg": None, "bg": None}]}],
            }
        }

    return render


async def _cancel_and_await(task):
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task


@pytest.mark.asyncio
async def test_async_handler_awaits_sleep_then_updates_state_and_screen_reflects_it():
    state = {"text": "before"}
    app = QuiApp(40, 10, render=_render_from(state))

    async def handler(event):
        if "KeyPressed" in event:
            await asyncio.sleep(0.05)
            state["text"] = "after"

    app.set_handler(async_handler(app, handler))
    app.push_key_event(ord("a"))

    task = asyncio.create_task(run(app, fallback_interval=0.05))
    try:
        # The handler's own task is still asleep, so the dispatch that
        # scheduled it must not have blocked on it: the screen keeps
        # showing the pre-update state right away.
        await asyncio.sleep(0.01)
        assert app.screen_contains("before")

        # Once the sleep elapses and the handler mutates `state`, the
        # wake-notify hook's prompt re-step (not the coarse fallback
        # poll -- see `run`'s own assertion below) must make the change
        # visible well before `fallback_interval` would have.
        await asyncio.sleep(0.2)
        assert app.screen_contains("after")
    finally:
        await _cancel_and_await(task)


@pytest.mark.asyncio
async def test_timer_posted_from_a_coroutine_arrives_promptly():
    received = []
    app = QuiApp(10, 10)
    app.set_handler(lambda event: received.append(event))

    async def poster():
        await asyncio.sleep(0.02)
        app.waker().post_after(20, "ding")

    poster_task = asyncio.create_task(poster())
    # A deliberately coarse fallback: if the delivery only reached `run`
    # through its own fallback poll instead of the wake-notify hook firing
    # promptly, this assertion below would need several seconds, not the
    # much shorter sleep the test actually waits.
    run_task = asyncio.create_task(run(app, fallback_interval=5))
    try:
        await asyncio.sleep(0.3)
        assert {"User": "ding"} in received
    finally:
        await poster_task
        await _cancel_and_await(run_task)


@pytest.mark.asyncio
async def test_cancellation_shuts_down_cleanly():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    task = asyncio.create_task(run(app, fallback_interval=0.05))
    await asyncio.sleep(0.05)
    await _cancel_and_await(task)

    # The wake-notify hook `run` armed must have been disarmed as part of
    # that clean shutdown: a later `wake()` must not resurrect the
    # cancelled run loop or raise for calling into a now-stale callback.
    app.waker().wake()
    still_running = app.step()
    assert still_running is True
    assert received and received[-1] == {"User": None}


@pytest.mark.asyncio
async def test_run_async_method_is_equivalent_to_run():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.waker().wake()
    task = asyncio.create_task(app.run_async(fallback_interval=0.05))
    try:
        await asyncio.sleep(0.05)
        assert received == [{"User": None}]
    finally:
        await _cancel_and_await(task)


@pytest.mark.asyncio
async def test_async_handler_passes_through_a_plain_sync_handler_unchanged():
    received = []

    def handler(event):
        received.append(event)

    app = QuiApp(10, 10)
    wrapped = async_handler(app, handler)
    assert wrapped is handler

    app.set_handler(wrapped)
    app.waker().wake()
    app.step()
    assert received == [{"User": None}]


@pytest.mark.asyncio
async def test_async_handler_done_callback_does_not_self_sustain_a_wake_loop():
    # Regression: an `async_handler`-wrapped handler's own `User` event
    # (the one its own done-callback's `waker().notify()` would turn into
    # if it used `wake()`/`post()` instead) must not immediately dispatch
    # right back into the same handler -- that would schedule another
    # task, whose own completion notifies again, forever, pegging the
    # loop at 100% CPU with no bound on how many times the handler runs.
    dispatch_count = 0
    app = QuiApp(10, 10)

    async def handler(event):
        nonlocal dispatch_count
        dispatch_count += 1

    app.set_handler(async_handler(app, handler))
    app.push_key_event(ord("a"))

    task = asyncio.create_task(run(app, fallback_interval=0.2))
    try:
        # One dispatch (the pushed key event) runs its task, which
        # notifies on completion; give the loop a few iterations' worth
        # of time to prove that notification doesn't cascade into more.
        await asyncio.sleep(0.15)
        assert dispatch_count == 1
    finally:
        await _cancel_and_await(task)
