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
and ``InvalidAppIdError``.
"""

from ._quadraui import (
    InternalError,
    InvalidAppIdError,
    InvalidCharError,
    InvalidDescriptorError,
    InvalidDimensionsError,
    NativeAlreadyRunError,
    NoDisplayError,
    NotOnMainThreadError,
    PanicError,
    PoisonedError,
    QuadrauiError,
    QuiApp,
    SlotOccupiedError,
    UnknownSlotError,
)

__all__ = [
    "QuiApp",
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
]
