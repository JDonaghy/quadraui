"""Python bindings over the quadraui headless TUI driver.

``QuiApp`` is a cell-grid app: insert one dict-shaped descriptor per core
primitive (button, text display, list, tree, data table, tab bar), then
read the rendered frame back with ``screen()`` / ``screen_contains()``.

Every error ``QuiApp`` raises itself (as opposed to one a registered
Python callback raised, which passes through unchanged) is a
``QuadrauiError`` subclass: ``InvalidDescriptorError``, ``SlotOccupiedError``,
``UnknownSlotError``, ``InvalidDimensionsError``, ``InvalidCharError``,
``PanicError``, ``PoisonedError``, and ``InternalError``.
"""

from ._quadraui import (
    InternalError,
    InvalidCharError,
    InvalidDescriptorError,
    InvalidDimensionsError,
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
]
