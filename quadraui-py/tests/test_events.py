"""Black-box coverage for `QuiApp`'s event marshalling and run loop:
pushing an event/key, draining it through a registered Python handler via
`step`/`run`, a handler exception surfacing out of `run`, and the GIL being
released while `run` is in flight.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_events.py
"""

import threading
import time

import pytest

from quadraui import QuiApp


def test_push_key_event_runs_handler_with_pythonized_payload():
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:ok", "label": "OK"})

    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_key_event(ord("x"))
    still_running = app.run(1)

    assert still_running is True
    assert len(received) == 1
    event = received[0]
    assert "KeyPressed" in event
    assert event["KeyPressed"]["key"] == {"Char": "x"}
    assert app.screen_contains("OK")


def test_push_event_dict_runs_handler():
    app = QuiApp(40, 20)

    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_event({"WindowResized": {"viewport": {"width": 10.0, "height": 5.0, "scale": 1.0}}})
    app.run(1)

    assert len(received) == 1
    assert "WindowResized" in received[0]


def test_on_event_constructor_kwarg_registers_handler():
    received = []
    app = QuiApp(40, 20, on_event=lambda event: received.append(event))

    app.push_key_event(ord("a"))
    app.run(1)

    assert len(received) == 1


def test_handler_exception_propagates_out_of_run():
    app = QuiApp(40, 20)

    def boom(_event):
        raise ValueError("handler blew up")

    app.set_handler(boom)
    app.push_key_event(ord("z"))

    with pytest.raises(ValueError, match="handler blew up"):
        app.run(1)


def test_handler_exception_propagates_out_of_step():
    app = QuiApp(40, 20)

    def boom(_event):
        raise RuntimeError("step blew up")

    app.set_handler(boom)
    app.push_key_event(ord("q"))

    with pytest.raises(RuntimeError, match="step blew up"):
        app.step()


def test_run_processes_multiple_queued_events_in_order():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    for ch in "abc":
        app.push_key_event(ord(ch))

    app.run(3)

    assert len(received) == 3
    assert [event["KeyPressed"]["key"]["Char"] for event in received] == ["a", "b", "c"]


def test_tick_handler_runs_when_no_event_is_queued():
    app = QuiApp(40, 20)
    ticks = []
    app.set_tick_handler(lambda: ticks.append(1))

    app.run(2)

    assert len(ticks) == 2


def test_run_releases_the_gil_for_a_slow_handler():
    """A slow handler running inside `run` must not block a second Python
    thread from making progress — proof the GIL is released for the Rust
    loop and only re-acquired per callback."""
    app = QuiApp(40, 20)

    def slow_handler(_event):
        time.sleep(0.3)

    app.set_handler(slow_handler)
    app.push_key_event(ord("s"))

    progress = []
    stop = threading.Event()

    def background_counter():
        while not stop.is_set():
            progress.append(1)

    worker = threading.Thread(target=background_counter)
    worker.start()
    try:
        start = time.monotonic()
        app.run(1)
        elapsed = time.monotonic() - start
        # Give the background thread a brief extra window to accumulate
        # ticks before asking it to stop.
        time.sleep(0.05)
    finally:
        stop.set()
        worker.join(timeout=5)

    assert elapsed >= 0.3
    assert len(progress) > 0


def test_run_releases_the_gil_for_the_rust_loop_itself():
    """`run` with no handler/tick callback registered never re-enters the
    Python interpreter during the loop, so this specifically proves the
    `Python::detach` around the Rust loop — not just that some *callback*
    happens to release the GIL on its own (e.g. `time.sleep` does that
    regardless of our own GIL discipline)."""
    app = QuiApp(40, 20)

    progress = []
    stop = threading.Event()

    def background_counter():
        while not stop.is_set():
            progress.append(1)

    worker = threading.Thread(target=background_counter)
    worker.start()
    try:
        before = len(progress)
        # Large enough step count that the Rust-only loop (no Python
        # handler registered, so no bytecode runs on this thread) takes
        # long enough for a starved background thread to show it.
        app.run(3_000_000)
        after = len(progress)
    finally:
        stop.set()
        worker.join(timeout=5)

    assert after - before > 1000
