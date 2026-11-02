"""Black-box coverage for `QuiApp.waker()` / `QuiAppWaker.post`/`.wake`:
event injection from a background thread into the headless `step`/`run`
loop, delivered as a `{"User": value}` event without crashing or exiting.

`UiEvent::User`'s `Serialize` always errors -- it carries an opaque,
in-process-only payload with no wire shape -- so these tests also exercise
the binding's dedicated `UiEvent::User` dispatch path (matched ahead of the
general `pythonize` call every other event goes through) through the real
compiled extension, not just the in-crate Rust harness.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_waker.py
"""

import threading

from quadraui import QuiApp


def test_waker_post_from_a_background_thread_is_delivered_as_a_user_event():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    waker = app.waker()
    done = threading.Event()

    def background():
        waker.post({"job": "finished", "result": 42})
        done.set()

    worker = threading.Thread(target=background)
    worker.start()
    worker.join(timeout=5)
    assert done.is_set(), "the background thread must finish posting"

    still_running = app.run(1)

    assert still_running is True, "a User event must not exit the app"
    assert len(received) == 1
    event = received[0]
    assert "User" in event
    assert event["User"] == {"job": "finished", "result": 42}


def test_waker_wake_with_no_payload_is_delivered_as_user_none():
    app = QuiApp(40, 20)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.waker().wake()
    app.run(1)

    assert len(received) == 1
    assert received[0] == {"User": None}


def test_waker_obtained_before_any_step_still_works():
    # `waker()` must be usable immediately after construction, before the
    # app has ever been stepped/run -- it is not lazily bound to whatever
    # backend happens to be driving at call time.
    app = QuiApp(10, 10)
    waker = app.waker()
    received = []
    app.set_handler(lambda event: received.append(event))

    waker.post("hello")
    app.step()

    assert received == [{"User": "hello"}]


def test_waker_post_with_no_handler_registered_does_not_raise():
    app = QuiApp(10, 10)
    app.waker().post("nobody is listening")

    # No handler registered at all: `PyAppLogic::handle` never reaches the
    # engine, so the event is simply dropped -- `step`/`run` must still
    # report the app as running, not raise or exit.
    still_running = app.step()
    assert still_running is True


def test_multiple_posts_are_delivered_in_order():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event["User"]))

    waker = app.waker()
    waker.post(1)
    waker.post(2)
    waker.post(3)

    app.run(1)

    assert received == [1, 2, 3]
