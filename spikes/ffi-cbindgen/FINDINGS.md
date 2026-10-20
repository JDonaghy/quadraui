# Findings — hand-rolled `extern "C"` + cbindgen (spike for #1640)

Evidence for the #1096/#1123 "C ABI production method" decision. This
document is the cbindgen-side half; see `spikes/ffi-diplomat/NOTE.md` for
the Diplomat-side status. The actual written recommendation is tracked
separately (see the PR/issue discussion) since it weighs both sides
together — this file only records what building and hardening *this*
skeleton actually surfaced.

## What this skeleton covers

`src/lib.rs` implements the three surfaces #1640 asks a candidate ABI
method to prove out:

1. an opaque `u64`-handle store (`quadra_store_new/free/insert/update/remove/len`)
   wrapping `DescriptorStore`,
2. one callback in a vtable (`QuadraCallbacks { user_data, handle, free_reaction }`)
   carrying JSON event/reaction payloads across the boundary,
3. `run`/`step`/`wake` (`quadra_runner_new/free/step/run`, plus a detachable
   `QuadraWaker`) mapped onto the real `Reaction`/`StepOutcome` types.

`cbindgen` generates `include/quadraui_ffi_cbindgen_spike.h` from this on
every `cargo build` (see `build.rs`); `python/client.py` is a ctypes
client exercising all three surfaces against the real built `cdylib`.

## Traps found while building it

### 1. Cross-allocator ownership (fixed here via `free_reaction`)

The first-draft shape had `handle` return an owned `*mut c_char` that
Rust freed with `CString::from_raw`. That's only sound if the foreign
caller allocated the buffer with the *same* allocator Rust's global
allocator uses — true by accident for a same-process Rust-only test, false
for essentially every real foreign caller (Python's ctypes-managed
buffers, a C caller's own `malloc`, a GC'd runtime). `CString::from_raw`
on a pointer it didn't allocate is immediate UB.

Fix: split the single "return owned pointer" callback into a
borrow-then-free-back pair. `handle` returns a pointer Rust only reads via
`CStr::from_ptr` (never frees); `free_reaction(user_data, ptr)` is called
immediately after with that same pointer, so whichever side allocated the
buffer is also the side that frees it, by whatever means it likes. This
cost one more field on `QuadraCallbacks` and one more call per step, in
exchange for actually being correct across a real language boundary.
`python/client.py`'s `Runner` class demonstrates the consequence on the
client side: it no longer needs a libc-matching allocator at all — a
plain `dict[int, ctypes.Array]` keeping the buffer alive between the two
calls is enough.

**Lesson for #1123:** any real ABI design with an "allocate and hand me
the pointer" callback shape needs this split (or an equivalent, like a
caller-provided output buffer) from the start. It is not an edge case —
it is the default failure mode of FFI across two allocators.

### 2. A raw pointer wake races `&mut` (fixed here via a detachable `QuadraWaker`)

The first-draft `quadra_runner_wake(*const QuadraRunner)` let a second
thread materialize `&QuadraRunner` over the same allocation a `step`/`run`
call might be holding `&mut QuadraRunner` over — undefined behavior per
Rust's aliasing rules, regardless of what the function body actually
touches. This mirrors `Backend::waker()`'s own reason for existing:
`quadraui::backend::Backend::waker()` returns a detached
`Arc<dyn Fn + Send + Sync>` rather than letting wake code hold any
reference into the backend at all.

Fix: `quadra_runner_waker(*const QuadraRunner) -> *mut QuadraWaker` clones
an `Arc<AtomicBool>` out of the runner into a wholly separate allocation.
`quadra_waker_wake`/`quadra_waker_free` only ever touch that `QuadraWaker`
allocation — never the runner's. The two pointers share no memory, so a
concurrent wake and a concurrent step can never alias.

**Lesson for #1123:** any cross-thread control surface (wake, cancel,
interrupt) needs its own opaque handle, cloned out at creation time, not
a raw pointer to the primary handle. This is a general FFI-over-Rust
pattern, not specific to this runner shape.

