"""Hot reload for `quadraui.app.App`: re-import edited source modules while
the app keeps running, and keep every piece of state the user built up.

What survives a reload, and why:

* **Component state.** `App` keys a component's state slot by the
  component class's *qualified name* (`module.Qualname`), not by the class
  object, so the class object a reload replaces still finds the slot the
  old one filled. On the first render after a reload each surviving slot
  passes through `Component.migrate_state(state)` once -- by default that
  adds any key the new `initial_state()` introduced and keeps every
  existing value.
* **App-object state.** A `view`/`on_event` that is a bound method keeps
  its instance: the instance's `__class__` is pointed at the reloaded
  class of the same qualified name, so every attribute already on the
  instance stays put and every method resolves to the new code. A plain
  function is looked up again by qualified name in its reloaded module.
* **Framework widget state.** Scroll offsets, selection, expanded rows and
  focus live in the wrapped `QuiApp`, keyed by widget id, and are never
  touched by a reload.

What happens on a broken edit: if re-importing raises (a syntax error, an
exception at import time) or the first render with the new code raises,
`App` puts the previous view, `on_event`, instance classes and module
namespaces back, records the exception on `App.reload_error`, and keeps
rendering the last good code; the next successful reload clears it.

What does not survive: module-level globals of a reloaded module are
re-executed (that is what re-importing means), and new attributes an
edited `__init__` would set are absent on an instance that already exists
-- read them with `getattr(self, name, default)` or set them in an
`on_reload` hook.

Change detection is a stat of each watched module's source file
(modification time and size). `App.reload_if_changed()` does one check
synchronously; `App.enable_hot_reload(..., interval=...)` additionally
starts a daemon thread that does the same check every `interval` seconds
and, on a change, wakes the app's event loop through `QuiApp.waker()`, so
the reload itself always runs on the thread driving `step()`/`run_tui()`.
"""

from __future__ import annotations

import importlib
import os
import sys
import threading
from types import ModuleType
from typing import Any, Callable, Dict, Iterable, List, Optional, Tuple, Union

__all__ = ["HOT_RELOAD_WAKE", "ModuleWatcher", "rebind_callable", "resolve_modules"]

#: Payload the watcher thread posts through `QuiAppWaker.post`; `App`
#: claims a `{"User": HOT_RELOAD_WAKE}` event itself instead of passing it
#: to `on_event`.
HOT_RELOAD_WAKE = "quadraui.app.hot_reload"

ModuleRef = Union[ModuleType, str]


def resolve_modules(modules: Iterable[ModuleRef]) -> List[ModuleType]:
    """Turns module objects or dotted names into module objects, rejecting
    anything a reload cannot re-import from a source file."""
    resolved: List[ModuleType] = []
    for module in modules:
        if isinstance(module, str):
            module = importlib.import_module(module)
        if not isinstance(module, ModuleType):
            raise TypeError(f"not a module or module name: {module!r}")
        if module.__name__ == "__main__" or getattr(module, "__spec__", None) is None:
            raise ValueError(
                f"cannot hot-reload {module.__name__!r}: it was not imported through the "
                "import system (a script run directly is `__main__`); move the view code "
                "into a module the script imports and watch that one"
            )
        if _source_path(module) is None:
            raise ValueError(f"cannot hot-reload {module.__name__!r}: it has no Python source file")
        resolved.append(module)
    return resolved


def _source_path(module: ModuleType) -> Optional[str]:
    path = getattr(module, "__file__", None)
    if not path or not path.endswith(".py"):
        return None
    return path


def _stamp(path: str) -> Optional[Tuple[int, int]]:
    try:
        st = os.stat(path)
    except OSError:
        return None
    return (st.st_mtime_ns, st.st_size)


