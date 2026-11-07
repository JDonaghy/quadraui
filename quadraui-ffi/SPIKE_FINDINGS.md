# Spike: can a foreign host drive an interactive quadraui over the C ABI?

Time-boxed spike for #1887. The question: should every future language
binding be layered on `quadraui-ffi`'s C ABI (and `quadraui-py`
re-platformed onto it), or should bindings keep binding `quadraui`
directly? This spike proves or disproves the ABI is *adequate* for
interactivity — it does not re-platform anything.

## Verdict: **Go**

A foreign host, talking only `extern "C"` functions and a JSON wire
format (no Rust, no `cdylib`-internal knowledge), can run a real
`TuiRunner`-backed interactive loop, build an interactive primitive,
receive a **hit-tested** semantic event for both a keyboard activation and
a mouse click, and update the descriptor in place so the next repaint
reflects it. Nothing in the slice below needed a shape the ABI couldn't
already express; the one real bug found was a foreign-host integration
mistake (see below), not a gap in the ABI's design.

## What was built

The narrowest vertical slice proving the hypothesis for one interactive
primitive (`ListView`), additive to `quadraui-ffi`:

1. **Real runner over the C ABI** — `qui_rt_app_new`/`qui_rt_pump`/
   `qui_rt_app_free` wrap the real `TuiRunner`, not a headless stub.
2. **Handle-addressed descriptor + in-place update** — `qui_rt_list_insert`
   returns an opaque `i64` handle; `qui_rt_list_update` mutates that same
   list's descriptor in the store between pump calls.
