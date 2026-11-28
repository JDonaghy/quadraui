"""Black-box coverage for `quadraui._ext`, which picks the compiled extension
(the GTK-enabled `_quadraui_gtk` when the Linux wheel ships it and the host
can load it, the GTK-free `_quadraui` otherwise) and for the
`UnsupportedError` `run_native` raises on Linux when no native backend
loaded.

Each check runs in a fresh interpreter, because the choice is made once at
`import quadraui` time and depends on the environment that import sees.
"""

import os
import subprocess
import sys
import textwrap

import pytest


def _run(body, **env):
    full_env = dict(os.environ)
    full_env.update(env)
    result = subprocess.run(
        [sys.executable, "-c", textwrap.dedent(body)],
        capture_output=True,
        text=True,
        timeout=60,
        env=full_env,
    )
    assert result.returncode == 0, f"stdout={result.stdout!r} stderr={result.stderr!r}"
    return result.stdout


def test_loaded_extension_is_registered_as_quadraui_underscore_quadraui():
    out = _run(
        """
        import sys
        import quadraui
        from quadraui import _quadraui
        import quadraui._quadraui as dotted
        assert sys.modules["quadraui._quadraui"] is quadraui._quadraui
        assert _quadraui is quadraui._quadraui is dotted
        assert quadraui.QuiApp is quadraui._quadraui.QuiApp
        assert quadraui.UnsupportedError is quadraui._quadraui.UnsupportedError
        print("SAME_MODULE", quadraui._quadraui.__name__)
        """
    )
    assert "SAME_MODULE" in out


def test_gtk_variant_never_loads_beside_the_plain_extension():
    out = _run(
        """
        import os
        import sys
        import quadraui
        loaded = [
            name for name in ("_quadraui", "_quadraui_gtk")
            if any(
                getattr(mod, "__file__", None)
                and os.path.basename(mod.__file__).startswith(name + ".")
                for mod in list(sys.modules.values())
            )
        ]
        print("LOADED", loaded)
        assert len(loaded) == 1, loaded
        """
    )
    assert "LOADED" in out


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="Linux-only fallback")
def test_disable_gtk_makes_run_native_raise_unsupported_error_on_linux():
    out = _run(
        """
        import quadraui
        if quadraui._quadraui._NATIVE_BACKEND is not None:
            # A `maturin develop --features gtk` build: GTK is compiled into
            # `_quadraui` itself, so there is no variant to skip.
            print("NATIVE_BUILT_IN")
        else:
            try:
                quadraui.QuiApp(10, 3).run_native()
            except quadraui.UnsupportedError as exc:
                print("UNSUPPORTED", exc)
        """,
        QUADRAUI_DISABLE_GTK="1",
    )
    if "NATIVE_BUILT_IN" in out:
        pytest.skip("extension built with GTK compiled in; nothing to disable")
    assert "UNSUPPORTED" in out
    assert "QUADRAUI_DISABLE_GTK" in out


def test_headless_driver_works_with_gtk_disabled():
    out = _run(
        """
        import quadraui
        app = quadraui.QuiApp(40, 5)
        app.set_render(lambda: {"button": {"id": "t:ok", "label": "OK"}})
        assert app.screen_contains("OK")
        print("RENDERED")
        """,
        QUADRAUI_DISABLE_GTK="1",
    )
    assert "RENDERED" in out
