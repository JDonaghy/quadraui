"""Demonstrates `quadraui.asyncio.run`/`QuiApp.run_async` -- the
asyncio-native alternative to driving `step()` in a plain `while`/`for`
loop -- and `quadraui.asyncio.async_handler`, which lets a registered
event handler be an `async def`.

    maturin develop
    python quadraui-py/examples/asyncio_demo.py

Three things happen over this demo's ~1.5 second run, each exercising a
different part of `quadraui.aio` (see that module's own doc for the full
contract):

1. Pressing `f` (synthesized here as a pushed key event, since this demo
   is headless) triggers an `async def` handler that `await`s an
   `asyncio.sleep(0.4)` -- standing in for a slow network call -- and then
   updates shared state. The screen keeps showing "fetching..." the whole
   time that `await` is pending (rendering is never blocked on it), and
   flips to "fetched" the moment the handler's own task finishes.
2. A plain background thread (not asyncio at all -- the same shape
   `examples/waker_demo.py` exercises against a real terminal) posts a
   value through `QuiAppWaker.post` partway through the run, arriving
   promptly rather than waiting on this runner's own fallback poll.
3. The whole thing runs inside an `asyncio.wait_for` with a timeout, so
   cancelling the underlying `run_async` task (here, by that timeout
   simply elapsing) is the normal way this demo -- and any real
   application -- shuts the loop down.

This demo stays headless on purpose: `quadraui.aio.run`'s own module doc
explains why a *live* terminal/native session still needs a polling
fallback for real OS input until a backend grows a non-polling way to
observe it, same as `quadraui-node`'s own `run()` -- nothing new for this
asyncio integration to solve on its own.
"""

from __future__ import annotations

import asyncio
import threading
import time
from typing import TYPE_CHECKING, Any, Dict, List

from quadraui import QuiApp
from quadraui.aio import async_handler, run

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import UiEvent

WIDTH = 60
HEIGHT = 6


def _render_from(state: Dict[str, str]) -> Any:
    def render() -> Dict[str, Any]:
        return {
            "text_display": {
                "id": "asyncio-demo:status",
                "lines": [{"spans": [{"text": state["status"], "fg": None, "bg": None}]}],
            }
        }

    return render


def _post_in_background(waker: Any) -> None:
    """Stands in for a wholly non-asyncio background thread -- the same
    shape `examples/waker_demo.py` exercises against a real terminal
    loop -- to show that `run`'s wake-notify hook covers this delivery
    path too, not just an `async def` handler's own `await`."""
    time.sleep(0.6)
    waker.post("background-thread-result")


async def main() -> None:
    state: Dict[str, str] = {"status": "idle"}
    received: List[Any] = []
    transitions: List[str] = [state["status"]]
    app = QuiApp(WIDTH, HEIGHT, render=_render_from(state))

    def _set_status(value: str) -> None:
        state["status"] = value
        transitions.append(value)

    async def on_event(event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "f"}:
                _set_status("fetching...")
                await asyncio.sleep(0.4)
                _set_status("fetched")
            return
        if isinstance(event, dict) and "User" in event:
            received.append(event["User"])
            _set_status(f"status: {event['User']}")

    app.set_handler(async_handler(app, on_event))

    waker = app.waker()
    worker = threading.Thread(target=_post_in_background, args=(waker,), daemon=True)
    worker.start()

    # Synthesizes the `f` keypress a real terminal session would have
    # delivered from the keyboard.
    app.push_key_event(ord("f"))

    run_task = asyncio.ensure_future(run(app, fallback_interval=0.05))
    try:
        await asyncio.sleep(1.5)
    finally:
        run_task.cancel()
        try:
            await run_task
        except asyncio.CancelledError:
            pass
    worker.join(timeout=2)

    print(f"asyncio_demo: status transitions -> {transitions!r}")
    print(f"asyncio_demo: values received through the waker -> {received!r}")


if __name__ == "__main__":
    asyncio.run(main())
