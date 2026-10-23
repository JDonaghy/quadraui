"""Python bindings over the quadraui headless TUI driver.

``QuiApp`` is a cell-grid app: insert one dict-shaped descriptor per core
primitive (button, text display, list, tree, data table, tab bar), then
read the rendered frame back with ``screen()`` / ``screen_contains()``.
"""

from ._quadraui import QuiApp

__all__ = ["QuiApp"]
