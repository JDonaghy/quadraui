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


def _lib_name() -> str:
    """cdylib filename for the current platform. The crate's own `Cargo.toml`
    produces `lib<name>.so`/`.dylib`/`<name>.dll` depending on the host, not
    just the Linux `.so` this client originally hardcoded."""
    base = "quadraui_ffi_cbindgen_spike"
    if sys.platform == "darwin":
        return f"lib{base}.dylib"
    if sys.platform == "win32":
        return f"{base}.dll"
    return f"lib{base}.so"


LIB_NAME = _lib_name()


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
# `free_reaction`: called once per non-null `handle` return, with the same
# pointer, so whichever side allocated the reaction buffer also frees it.
# Rust only ever borrows it (`CStr::from_ptr`) in between — it never calls
# `free`/`CString::from_raw` on a pointer this client produced, so the
# buffer can be ordinary ctypes-managed Python memory instead of needing to
# come from libc's `malloc` to match Rust's allocator.
FREE_CALLBACK = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)


class QuadraCallbacks(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("handle", HANDLE_CALLBACK),
        ("free_reaction", FREE_CALLBACK),
    ]


lib.quadra_runner_new.argtypes = [QuadraCallbacks]
lib.quadra_runner_new.restype = ctypes.c_void_p
lib.quadra_runner_free.argtypes = [ctypes.c_void_p]
lib.quadra_runner_step.argtypes = [ctypes.c_void_p]
lib.quadra_runner_step.restype = ctypes.c_uint32
lib.quadra_runner_run.argtypes = [ctypes.c_void_p]
lib.quadra_runner_waker.argtypes = [ctypes.c_void_p]
lib.quadra_runner_waker.restype = ctypes.c_void_p
lib.quadra_waker_wake.argtypes = [ctypes.c_void_p]
lib.quadra_waker_free.argtypes = [ctypes.c_void_p]

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
        # Keyed by the buffer's ctypes address: `handle` hands Rust a
        # pointer it only *borrows* (it calls `free_reaction` with the same
        # pointer once it's done reading), so this dict is what actually
        # keeps the Python-owned buffer alive in between those two calls.
        self._pending_reactions: dict[int, ctypes.Array] = {}

        def _trampoline(_user_data: int, event_json: bytes) -> int:
            event = json.loads(event_json.decode("utf-8"))
            reaction = self._on_event(event)
            encoded = json.dumps(reaction).encode("utf-8")
            buf = ctypes.create_string_buffer(encoded)
            addr = ctypes.addressof(buf)
            self._pending_reactions[addr] = buf
            return addr

        def _free_reaction(_user_data: int, reaction_json: int) -> None:
            self._pending_reactions.pop(reaction_json, None)

        self._callback = HANDLE_CALLBACK(_trampoline)
        self._free_callback = FREE_CALLBACK(_free_reaction)
        self._ptr = lib.quadra_runner_new(
            QuadraCallbacks(
                user_data=None,
                handle=self._callback,
                free_reaction=self._free_callback,
            )
        )

    def step(self) -> int:
        return lib.quadra_runner_step(self._ptr)

    def run(self) -> None:
        lib.quadra_runner_run(self._ptr)

    def waker(self) -> "Waker":
        return Waker(lib.quadra_runner_waker(self._ptr))

    def close(self) -> None:
        if self._ptr:
            lib.quadra_runner_free(self._ptr)
            self._ptr = None


class Waker:
    """Detachable wake handle cloned out of a `Runner`. Safe to hand to
    another thread and call `wake()` from there — see
    `quadra_runner_waker`'s doc comment in the generated header for why
    this is a separate handle rather than a raw `Runner` pointer."""

    def __init__(self, ptr: int) -> None:
        self._ptr = ptr

    def wake(self) -> None:
        lib.quadra_waker_wake(self._ptr)

    def close(self) -> None:
        if self._ptr:
            lib.quadra_waker_free(self._ptr)
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
    waker = runner.waker()
    try:
        assert runner.step() == STEP_CONTINUE
        waker.wake()
        assert runner.step() == STEP_EXITED
        # Idempotent after exit.
        assert runner.step() == STEP_EXITED
    finally:
        waker.close()
        runner.close()

    assert [e["type"] for e in events_seen] == ["tick", "woke"]
    print("run/step/wake round-trip: ok", events_seen)


if __name__ == "__main__":
    main()
