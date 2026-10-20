# Recommendation — C ABI production method for #1096/#1123 (#1640)

**Recommendation: hand-rolled `extern "C"` + cbindgen.** Build the real
`quadraui-ffi` crate the way `spikes/ffi-cbindgen/` does — plain
`#[no_mangle] pub extern "C" fn` definitions, opaque `*mut` handles, a
hand-written `#[repr(C)]` callback vtable, cbindgen generating the C
header from that source — not Diplomat's `#[diplomat::bridge]` dialect.

This is a close call on ergonomics and not close at all on evidence:
cbindgen's side of this spike has a generated header fed to a real C
compiler and a ctypes client round-tripping against the real built
`cdylib`; Diplomat's side has a Rust source skeleton and Rust-only
tests, with `diplomat-tool` never invoked (see
`spikes/ffi-diplomat/NOTE.md`). That asymmetry is itself a data point,
not just a scheduling artifact — see "Why the evidence gap matters"
below.

## What both skeletons prove, identically

Both spikes implement the same three surfaces over the same real
quadraui types (`DescriptorStore`/`Handle`, `Reaction`/`StepOutcome`),
and arrive at the *identical* aliasing-safety design in two places that
have nothing to do with which codegen tool is in use:

- **The cross-allocator ownership trap.** cbindgen's spike hit this with
  a callback that returns an owned pointer (`handle` / `free_reaction`
  split, `FINDINGS.md` finding 1). Diplomat's `DiplomatWrite` output
  shape sidesteps *this specific* trap by construction (see "Where
  Diplomat genuinely wins" below) — but that's a property of choosing a
  string-return method shape, not of Diplomat analyzing or preventing
  the trap generally. A Diplomat callback that still wanted to hand back
  an owned, non-string resource would hit the identical problem.
- **The `wake`-vs-`step` aliasing hazard.** Both spikes independently
  needed a `QuadraWaker` that clones an `Arc<AtomicBool>` out of the
  runner into a wholly separate allocation, rather than exposing a raw
  pointer/opaque handle to the runner itself to cross-thread wake code.
  Diplomat's bridge macro mirrors whatever `&self`/`&mut self` method
  shapes the Rust source already has into the generated bindings — it
  does not detect or warn about a naively-shaped `wake(&self)` aliasing
  `step(&mut self)` on the same opaque type. This is recorded in
  `ffi-diplomat/src/lib.rs`'s doc comment on `QuadraWaker` as that
  spike's single most important finding: **the bridge macro generates
  glue, not a safety analysis.**

Conclusion from this pair: **the hardest work in building `quadraui-ffi`
— getting the aliasing and ownership story right across a real language
boundary — is unaffected by which of these two tools does the
C-signature bookkeeping.** Neither tool buys safety; both only differ in
how much boilerplate they save around a design the author still has to
get right by hand.

## Where Diplomat genuinely wins

Two real ergonomic advantages showed up, and should be weighed honestly
even though the recommendation goes the other way:

1. **Callbacks are ordinary Rust closures.** `ffi-diplomat`'s
   `QuadraRunner::step(&mut self, on_event: impl Fn(u64, bool) -> bool)`
   needed no hand-written `#[repr(C)]` vtable struct, no `user_data`
   pointer threading, no separate free-function slot. `ffi-cbindgen`'s
   equivalent (`QuadraCallbacks { user_data, handle, free_reaction }`)
   is exactly the boilerplate Diplomat's codegen removes. For a
   `quadraui-ffi` surface with many callback-shaped methods (and the
   real runner has several: event dispatch, backend hooks, possibly
   render callbacks down the line), this compounds — every new callback
   is one line in the Diplomat dialect vs. a new vtable field plus
   marshaling code by hand.
2. **`DiplomatStr`/`DiplomatWrite` remove a specific, real trap by
   construction.** `label_at(&self, id: u64, write: &mut DiplomatWrite)`
   never hands back an owned pointer across the allocator boundary at
   all — the generated binding allocates the target-language string
   directly from what `write!` writes into the buffer. `ffi-cbindgen`'s
   `free_reaction` split (`FINDINGS.md` finding 1) exists *only* because
   that spike's callback shape returns an owned pointer in the first
   place; a Diplomat string-returning method in this shape has no
   equivalent trap to fall into.

## Why the recommendation goes to cbindgen anyway

1. **The friction Diplomat removes is already solved, cheaply, in the
   cbindgen spike.** The `free_reaction` split costs one extra vtable
   field and one extra call per callback invocation — a known, bounded,
   one-time cost paid once in `quadraui-ffi`'s design, not a recurring
   tax. Diplomat's advantage here is real but marginal once that pattern
   exists.
2. **`DiplomatWrite` has a real, asymmetric cost of its own.** It has no
   safe/ergonomic Rust-side constructor — building one in a plain Rust
   unit test requires going through `diplomat_buffer_write_create`/
   `diplomat_buffer_write_destroy` inside an `unsafe` block (see the
   comment on `store_insert_update_remove_round_trips` in
   `ffi-diplomat/src/lib.rs`). `ffi-cbindgen`'s plain `*const c_char` +
   `CStr::from_ptr` pattern needs no such dance from a Rust-only test.
   This is a genuine net *increase* in Rust-side testing friction
   exactly where Diplomat is supposed to be making things easier — the
   type that's effortless to consume from the *generated* binding is
   awkward to exercise from Rust itself.
3. **cbindgen's output is a plain C header — directly inspectable,
   directly reviewable, compiler-checkable without any extra tooling.**
   `ffi-cbindgen`'s validation (`gcc -fsyntax-only` against the generated
   header under two C standards, a real compiled usage file, a ctypes
   client round-tripping against the real `cdylib`) was all possible
   with tools already on hand. Diplomat's generated bindings (Python via
   nanobind, TypeScript, ...) require `diplomat-tool` as a separate,
   unfamiliar piece of toolchain the team doesn't yet have experience
   operating, debugging, or vendoring. That's not a blocker, but it's a
   real unknown-cost item `#1123` would be importing, vs. cbindgen's
   already-proven, already-boring toolchain.
