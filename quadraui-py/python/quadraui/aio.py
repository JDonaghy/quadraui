"""An asyncio-native runner for `QuiApp`: drives `step()`/render from a
running asyncio event loop instead of a plain `for`/`while` loop around
`step()`/`run()` -- the Python counterpart to `quadraui-node`'s own
`js/index.ts` `run()` (see that module's doc for the shape this mirrors).

`run(app)` (or, equivalently, `await app.run_async()` -- this module patches
that method onto `QuiApp` as a thin wrapper the moment it is imported, which
`quadraui/__init__.py` always does) awaits forever, calling `app.step()`
each time either of two things happens: `app.arm_wake_notify()`'s callback
fires (a `QuiAppWaker.post`/`.wake`/`.post_after`/`.wake_after` call
delivered, from any thread, including one this same loop's own `async def`
handler triggered through `post_after`/`wake_after`), or `fallback_interval`
seconds have elapsed with nothing else to report. Returns once `step()`
reports the app has exited; propagates `asyncio.CancelledError` after
disarming the wake-notify hook if the awaiting task is cancelled instead
-- see `run`'s own doc for the full contract, including why that fallback
poll exists at all and what it still cannot cover.

`async_handler(app, handler)` adapts a `handler` that may be a plain
callable or an `async def` into the plain callable every `set_handler`/
`set_render_content`/... registration point expects -- see its own doc for
why an `async def` handler cannot be registered directly, and what
"awaited... must not block rendering, so later events queue" means in
practice: the handler's own coroutine runs as a separate `asyncio.Task`,
the wrapper returns immediately without awaiting it, and whatever event
arrives next (new keypress, new `post`) dispatches and renders normally
while that task is still suspended on its own `await`.
"""

from __future__ import annotations

import asyncio
import functools
import inspect
from typing import Any, Awaitable, Callable, Coroutine, Union, cast

from ._quadraui import QuiApp

__all__ = ["run", "async_handler"]

#: `quadraui-node`'s own `run` (`js/index.ts`) defaults its matching
#: fallback poll to 250ms for the same reason documented there: most of
#: what it stands in for (real OS/terminal input on an idle session) is
#: rare compared to a `QuiAppWaker` delivery, which this module's own
#: `arm_wake_notify` wiring observes instantly regardless of this value.
_DEFAULT_FALLBACK_INTERVAL = 0.25


async def run(app: "QuiApp", *, fallback_interval: float = _DEFAULT_FALLBACK_INTERVAL) -> None:
    """Drives `app.step()` from the currently running asyncio loop until it
    exits or this coroutine is cancelled.

    Arms `app.arm_wake_notify()` with a callback that sets an
    `asyncio.Event` via `loop.call_soon_threadsafe` -- the thread-safe way
    to wake *this* loop from whatever thread a `QuiAppWaker.post`/`.wake`/
    `.post_after`/`.wake_after` call happens to fire on, including a
    `post_after`/`wake_after` call's own spawned timer thread, or a
    background thread wholly unrelated to asyncio (the exact scenario
    `examples/waker_demo.py` exercises against `run_tui()` instead of this
    loop). A delivery is observed the instant it fires rather than waiting
    for `fallback_interval` to elapse.

    That wake-notify hook covers everything delivered through a
    `QuiAppWaker` handle -- it does **not** cover real OS/terminal input (a
    keypress, a resize) arriving on a live terminal/native session: no
    backend exposes a way to observe that without a thread polling for it,
    the same gap `quadraui-node`'s own `run()` documents and works around
    identically (its `fallbackIntervalMs`, this function's
    `fallback_interval`), until a backend grows a real wake primitive for
    native/terminal input. Driving a live `run_tui()`/`run_native()`
    session through this loop needs that polling fallback to actually pick
    up keyboard/mouse/window input at all -- this function only calls
    `app.step()`, which never opens a live session itself, so a caller
    wanting that combination still owns wiring it (push real input into
    `push_event`/`push_key_event` from whatever thread is polling for it).

    Calls `step()` once immediately, before the first wait, so the first
    frame renders without waiting on either the wake notify or the
    fallback timer. Disarms the wake-notify hook before returning, whether
    `step()` reported the app exited or this coroutine's own task was
    cancelled -- an `asyncio.CancelledError` propagates after that cleanup
    runs, same as any other well-behaved coroutine's `finally`, so a
    caller awaiting a cancelled run task still sees the cancellation.
    Re-raises whatever exception a registered handler/render callback
    raised, same as `step()`/`run()` themselves.

    Calls `step()` at most once per wake (wake-notify delivery or
    `fallback_interval` elapsing), and `step()` itself dispatches at most
    one queued event, so a burst of several `push_event`/`push_key_event`
    calls queued back-to-back drains at one per `fallback_interval` (the
    wake-notify hook does not re-fire for events already in that queue).
    `app.waker().post(...)`/`post_after(...)` are unaffected -- draining the
    *user* value queue is `step()`'s own job, not this loop's, and it
    drains the whole thing in one call. `quadraui-node`'s own `run()` has
    the identical per-wake shape, so this is parity with it, not a
    regression unique to this module.
    """
    loop = asyncio.get_running_loop()
    wake = asyncio.Event()

    def _notify() -> None:
        # If the task running this coroutine was dropped while cancelled
        # but never awaited (the "Task was destroyed but it is pending"
        # shape -- the `finally` below never ran, so the hook is still
        # armed), `loop` may already be closed by the time a later waker
        # delivery fires this. Swallow that one case quietly instead of
        # letting `fire_wake_notify` turn it into a traceback on stderr for
        # a loop shutdown that already happened.
        try:
            loop.call_soon_threadsafe(wake.set)
        except RuntimeError:
            pass

    app.arm_wake_notify(_notify)
    try:
        running = app.step()
        while running:
            wake.clear()
            try:
                await asyncio.wait_for(wake.wait(), timeout=fallback_interval)
            except asyncio.TimeoutError:
                pass
            running = app.step()
    finally:
        app.disarm_wake_notify()


