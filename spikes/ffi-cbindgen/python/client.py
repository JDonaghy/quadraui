"""Thin ctypes client over the hand-rolled + cbindgen `quadraui_ffi_cbindgen_spike`
cdylib. Spike evidence for #1640 — demonstrates what "real Python" looks like
calling the hand-rolled C ABI, not a shippable binding.

Run from this directory after building the crate:

    cargo build --release   # from spikes/ffi-cbindgen/
    python3 python/client.py

Three things this exercises end-to-end, matching lib.rs's doc comment:
  1. insert/update/remove over the opaque `u64` handle store
  2. one callback in a vtable (event JSON in, reaction JSON out)
  3. run/step/wake over the real Reaction/StepOutcome shapes
"""

from __future__ import annotations

import ctypes
import json
import os
import sys
from pathlib import Path

CRATE_DIR = Path(__file__).resolve().parent.parent
LIB_NAME = "libquadraui_ffi_cbindgen_spike.so"


def _find_library() -> Path:
    """Locate the built cdylib. Checks `$CARGO_TARGET_DIR` first (this repo's
    dev environment sets it globally), then cargo's own default `target/`
    layout relative to the crate, release before debug."""
    search_roots = []
    env_target_dir = os.environ.get("CARGO_TARGET_DIR")
    if env_target_dir:
        search_roots.append(Path(env_target_dir))
    search_roots.append(CRATE_DIR / "target")

    for root in search_roots:
        for profile in ("release", "debug"):
            candidate = root / profile / LIB_NAME
            if candidate.exists():
                return candidate

    raise FileNotFoundError(
        f"{LIB_NAME} not found under any of {search_roots}; "
        "run `cargo build` (or `--release`) in spikes/ffi-cbindgen/ first"
    )


lib = ctypes.CDLL(str(_find_library()))
# The callback trampoline below hands Rust a heap pointer it will later
# free via `CString::from_raw` (Rust's default System allocator wraps
# libc's malloc/free on this platform), so the buffer has to come from
# libc's malloc, not from Python/ctypes-managed memory.
libc = ctypes.CDLL(None)
libc.malloc.restype = ctypes.c_void_p
libc.malloc.argtypes = [ctypes.c_size_t]

# --- Opaque handle store ----------------------------------------------------
lib.quadra_store_new.restype = ctypes.c_void_p
lib.quadra_store_free.argtypes = [ctypes.c_void_p]
lib.quadra_store_insert.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.quadra_store_insert.restype = ctypes.c_uint64
lib.quadra_store_update.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
lib.quadra_store_update.restype = ctypes.c_bool
lib.quadra_store_remove.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
lib.quadra_store_remove.restype = ctypes.c_bool
lib.quadra_store_len.argtypes = [ctypes.c_void_p]
lib.quadra_store_len.restype = ctypes.c_size_t

# --- Callback vtable + runner ------------------------------------------------
HANDLE_CALLBACK = ctypes.CFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_char_p
)


class QuadraCallbacks(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("handle", HANDLE_CALLBACK),
    ]


lib.quadra_runner_new.argtypes = [QuadraCallbacks]
lib.quadra_runner_new.restype = ctypes.c_void_p
lib.quadra_runner_free.argtypes = [ctypes.c_void_p]
lib.quadra_runner_step.argtypes = [ctypes.c_void_p]
lib.quadra_runner_step.restype = ctypes.c_uint32
lib.quadra_runner_wake.argtypes = [ctypes.c_void_p]
lib.quadra_free_string.argtypes = [ctypes.c_void_p]

STEP_CONTINUE, STEP_EXITED = 0, 1


class DescriptorStore:
    """Pythonic wrapper around the opaque `u64`-handle store — this is the
    "does the generated/hand-written client read like real Python" bar the
    spike is evaluating."""

    def __init__(self) -> None:
        self._ptr = lib.quadra_store_new()

    def insert(self, label: str, value: float) -> int:
        payload = json.dumps({"label": label, "value": value}).encode("utf-8")
        handle = lib.quadra_store_insert(self._ptr, payload)
        if handle == 0:
            raise ValueError("insert rejected (bad JSON or null store)")
        return handle

    def update(self, handle: int, label: str, value: float) -> None:
        payload = json.dumps({"label": label, "value": value}).encode("utf-8")
        if not lib.quadra_store_update(self._ptr, handle, payload):
            raise KeyError(f"no live descriptor at handle {handle}")

    def remove(self, handle: int) -> None:
        if not lib.quadra_store_remove(self._ptr, handle):
            raise KeyError(f"no live descriptor at handle {handle}")

    def __len__(self) -> int:
        return lib.quadra_store_len(self._ptr)

    def close(self) -> None:
        if self._ptr:
            lib.quadra_store_free(self._ptr)
            self._ptr = None

    def __enter__(self) -> "DescriptorStore":
        return self

    def __exit__(self, *_exc_info: object) -> None:
        self.close()


class Runner:
    """Python-side `handle` implementation driving the `run`/`step`/`wake`
    triad. `on_event` receives a decoded event dict and returns a reaction
    dict; this class handles the JSON + C-string marshalling."""

    def __init__(self, on_event) -> None:
        self._on_event = on_event

        def _trampoline(_user_data: int, event_json: bytes) -> int:
            event = json.loads(event_json.decode("utf-8"))
            reaction = self._on_event(event)
            encoded = json.dumps(reaction).encode("utf-8")
            # Ownership: the Rust side takes this pointer via
            # `CString::from_raw` inside `quadra_runner_step` and frees it —
            # must hand out a raw malloc'd buffer, not a ctypes-owned one.
            buf = ctypes.create_string_buffer(encoded)
            size = len(encoded) + 1
            raw = libc.malloc(size)
            ctypes.memmove(raw, buf, size)
            return raw

        self._callback = HANDLE_CALLBACK(_trampoline)
        self._ptr = lib.quadra_runner_new(
            QuadraCallbacks(user_data=None, handle=self._callback)
        )

    def step(self) -> int:
        return lib.quadra_runner_step(self._ptr)

    def wake(self) -> None:
        lib.quadra_runner_wake(self._ptr)

    def close(self) -> None:
        if self._ptr:
            lib.quadra_runner_free(self._ptr)
            self._ptr = None


def main() -> None:
    with DescriptorStore() as store:
        row = store.insert("first row", 1.0)
        store.update(row, "first row (edited)", 2.0)
        assert len(store) == 1
        store.remove(row)
        assert len(store) == 0
        print("descriptor store round-trip: ok")

    events_seen = []

    def on_event(event: dict) -> dict:
        events_seen.append(event)
        if event["type"] == "woke":
            return {"type": "exit"}
        return {"type": "continue"}

    runner = Runner(on_event)
    try:
        assert runner.step() == STEP_CONTINUE
        runner.wake()
        assert runner.step() == STEP_EXITED
        # Idempotent after exit.
        assert runner.step() == STEP_EXITED
    finally:
        runner.close()

    assert [e["type"] for e in events_seen] == ["tick", "woke"]
    print("run/step/wake round-trip: ok", events_seen)


if __name__ == "__main__":
    main()
    sys.exit(0)