3. **Hit-tested semantic event out** — a click or keyboard activation
   reaches the host's `handle` vtable callback as `{"type":
   "list_item_activated", "handle": <id>, "idx": <n>}`, not a raw
   `MouseDown`/`KeyPressed`.
4. `spike/list_demo.py` — a stdlib-only (`ctypes`, no `pip install`)
   Python demo exercising all three end to end in a real terminal.

Stretch goal (`qui_wake` + opaque `UiEvent::User` wire form) was not
attempted — the time-box was spent on 1–3 plus closing an evidence gap
(below), and the stretch goal doesn't bear on the go/no-go call.

## Evidence

### The hit-tested path is two paths, not one

Reading `quadraui-ffi/src/lib.rs`'s `FfiRtApp` impl shows the "hit-tested
semantic event" requirement is satisfied by two distinct code paths that
emit the same payload shape:

- `handle_mouse_down` does **real geometric hit-testing**: it derives the
  same rect `render` paints into from `backend.viewport()`, looks up the
  active list's handle, maps the click position into rect-local
  coordinates, and calls `backend.list_layout(rect, list).hit_test(local_x,
  local_y)` — the same `Backend` method `draw_list` itself resolves
  through, so paint and hit-test can't drift apart. Only a hit on
  `ListViewHit::Item(idx)` emits `list_item_activated`; a hit on the
  title/border/empty area falls back to `None` (the host gets the raw
  `MouseDown` instead).
- `handle_key`'s `Enter` branch is a convenience path with no
  hit-testing: it just reads the already-known `selected_idx` out of the
  store and emits the identical payload. `Up`/`Down` mutate
  `selected_idx` directly and return `Redraw` without a round trip to the
  host at all.

Both needed to be exercised separately to actually prove the issue's
literal claim, since source-reading only confirms the mouse path *does*
hit-test — it doesn't prove the wire format and the Python-side
handling of it are correct end to end.

### Automated proof, via a pty-emulation harness (not this repo)

`qui_rt_app_new` refuses to construct outside a real, attached terminal
(by design — that's `TuiRunner::new`'s own constraint, not an ABI gap),
which makes this hard to verify in a sandboxed/headless session with no
human operator. A throwaway harness (outside this repo, not part of the
deliverable) opened a real pty via Python's `pty.fork()`, answered
crossterm's terminal-capability probes (cursor position `ESC[6n`,
Primary Device Attributes `ESC[c`, DECRQM for SGR-pixel mouse mode
`ESC[?1016$p`), and set a non-zero window size via `TIOCSWINSZ` — a bare
pty defaults to 0×0 and answers none of these, which is exactly what
blocks `TuiRunner::new` from ever finishing setup.

With that harness driving `spike/list_demo.py`, sending Down, Enter, and
then an SGR mouse-click escape sequence (`ESC[<0;3;4M`) at a different
screen position:

```
Row 1
Row 2
Row 2 *      <- keyboard path: Down (idx 0->1) + Enter activates "Row 2"
Row 3
Row 3 *      <- mouse path: click hit-tests to a *different* row, "Row 3"
Row 4
Row 5
...
exited cleanly
```

Both markers appear, on two *different* rows, from two *different* input
mechanisms — real evidence that `handle_mouse_down`'s geometric hit-test
resolved the click's screen coordinates to the correct item independent
of whatever the keyboard had last selected, and that the resulting
semantic event round-tripped correctly through the demo's `ctypes`
callback, its `pending_activation` buffer, and back out through
`qui_rt_list_update` into a visible repaint.

This is strong automated evidence, not a substitute for a human running
`spike/list_demo.py` in a real terminal — see `SMOKE_TESTS` in this PR.

## A real integration hazard found (and fixed)

The first version of `spike/list_demo.py` crashed with
`munmap_chunk(): invalid pointer` (SIGABRT) the first time it hit an
error path. Root cause: `qui_last_error` was bound with
`restype = ctypes.c_char_p`, which makes `ctypes` eagerly copy the
returned string into an immutable Python `bytes` object *and discard the
original pointer*. Passing that `bytes` value into `qui_free`
(`argtypes = [ctypes.c_char_p]`) made `ctypes` marshal a **brand-new**
temporary buffer and hand *that* buffer's address to `qui_free` — not the
address Rust's allocator produced. Freeing an address the allocator never
returned is undefined behaviour; glibc caught it as heap corruption.

Fix: bind `qui_last_error`'s `restype` (and `qui_free`'s `argtypes`) as
`ctypes.c_void_p` — the raw, untouched address — and decode the string on
demand via `ctypes.cast(ptr, ctypes.c_char_p).value`, freeing that exact
same pointer afterwards. `qui_error_name`, which returns a static string
nothing ever frees, is unaffected and stays `c_char_p`.

This isn't an ABI defect — `qui_last_error`'s contract (`char*`, caller
frees via `qui_free`) is correct and unambiguous. It's a generic `ctypes`
pitfall any binding author hits the first time they wire up a "read a
string, then free it" pair, worth documenting here because every future
C-ABI-based binding will hit it once.

## P0 gaps for whoever re-platforms on this later

Not blockers for this spike's go/no-go, but real limits worth knowing
before building more bindings on this surface:

- **No hit-zone registry; only the single active list is hit-tested.**
  `handle_mouse_down` hit-tests against one `self.active` handle and one
  `self.last_rect`. A real app with multiple interactive primitives on
  screen at once needs the ABI to grow a way to register/route hits
  across more than one widget — today's spike proves the mechanism works
  for one widget, not that it scales to many.
- **No handle removal or reuse.** `handles: Vec<Handle<ListView>>` only
  grows; there's no `qui_rt_list_remove`. `DescriptorStore::remove`
  already exists (`quadraui/src/descriptor_store.rs:217`), so the store
  itself isn't the gap — it's the missing ABI wrapper and the `handles`
  index that would need a tombstone instead of a bare `Vec`. Fine for a
  spike, not fine for a long-running host that creates and destroys
  widgets.
- **The semantic-event design is additive, not a replacement.**
  `handle_mouse_down`/`handle_key` return `None` for anything they don't
  specifically recognize, and the raw event still reaches the host in
  that case. This is a deliberate, good property (nothing is silently
  swallowed) but it means a host integrating more primitives needs to
  keep handling the raw channel too, not just the semantic one.
- **`TuiRunner::new` truly requires a real attached terminal.** There is
  no documented ABI-level way to run the loop against a non-TTY test
  double; every foreign-host integration test will need the same kind of
  pty-emulation workaround used to gather the evidence above (or a real
  human in a real terminal, per `SMOKE_TESTS`).
- **`qui_rt_list_insert`/`qui_rt_list_update` always request a full
  repaint**, not a targeted one — `request_full_repaint()` is documented
  as a last-resort escape hatch, not a per-mutation default. It is
  correct for this spike (one list, one screen) but a host with several
  independent widgets would redraw everything on every single mutation;
  a real binding needs a narrower "this descriptor changed" signal.
- **`handle_key` ignores `Modifiers`.** It pattern-matches on the key
  alone, so e.g. Shift+Enter or Ctrl+Up behave identically to their
  unmodified counterparts — fine for this spike's controls, but a gap
  for any primitive that wants modifier-sensitive key handling over the
  ABI.

## Why this supports "go"

Every piece the issue asked this spike to prove — real runner, in-place
update, hit-tested semantic event — worked exactly as the ABI's existing
design promised, for both of the input mechanisms the primitive exposes,
with zero changes needed to `quadraui`'s public API and only an additive
change to `quadraui-ffi`. The one real bug found was a `ctypes`
ownership mistake on the Python side, not a flaw in the ABI's contract —
and it's exactly the kind of mistake worth documenting once here so the
next binding author doesn't repeat it. Nothing encountered in this slice
suggests the C ABI is inadequate as a foundation for interactive
bindings.
