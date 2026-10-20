# Diplomat spike — status (#1640)

This directory holds the Diplomat-generated half of the #1640 spike.
`src/lib.rs` defines the same three surfaces `spikes/ffi-cbindgen/`
proves out — an opaque `u64`-handle store, a callback crossing the FFI
boundary, and `run`/`step`/`wake` over the real
`quadraui::runner::{Reaction, StepOutcome}` types — inside a
`#[diplomat::bridge] pub mod ffi { ... }` module, so the two spikes can
be judged against identical Rust-side shapes.

`cargo build`/`cargo test` both pass clean (zero warnings, `cargo
clippy` clean, `cargo fmt --check` clean): 5 tests covering the
descriptor-store round trip (including the `DiplomatWrite`-backed string
return and an invalid-UTF-8 rejection), step/run continuing until the
callback signals exit, and a genuine cross-thread wake test.

## What was not done in this iteration

**`diplomat-tool` was never invoked.** Diplomat's actual code generation
is a separate CLI (unlike cbindgen's `build.rs` hook) that reads
`#[diplomat::bridge]`-annotated source and emits per-language bindings
(C header, Python via nanobind, TypeScript, ...). This spike only proves
out what the *Rust source* looks like and compiles/tests the Rust side
in isolation; it does not produce a generated C header or a Python
client analogous to `spikes/ffi-cbindgen/python/client.py`, so there is
no cross-language round-trip evidence on this side the way there is for
cbindgen. Running `diplomat-tool` against this module and writing a
matching Python client (nanobind-based, not ctypes — Diplomat's Python
backend doesn't produce a raw cdylib consumable by ctypes the way
cbindgen's plain C header does) is the natural next step for anyone
picking this back up, but it is out of scope for what this iteration's
time budget covers.

## What building the Rust side alone already shows

See `spikes/ffi-diplomat/src/lib.rs`'s module doc comment and inline
comments for the detailed findings (callback-as-`impl Fn` eliminating
the hand-written vtable, `DiplomatWrite` having no ergonomic
Rust-side constructor, aliasing-safety being entirely unaffected by
which codegen tool is in use). The written recommendation weighing both
spikes together lives in `spikes/ffi-cbindgen/RECOMMENDATION.md`.
