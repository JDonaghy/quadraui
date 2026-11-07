#!/usr/bin/env python3
"""Interactive real-terminal demo of the `qui_rt_*` C ABI over stdlib `ctypes`.

This is the deliverable for the "can a foreign host drive an interactive
quadraui over the C ABI?" spike: it opens a real terminal (raw mode,
alternate screen — exactly what `TuiRunner::new` negotiates), builds a
`ListView` descriptor through `qui_rt_list_insert`, drives the event loop
through `qui_rt_pump`, and on a row activation (Enter, or a mouse click on a
row) mutates the descriptor in place through `qui_rt_list_update` so the next
repaint reflects the change. No Rust code runs on the Python side: every
observable behaviour here is reached purely through `extern "C"` functions
and a JSON wire format.

Requires:
- A real, attached terminal on stdin/stdout — this will fail to even
  construct the app (`qui_rt_app_new` returns NULL) under a non-interactive
  shell, CI runner, or an IDE's captured-output pane. That is a terminal
  constraint from the underlying `TuiRunner`, not something this script can
  work around.
- The quadraui-ffi cdylib already built: `cargo build -p quadraui-ffi` from
  the crate root (or anywhere in the workspace). No `pip install` of
  anything is required; this uses only the Python standard library.

Usage:
    cargo build -p quadraui-ffi
    python3 spike/list_demo.py

Controls once running: Up/Down to move the selection, Enter (or clicking a
row) to "activate" it — which appends a `*` marker to that row's text via an
in-place descriptor update — and q or Esc to quit.
"""

from __future__ import annotations

import ctypes
import json
import pathlib
import platform
import subprocess
import sys

# ─── Locate the built cdylib ────────────────────────────────────────────────

CRATE_DIR = pathlib.Path(__file__).resolve().parent.parent


def _shared_lib_name() -> str:
    system = platform.system()
    if system == "Windows":
        return "quadraui_ffi.dll"
    if system == "Darwin":
        return "libquadraui_ffi.dylib"
    return "libquadraui_ffi.so"


def find_library() -> pathlib.Path:
    """Locate the cdylib built from this crate.

    Checks, in order: an explicit `QUADRAUI_FFI_LIB` override, `cargo
    metadata`'s reported `target_directory` (works regardless of a custom
    `CARGO_TARGET_DIR` or workspace-level target dir), and finally the
    crate-relative `target/debug` a plain `cargo build` from this directory
    would produce.
    """
    override = sys.argv[1] if len(sys.argv) > 1 else None
    name = _shared_lib_name()
    candidates: list[pathlib.Path] = []

    if override:
        candidates.append(pathlib.Path(override))

    try:
        metadata = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=CRATE_DIR,
            capture_output=True,
            text=True,
            check=True,
        )
        target_dir = pathlib.Path(json.loads(metadata.stdout)["target_directory"])
        candidates.append(target_dir / "debug" / name)
    except (OSError, subprocess.CalledProcessError, KeyError, json.JSONDecodeError):
        pass

    candidates.append(CRATE_DIR / "target" / "debug" / name)

    for candidate in candidates:
        if candidate.is_file():
            return candidate

    tried = "\n  ".join(str(c) for c in candidates)
    raise SystemExit(
        f"could not find {name}; build it first with "
        f"`cargo build -p quadraui-ffi`, or pass an explicit path as "
        f"argv[1].\nTried:\n  {tried}"
    )


# ─── ctypes bindings for the qui_rt_* surface ───────────────────────────────

QUI_OK = 0
QUI_STEP_CONTINUE = 0
QUI_STEP_EXITED = 1

REACTION_CONTINUE = 0
REACTION_REDRAW = 1
REACTION_EXIT = 2

HandleCb = ctypes.CFUNCTYPE(
    ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t
)
TickCb = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p)


class QuiVTable(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("handle", HandleCb),
        ("tick", TickCb),
    ]


class QuiRtApp(ctypes.Structure):
    pass


def bind(lib: ctypes.CDLL) -> None:
    lib.qui_rt_app_new.argtypes = [QuiVTable]
    lib.qui_rt_app_new.restype = ctypes.POINTER(QuiRtApp)

    lib.qui_rt_app_free.argtypes = [ctypes.POINTER(QuiRtApp)]
    lib.qui_rt_app_free.restype = None

    lib.qui_rt_list_insert.argtypes = [
        ctypes.POINTER(QuiRtApp),
        ctypes.POINTER(ctypes.c_uint8),
        ctypes.c_size_t,
    ]
    lib.qui_rt_list_insert.restype = ctypes.c_int64

    lib.qui_rt_list_update.argtypes = [
        ctypes.POINTER(QuiRtApp),
        ctypes.c_int64,
        ctypes.POINTER(ctypes.c_uint8),
        ctypes.c_size_t,
    ]
    lib.qui_rt_list_update.restype = ctypes.c_int32

    lib.qui_rt_pump.argtypes = [ctypes.POINTER(QuiRtApp), ctypes.c_uint32]
    lib.qui_rt_pump.restype = ctypes.c_int32

    lib.qui_last_error_code.argtypes = []
    lib.qui_last_error_code.restype = ctypes.c_int32

    # `c_void_p`, not `c_char_p`: `c_char_p` makes ctypes eagerly copy the
    # string into Python bytes and discard the original pointer, so the
    # value handed to `qui_free` below would be a address ctypes invented,
    # not the one Rust's allocator produced — freeing it corrupts the heap.
    # Keeping the raw pointer lets us decode it ourselves and free that
    # exact address.
    lib.qui_last_error.argtypes = []
    lib.qui_last_error.restype = ctypes.c_void_p

    lib.qui_error_name.argtypes = [ctypes.c_int32]
    lib.qui_error_name.restype = ctypes.c_char_p

    lib.qui_free.argtypes = [ctypes.c_void_p]
    lib.qui_free.restype = None