4. **The evidence gap is itself informative, not just a scheduling
   accident.** cbindgen's `build.rs` hook meant "stand up the skeleton
   and get a generated, compiler-checked header" was a `cargo build`
   away. Diplomat's codegen is a separate CLI invocation outside the
   normal `cargo build`/`cargo test` loop (see `ffi-diplomat/NOTE.md`),
   which is exactly why this spike's Diplomat half stopped at the Rust
   source layer within its time budget. That asymmetry previews a real
   operational difference `#1123` will live with permanently: cbindgen's
   codegen is a transparent, zero-extra-step part of the existing build;
   Diplomat's is an additional tool in the release/CI pipeline. A
   process that's one command simpler to validate today is also one
   command simpler to maintain for the life of `quadraui-ffi`.
5. **`#[non_exhaustive]` and opaque-type module-privacy both needed the
   same hand-reasoning on both sides.** Mapping `Reaction`/`StepOutcome`
   (both `#[non_exhaustive]`) onto FFI-safe enums needed an identical
   manual "unknown variant → fail toward Continue" fallback in both
   spikes' `step_outcome_to_ffi`. Diplomat's `#[diplomat::opaque]`
   pattern additionally surfaces a minor but real wrinkle `ffi-cbindgen`
   doesn't have: a private field inside `mod ffi` isn't visible to a
   sibling `mod tests` at the crate root the way it would be to a plain
   `extern "C"` function defined anywhere in the crate, which shapes
   where whitebox tests can live relative to the bridge module. Neither
   side gets a pass on manual reasoning; cbindgen's is just slightly
   less structurally constrained.

## What this does not resolve for #1123

- No error-reporting channel was designed on either side (`FINDINGS.md`
  finding 3) — both skeletons degrade fallible operations to a sentinel
  or `bool`. The real `quadraui-ffi` crate still needs to answer this,
  independent of which codegen tool is chosen.
- `run`'s callback-reentrancy hazard (`FINDINGS.md` finding 4) is
  unaddressed on both sides and is not a codegen-tool question at all.
- Diplomat's generated-artifact story (actual C header content, actual
  Python/nanobind bindings, actual JS output) was never produced in
  this spike. If a future session wants to re-open this recommendation
  with firmer footing, running `diplomat-tool` against
  `ffi-diplomat/src/lib.rs` and writing a matching Python client is the
  concrete next step — see `spikes/ffi-diplomat/NOTE.md`.

## Evidence

- `spikes/ffi-cbindgen/FINDINGS.md` + `spikes/ffi-cbindgen/python/client.py`
  — generated header, C-compiler validation, full Python round-trip.
- `spikes/ffi-diplomat/src/lib.rs` + `spikes/ffi-diplomat/NOTE.md` —
  Rust-source skeleton, 5 passing tests, no generated bindings.
