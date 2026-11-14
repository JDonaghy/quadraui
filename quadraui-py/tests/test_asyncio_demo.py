"""Black-box acceptance test for `examples/asyncio_demo.py`: drives the
real `main()` coroutine (not a reimplementation of it) and asserts on the
state transitions and waker-delivered values it prints, so a later edit to
the demo that breaks the async-handler or waker-delivery path it
demonstrates fails here instead of only being caught by eye.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_asyncio_demo.py
"""

import asyncio
import importlib.util
import pathlib

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "asyncio_demo.py"
)


def _load_asyncio_demo():
    spec = importlib.util.spec_from_file_location("asyncio_demo", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_main_runs_the_async_handler_and_the_background_waker_delivery(capsys):
    asyncio_demo = _load_asyncio_demo()
    asyncio.run(asyncio_demo.main())

    out = capsys.readouterr().out
    # The `f` keypress's `async def` handler must have gone through
    # "fetching..." (dispatched without blocking on its own `await`) before
    # settling on "fetched".
    assert "'idle', 'fetching...', 'fetched'" in out
    # The background thread's `waker().post(...)` delivery must have
    # arrived and updated status through the same handler.
    assert "background-thread-result" in out