### 3. Missing error channel — **left open, not fixed in this spike**

Every fallible operation here degrades to a sentinel: `insert` returns
`0`, `update`/`remove` return `bool`, and there is no way for `step`/`run`
to report an error at all (there is nothing to report — this skeleton's
`handle` callback can't fail, it can only choose `Exit`). Compare the real
`TuiRunner::step() -> io::Result<StepOutcome>` this mirrors: a real
backend step can fail (terminal I/O, GTK event loop errors), and `io::Result`
carries that detail. A `bool`/sentinel-`0` C ABI throws it away.

This is a real open design question for #1123, not something this
throwaway skeleton should paper over with an invented error-code enum —
the right shape (an out-parameter `QuadraError**`? a packed
`(tag, message_ptr)` struct? a thread-local "last error" the caller
polls, à la `errno`?) depends on what the real runner's fallible
operations turn out to be, which this spike doesn't have. Recorded here
as a question the real implementation must answer, not a gap this spike
leaves fixed.

### 4. `run`'s re-entrancy hazard — named in doc comments, restated here

`quadra_runner_run` is a thin `while step() == Continue {}` loop. Every
iteration calls back into foreign code via `callbacks.handle`. If that
foreign callback re-enters `quadra_runner_step` or `quadra_runner_run` on
the *same* runner pointer (plausible in any language with reentrant
callback-into-host patterns, e.g. a Python callback that itself drives
the same runner), it aliases `&mut QuadraRunner` — the same class of bug
as finding 2, but self-inflicted by a well-intentioned but incorrect
foreign caller rather than by this crate's own API shape. Nothing in the
C type signature prevents it; only the doc comment's `# Safety` section
says not to. A real #1123 implementation needs to either design this
hazard away (e.g. by making the runner's internals re-entrancy-tolerant,
or by detecting and rejecting re-entrant calls at the Rust boundary with
a runtime check rather than relying on caller discipline) or accept it as
a documented precondition the same way this spike does.

### 5. Process note: `build.rs` writing into a committed directory

This spike's `build.rs` writes the generated header straight into
`include/` inside the crate (not `$OUT_DIR`), so the header is visible in
`git status`/diffs and in a built sdist/tarball without a build step. For
a real shippable `quadraui-ffi` crate this is a real tradeoff to decide
deliberately (OUT_DIR keeps the header out of version control and
guarantees it's never stale, but makes `cargo doc`/packaging/"what does
the header for commit X look like" harder); it was a `build.rs` detail
convenient for *this* spike's purpose of letting `cargo build` regenerate
the header for the C-compiler validation below, not an endorsement either
way for #1123.

## Validation performed

- `cargo test` — all 7 tests pass, zero warnings (`store` round-trip,
  `insert` rejecting malformed JSON, `step`/`run` continue-until-exit
  and idempotent-after-exit, a genuinely falsifiable wake-observation
  test asserting the recorded event-type sequence, and a real
  cross-thread wake test that joins a spawned thread's `wake()` call
  before stepping on the main thread).
- The generated header was fed to a real C compiler, not just eyeballed:
  `gcc -std=c11 -fsyntax-only -Wall -Wextra -x c` and the same under
  `-std=c99`, both clean; plus a full compile (`-Wall -Wextra -Wpedantic
  -c`) of a throwaway usage file that double-`#include`s the header
  (include-guard check) and calls every declared function with matching
  real callback signatures.
- `python/client.py` round-trips end-to-end against the real built
  `cdylib` (`cargo build --release` then `python3 python/client.py`):
  descriptor store insert/update/remove/len, and a `step`→`wake`→`step`
  sequence observing `["tick", "woke"]` in order — the cross-language
  proof that the `free_reaction` ownership split and the `QuadraWaker`
  split both work from a genuinely foreign caller, not just from Rust
  unit tests calling Rust.