class ModuleWatcher:
    """Remembers each watched module's source-file stamp and reports which
    modules changed since the stamps were last taken."""

    def __init__(self, modules: Iterable[ModuleType]) -> None:
        self.modules: List[ModuleType] = list(modules)
        self._stamps: Dict[str, Optional[Tuple[int, int]]] = {}
        self.snapshot()

    def snapshot(self) -> None:
        """Records every watched file's current stamp as the baseline."""
        for module in self.modules:
            path = _source_path(module)
            if path is not None:
                self._stamps[module.__name__] = _stamp(path)

    def changed(self) -> List[ModuleType]:
        """The watched modules whose source file differs from the baseline,
        in watch order. A file that is missing right now (an editor's
        write-by-rename in progress) does not count as changed."""
        out: List[ModuleType] = []
        for module in self.modules:
            path = _source_path(module)
            if path is None:
                continue
            stamp = _stamp(path)
            if stamp is not None and stamp != self._stamps.get(module.__name__):
                out.append(module)
        return out


class _PollThread:
    """Daemon thread calling `on_change()` whenever `watcher.changed()` is
    non-empty, at most once per distinct change (it waits until the app has
    taken a new snapshot before reporting again)."""

    def __init__(self, watcher: ModuleWatcher, interval: float, on_change: Callable[[], None]) -> None:
        self._watcher = watcher
        self._interval = interval
        self._on_change = on_change
        self._stop = threading.Event()
        self._reported: Optional[Tuple[Any, ...]] = None
        self._thread = threading.Thread(target=self._loop, name="quadraui-hot-reload", daemon=True)
        self._thread.start()

    def _loop(self) -> None:
        while not self._stop.wait(self._interval):
            changed = self._watcher.changed()
            if not changed:
                self._reported = None
                continue
            key = tuple(_stamp(_source_path(m) or "") for m in changed)
            if key != self._reported:
                self._reported = key
                self._on_change()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=max(1.0, self._interval * 2))


def drop_bytecode_cache(module: ModuleType) -> None:
    """Removes the module's cached `.pyc`, whose freshness check uses a
    whole-second source mtime plus size: two same-size edits inside one
    second would otherwise re-import the stale bytecode."""
    cached = getattr(module, "__cached__", None)
    if cached:
        try:
            os.remove(cached)
        except OSError:
            pass


def rebind_callable(func: Any, reloaded: Dict[str, ModuleType], rebound: Dict[int, Tuple[Any, type]]) -> Any:
    """The reloaded counterpart of `func`, or `func` itself when none of
    its code lives in a reloaded module.

    A bound method keeps its instance; the instance's class is swapped for
    the reloaded class of the same qualified name (recorded in `rebound`
    as `id(instance) -> (instance, old class)` so a failed reload can put
    it back). A plain function is looked up again by qualified name."""
    if func is None:
        return None
    instance = getattr(func, "__self__", None)
    if instance is not None and not isinstance(instance, ModuleType):
        cls = type(instance)
        new_cls = _lookup(reloaded.get(cls.__module__), cls.__qualname__)
        if isinstance(new_cls, type) and new_cls is not cls:
            if id(instance) not in rebound:
                rebound[id(instance)] = (instance, cls)
            instance.__class__ = new_cls
        return getattr(instance, func.__name__)
    module = reloaded.get(getattr(func, "__module__", None) or "")
    replacement = _lookup(module, getattr(func, "__qualname__", ""))
    return replacement if callable(replacement) else func


def _lookup(module: Optional[ModuleType], qualname: str) -> Any:
    if module is None or not qualname or "<locals>" in qualname:
        return None
    obj: Any = module
    for part in qualname.split("."):
        obj = getattr(obj, part, None)
        if obj is None:
            return None
    return obj


def reload_modules(modules: List[ModuleType]) -> Dict[str, Dict[str, Any]]:
    """Re-imports `modules` in order, returning each one's namespace as it
    was before (for `restore_modules`). Raises whatever the first failing
    re-import raised, after restoring every module it touched."""
    saved: Dict[str, Dict[str, Any]] = {}
    importlib.invalidate_caches()
    try:
        for module in modules:
            saved[module.__name__] = dict(module.__dict__)
            drop_bytecode_cache(module)
            importlib.reload(module)
    except BaseException:
        restore_modules(saved)
        raise
    return saved


def restore_modules(saved: Dict[str, Dict[str, Any]]) -> None:
    """Puts each module's namespace back to a `reload_modules` snapshot."""
    for name, namespace in saved.items():
        module = sys.modules.get(name)
        if module is None:
            continue
        module.__dict__.clear()
        module.__dict__.update(namespace)
