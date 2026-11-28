"""Picks which compiled extension module backs the ``quadraui`` package.

The Linux wheel ships two builds of the same extension side by side:

- ``_quadraui_gtk`` -- built with the ``gtk`` feature, so ``QuiApp.run_native``
  opens a GTK 4 window. It links the system ``libgtk-4.so.1`` (plus Pango,
  Cairo, GLib, GdkPixbuf and Fontconfig) dynamically; the dynamic loader
  refuses to load it on a machine without those libraries, or with a GTK
  older than the one it was built against, and Python reports that as an
  ``ImportError``.
- ``_quadraui`` -- built with no native feature. It has no system
  dependencies beyond glibc, so it always loads, and gives the terminal
  backend (``run_tui``) and the headless driver.

This module tries ``_quadraui_gtk`` first and falls back to ``_quadraui``.
Exactly one of the two is loaded in a process: each carries its own copy of
every class and exception type, so mixing them would make
``isinstance``/``except`` checks fail across the boundary. Whichever one
loads is also registered as ``quadraui._quadraui`` in ``sys.modules``, so
``from quadraui import _quadraui`` and ``import quadraui._quadraui`` reach
the same module object everywhere.

macOS and Windows wheels carry only ``_quadraui`` (built with the ``macos`` or
``win`` feature, whose system libraries every install already has), so the
first import attempt there finds no ``_quadraui_gtk`` and moves straight on.

Setting ``QUADRAUI_DISABLE_GTK=1`` in the environment skips the GTK variant,
for a process that never opens a window and does not want GTK loaded.

``native_unavailable_reason`` is ``None`` when a native backend loaded, and
otherwise a sentence explaining why ``run_native`` is unavailable; the
package's ``__init__`` raises it from ``QuiApp.run_native``.
"""

import importlib
import os
import sys
from types import ModuleType
from typing import Optional

_PACKAGE = __name__.rpartition(".")[0]

native_unavailable_reason: Optional[str] = None

_GTK_INSTALL_HINT = (
    "Install GTK 4.10 or newer from your distribution "
    "(Ubuntu/Debian: `sudo apt install libgtk-4-1`; Fedora: `sudo dnf install gtk4`; "
    "Arch: `sudo pacman -S gtk4`), or use `run_tui()` instead."
)


def _gtk_probe() -> str:
    """Names ``libgtk-4.so.1`` as missing when the loader cannot open it.

    The loader's own error names only the first unresolved ``DT_NEEDED``
    entry, which on a host without GTK is often a Pango or Cairo library
    rather than GTK itself.
    """
    import ctypes

    try:
        ctypes.CDLL("libgtk-4.so.1")
    except OSError:
        return " (libgtk-4.so.1 is not installed)"
    return ""


def _load() -> ModuleType:
    global native_unavailable_reason
    gtk_error: Optional[ImportError] = None
    disabled = os.environ.get("QUADRAUI_DISABLE_GTK", "") not in ("", "0")
    if not disabled:
        try:
            return importlib.import_module(f"{_PACKAGE}._quadraui_gtk")
        except ModuleNotFoundError as exc:
            if exc.name != f"{_PACKAGE}._quadraui_gtk":
                raise
        except ImportError as exc:
            gtk_error = exc

    module = importlib.import_module(f"{_PACKAGE}._quadraui")
    if getattr(module, "_NATIVE_BACKEND", None) is None and sys.platform.startswith("linux"):
        if disabled:
            native_unavailable_reason = (
                "run_native is disabled because QUADRAUI_DISABLE_GTK is set in the "
                "environment; unset it to load the GTK backend."
            )
        elif gtk_error is not None:
            native_unavailable_reason = (
                f"run_native needs the system GTK 4 libraries{_gtk_probe()}, but the "
                f"GTK build of quadraui could not be loaded: {gtk_error}. "
                f"{_GTK_INSTALL_HINT}"
            )
        else:
            native_unavailable_reason = (
                "run_native is not available in this quadraui build: the installed "
                "package carries no GTK backend. Prebuilt GTK-enabled Linux wheels "
                "exist only for x86_64 with glibc 2.39 or newer (e.g. Ubuntu 24.04 "
                "or Fedora 40). On another architecture, an older glibc, or a "
                "source install, build with the GTK feature instead: install the "
                "GTK 4 development package (`libgtk-4-dev` or `gtk4-devel`) and a "
                "Rust toolchain, then run `MATURIN_PEP517_ARGS=\"--features gtk\" "
                "pip install --no-binary quadraui quadraui`. Otherwise use "
                "`run_tui()`."
            )
    return module


ext = _load()
sys.modules[f"{_PACKAGE}._quadraui"] = ext