def to_bytes(payload: dict | str) -> bytes:
    text = payload if isinstance(payload, str) else json.dumps(payload)
    return text.encode("utf-8")


def c_buf(data: bytes) -> ctypes.Array:
    return (ctypes.c_uint8 * len(data)).from_buffer_copy(data)


def report_last_error(lib: ctypes.CDLL, where: str) -> None:
    code = lib.qui_last_error_code()
    name_ptr = lib.qui_error_name(code)
    name = name_ptr.decode("utf-8") if name_ptr else f"<code {code}>"
    # `qui_last_error` is bound with `restype = c_void_p` (see `bind()`), so
    # `msg_ptr` is the raw address — decode it ourselves via a temporary
    # `c_char_p` cast, then free that same address, never a ctypes-invented
    # copy.
    msg_ptr = lib.qui_last_error()
    if msg_ptr:
        msg = ctypes.cast(msg_ptr, ctypes.c_char_p).value.decode("utf-8")
        lib.qui_free(msg_ptr)
    else:
        msg = "<no message>"
    print(f"{where}: {name}: {msg}", file=sys.stderr)


def row(text: str) -> dict:
    return {"text": {"spans": [{"text": text, "fg": None, "bg": None}]}}


def list_descriptor(labels: list[str], selected_idx: int) -> dict:
    # Every field below besides `id`/`items`/`selected_idx` has a
    # `#[serde(default)]` on the Rust side; they are spelled out here only
    # to show the full shape a host sends, not because they are required.
    return {
        "id": "spike:list",
        "items": [row(label) for label in labels],
        "selected_idx": selected_idx,
        "scroll_offset": 0,
        "has_focus": True,
        "bordered": True,
        "h_scroll": 0,
        "max_content_width": None,
        "show_v_scrollbar": True,
    }


def main() -> int:
    lib_path = find_library()
    lib = ctypes.CDLL(str(lib_path))
    bind(lib)

    labels = [f"Row {i}" for i in range(1, 6)]
    pending_activation: list[int] = []  # filled by on_handle, drained by main loop
    should_exit = [False]

    def on_handle(_user_data, event_json, event_len) -> int:
        raw = ctypes.string_at(event_json, event_len)
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            return REACTION_CONTINUE

        if isinstance(payload, dict) and payload.get("type") == "list_item_activated":
            # Recorded, not acted on here: mutating the descriptor via
            # qui_rt_list_update from inside this callback would reenter the
            # runner mid-dispatch. The main loop applies it once qui_rt_pump
            # has returned.
            pending_activation.append(payload["idx"])
            return REACTION_REDRAW

        key_pressed = payload.get("KeyPressed") if isinstance(payload, dict) else None
        if key_pressed is not None:
            key = key_pressed.get("key")
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                should_exit[0] = True
                return REACTION_EXIT

        return REACTION_CONTINUE

    def on_tick(_user_data) -> int:
        return REACTION_CONTINUE

    handle_cb = HandleCb(on_handle)
    tick_cb = TickCb(on_tick)
    vtable = QuiVTable(user_data=None, handle=handle_cb, tick=tick_cb)

    print(
        "quadraui-ffi qui_rt_* spike: Up/Down move the selection, Enter or a "
        "mouse click activates a row (marks it with ' *' via an in-place "
        "qui_rt_list_update), q or Esc quits.",
        file=sys.stderr,
    )

    app = lib.qui_rt_app_new(vtable)
    if not app:
        report_last_error(lib, "qui_rt_app_new")
        print(
            "qui_rt_app_new failed — this demo needs a real, attached "
            "terminal (it will not work piped, redirected, or under a "
            "non-interactive runner).",
            file=sys.stderr,
        )
        return 1

    try:
        initial = to_bytes(list_descriptor(labels, selected_idx=0))
        handle_id = lib.qui_rt_list_insert(app, c_buf(initial), len(initial))
        if handle_id < 0:
            report_last_error(lib, "qui_rt_list_insert")
            return 1

        while True:
            outcome = lib.qui_rt_pump(app, 100)
            if outcome == QUI_STEP_EXITED or should_exit[0]:
                break
            if outcome != QUI_STEP_CONTINUE:
                report_last_error(lib, "qui_rt_pump")
                return 1

            while pending_activation:
                idx = pending_activation.pop(0)
                if not 0 <= idx < len(labels):
                    continue
                if not labels[idx].endswith(" *"):
                    labels[idx] = labels[idx] + " *"
                updated = to_bytes(list_descriptor(labels, selected_idx=idx))
                code = lib.qui_rt_list_update(app, handle_id, c_buf(updated), len(updated))
                if code != QUI_OK:
                    report_last_error(lib, "qui_rt_list_update")
                    return 1
    finally:
        lib.qui_rt_app_free(app)

    print("exited cleanly")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
