"""Black-box coverage for the `quadraui` package's own public surface: every
exception the compiled extension registers must be importable from the
top-level package, not just from the private `quadraui._quadraui` module.

The package's `__init__.py` re-exports names one at a time, so an exception
added on the Rust side is silently reachable only as
`quadraui._quadraui.SomeError` until someone remembers to list it. That
breaks the documented usage pattern every example and docstring uses
(`from quadraui import QuiApp, NoDisplayError`), so it is asserted here
rather than left to review.

Feature-independent: these are registered unconditionally by the pymodule
init, so this passes on a plain, `gtk` or `macos` build.

    maturin develop
    pytest quadraui-py/tests/test_public_api.py
"""

import quadraui
from quadraui import _quadraui as _native


def _registered_error_names():
    """Every `QuadrauiError` subclass the extension module registers."""
    return {
        name
        for name in dir(_native)
        if not name.startswith("_")
        and isinstance(getattr(_native, name), type)
        and issubclass(getattr(_native, name), _native.QuadrauiError)
    }


def test_every_registered_error_is_exported_from_the_package():
    missing = sorted(
        name for name in _registered_error_names() if not hasattr(quadraui, name)
    )
    assert missing == [], (
        f"registered by the extension but not re-exported from `quadraui`: {missing}"
    )


def test_every_registered_error_is_listed_in_dunder_all():
    missing = sorted(
        name for name in _registered_error_names() if name not in quadraui.__all__
    )
    assert missing == [], f"exported but absent from `quadraui.__all__`: {missing}"


def test_dunder_all_names_all_resolve():
    unresolvable = sorted(
        name for name in quadraui.__all__ if not hasattr(quadraui, name)
    )
    assert unresolvable == [], f"listed in `__all__` but not importable: {unresolvable}"


def test_not_on_main_thread_error_is_importable_and_catchable():
    # `run_native`'s doc promises callers can catch this by name on the
    # `macos` feature; the import below is the form that doc implies, and
    # the subclass check is what makes a bare `except QuadrauiError`
    # catch it too.
    from quadraui import NotOnMainThreadError, QuadrauiError

    assert issubclass(NotOnMainThreadError, QuadrauiError)
    try:
        raise NotOnMainThreadError("not on the main thread")
    except QuadrauiError as exc:
        assert "not on the main thread" in str(exc)
