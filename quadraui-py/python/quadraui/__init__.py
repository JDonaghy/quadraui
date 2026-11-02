"""Python bindings over quadraui's TUI driver, headless or real.

``QuiApp`` is a cell-grid app: insert one dict-shaped descriptor per core
primitive (button, text display, list, tree, data table, tab bar), then
either read the rendered frame back headlessly with ``screen()`` /
``screen_contains()``, or call ``run_tui()`` to hand the same descriptors to
a real terminal -- raw mode, alternate screen, live redraw on real
keyboard/resize events -- until a registered handler raises to quit.
Any number of further primitives (``progress``, ``spinner``, ``image``, ...)
insert additively through ``insert_extra(kind, descriptor)`` /
``clear_extra(kind)`` alongside the six core ones, with no fixed slot table
to grow; ``insert_progress``/``insert_spinner``/``insert_image`` are typed
convenience wrappers over the same mechanism.

Every error ``QuiApp`` raises itself (as opposed to one a registered
Python callback raised, which passes through unchanged) is a
``QuadrauiError`` subclass: ``InvalidDescriptorError``, ``SlotOccupiedError``,
``UnknownSlotError``, ``InvalidDimensionsError``, ``InvalidCharError``,
``PanicError``, ``PoisonedError``, ``InternalError``,
``NativeAlreadyRunError``, ``NoDisplayError``, ``NotOnMainThreadError``,
``InvalidAppIdError``, and ``InvalidFontRoleError``.

``QuiApp.viewport()``, ``line_height()``, ``char_width()``,
``list_char_width()``, ``chrome_line_height()``, ``scrollbar_reserve()``,
``scales_text_rows()``, ``snap_height(h)``, ``measure_text(text, role)``,
and ``backend_caps()`` expose the headless backend's own layout metrics and
declared capabilities, so an app can size and scroll content against real
measurements instead of a hardcoded guess.

``QuiApp.waker()`` returns a ``QuiAppWaker`` -- unlike ``QuiApp`` itself,
which is pinned to the thread that created it, this handle is safe to pass
to a background thread so it can call ``post(value)``/``wake()`` to deliver
a value back into the app's event stream as a ``{"User": value}`` event
and wake whichever loop currently owns it, without exiting. Its
``post_after(delay_ms, value)``/``wake_after(delay_ms)`` schedule the same
delivery after a delay, from any thread, as a lightweight stand-in for a
timer.

``run_native()`` dispatches a ``"WindowClose"`` event to a registered
handler when the window manager's close button (or Alt-F4, or the red
traffic-light button on macOS) asks to close the window, same as any other
event -- and unlike every other event, its default is to let the close
proceed: a handler that returns ``None`` (no explicit ``return``) or
``False`` does not block it, same as having no handler registered at all.
Only an explicit ``return True`` vetoes the close and leaves the window
open, e.g. to show an "unsaved changes?" prompt first. A terminal opened
via ``run_tui()`` has no close button to dispatch this for; quitting there
is entirely up to whatever keystroke a registered handler chooses to raise
on.

``QuiApp.run_shell(width, height, config, ...)`` builds a ``QuiShellApp``
instead: a headless VS Code-style app frame (activity bar + sidebar +
optional bottom-panel tab strip) around Python-registered content and
event handlers, driven by the same ``step``/``run`` discipline as ``QuiApp``
itself. ``config`` is a ``dict`` (``"title"``, ``"panels"``, ``"position"``,
``"bottom_panel"``, ...); sidebar show/hide/resize and bottom-panel tab
switching are handled by the shell chrome itself and reported back via
``set_on_shell_event``/``set_on_bottom_panel_event``.
"""

from ._quadraui import (
    InternalError,
    InvalidAppIdError,
    InvalidCharError,
    InvalidDescriptorError,
    InvalidDimensionsError,
    InvalidFontRoleError,
    NativeAlreadyRunError,
    NoDisplayError,
    NotOnMainThreadError,
    PanicError,
    PoisonedError,
    QuadrauiError,
    QuiApp,
    QuiAppWaker,
    QuiShellApp,
    SlotOccupiedError,
    UnknownSlotError,
)

__all__ = [
    "QuiApp",
    "QuiAppWaker",
    "QuiShellApp",
    "QuadrauiError",
    "InvalidDescriptorError",
    "SlotOccupiedError",
    "UnknownSlotError",
    "InvalidDimensionsError",
    "InvalidCharError",
    "PanicError",
    "PoisonedError",
    "InternalError",
    "NativeAlreadyRunError",
    "NoDisplayError",
    "NotOnMainThreadError",
    "InvalidAppIdError",
    "InvalidFontRoleError",
]
