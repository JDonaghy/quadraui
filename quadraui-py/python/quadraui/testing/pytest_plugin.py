"""The `pytest` packaging of `quadraui.testing`: a `quadraui_harness`
fixture, registered automatically by this distribution's `pytest11` entry
point, so a consumer's test file needs no `conftest.py` boilerplate to get
one.

    def test_saving(quadraui_harness):
        h = quadraui_harness(MyApp().app)
        h.find(role="button", text="Save").click()
        h.assert_text("Saved")

The fixture is a *factory*, not a harness: only the test knows how to
build the app under test, and one test may want two of them. Whatever it
builds is `close()`d when the test ends, so a test that enabled hot
reload cannot leak its watcher thread into the rest of the session.

This module is imported by `pytest` itself at startup, in any environment
where `quadraui` is installed. It therefore imports nothing from the
compiled extension -- `quadraui.testing` is pure Python, and a
`quadraui.app`/`QuiApp` import only ever happens inside a test's own
call to the factory.
"""

from __future__ import annotations

from typing import Any, Callable, List, Optional

import pytest

from . import Harness, harness

__all__ = ["harness_factory", "quadraui_harness"]


def harness_factory() -> Any:
    """The `quadraui_harness` fixture's whole body, as a plain generator:
    yields the factory, then closes everything the factory built. Factored
    out of the fixture itself so a test can drive both halves -- pytest
    refuses a direct call to a fixture function."""
    built: List[Harness] = []

    def build(app: Any, *, render: Optional[Callable[[], Any]] = None) -> Harness:
        wrapped = harness(app, render=render)
        built.append(wrapped)
        return wrapped

    yield build

    for wrapped in built:
        wrapped.close()


@pytest.fixture
def quadraui_harness() -> Any:
    """Builds `quadraui.testing.Harness` objects and closes them after the
    test: `quadraui_harness(app)` for a `quadraui.app.App`, or
    `quadraui_harness(qui_app, render=my_render)` for a plain `QuiApp`."""
    yield from harness_factory()