async def _run_async_method(
    self: "QuiApp", *, fallback_interval: float = _DEFAULT_FALLBACK_INTERVAL
) -> None:
    await run(self, fallback_interval=fallback_interval)


def _is_async_callable(handler: Callable[[Any], Any]) -> bool:
    """`inspect.iscoroutinefunction(handler)` only recognises an `async def`
    function (or bound method) itself -- a callable *object* whose
    `__call__` is `async def` (or a `functools.partial` wrapping either)
    reports `False`, which would let it through `async_handler` unwrapped
    and silently hand back a never-awaited coroutine, the exact failure
    this module exists to prevent. Unwraps `functools.partial` down to its
    underlying callable and falls back to the type's own `__call__` before
    giving up.
    """
    while isinstance(handler, functools.partial):
        handler = handler.func
    if inspect.iscoroutinefunction(handler):
        return True
    call = getattr(type(handler), "__call__", None)
    return call is not None and inspect.iscoroutinefunction(call)


_run_async_method.__doc__ = (
    "Equivalent to `quadraui.aio.run(self, fallback_interval=fallback_interval)` "
    "-- see that function's own doc for the full contract. Patched onto "
    "`QuiApp` by this module, imported unconditionally as `quadraui.asyncio` "
    "by `quadraui/__init__.py`, so `await app.run_async()` works with no "
    "further import of its own."
)
# `_quadraui.pyi` (generated from `quadraui-py/src/lib.rs`'s own
# `#[pymethods]`, not from this file) has no `run_async` entry -- this is
# a real method, reachable and awaitable at runtime, just one `QuiApp`'s
# own compiled-extension type doesn't declare for a type checker; a
# caller who wants it type-checked uses `quadraui.aio.run(app)` directly
# instead, which needs no such patch.
QuiApp.run_async = _run_async_method  # type: ignore[attr-defined]


def async_handler(
    app: "QuiApp", handler: Callable[[Any], Union[Any, Awaitable[Any]]]
) -> Callable[[Any], Any]:
    """Adapts `handler` -- a plain callable, or an `async def` -- into the
    plain, synchronous callable `QuiApp.set_handler`/`set_render_content`/
    `QuiShellApp.set_handler`/... itself always expects. Every dispatch into
    a registered handler is a direct Rust -> Python call with no `await`
    anywhere on that path, so registering an `async def` directly would
    just hand back an unawaited, never-run coroutine object (and a Python
    "coroutine was never awaited" warning) instead of ever running it.

    A plain (non-`async def`) `handler` passes through completely
    unchanged -- wrapping it would add a pointless `asyncio` call to every
    dispatch for no behavioural difference, and its return value (a
    `WindowClose` veto, an exit request -- see `QuiApp`'s own module doc)
    keeps working exactly as it always did.

    An `async def` handler is scheduled as a task on the asyncio loop
    currently running (there must be one -- call this from inside, or
    after, the `run()`/`run_async()` call that will drive `app`) via
    `asyncio.get_running_loop().create_task`, and the returned wrapper
    itself returns `None` immediately without awaiting it: the dispatch
    that invoked it, and the render that follows, are never blocked on
    whatever the handler itself awaits, so a second event arriving before
    the first handler's task finishes dispatches and renders normally
    instead of waiting -- the "later events queue" behaviour this module's
    own doc names.

    Once the handler's task finishes, `app.waker().notify()` prompts an
    immediate re-step through `run`'s own wake-notify hook (rather than
    only on its next `fallback_interval` poll), so a state mutation the
    handler made after its last `await` becomes visible right away --
    `notify()`, not `wake()`/`post()`: queueing an actual `{"User": ...}`
    event here would dispatch right back into this same wrapped handler,
    whose own task finishing would notify again, forever. An exception the
    task raised does not propagate back into `run`'s own loop -- by the
    time it raises, the dispatch that scheduled the task has long since
    returned -- but is not silently dropped either: it is retrieved and
    handed to the loop's own exception handler (the default one logs it to
    stderr), the same as any other fire-and-forget background task's
    unretrieved exception.
    """
    if not _is_async_callable(handler):
        return handler

    waker = app.waker()

    def _dispatch(event: Any) -> None:
        loop = asyncio.get_running_loop()
        task = loop.create_task(cast(Coroutine[Any, Any, Any], handler(event)))

        def _on_done(done: "asyncio.Task[Any]") -> None:
            if not done.cancelled():
                exc = done.exception()
                if exc is not None:
                    loop.call_exception_handler(
                        {
                            "message": "exception in quadraui.aio.async_handler task",
                            "exception": exc,
                            "task": done,
                        }
                    )
            waker.notify()

        task.add_done_callback(_on_done)

    return _dispatch
