# quadraui-ffi: C ABI toolchain evaluation + PoC

This crate is phase 1 of #1123: decide which Rust→C toolchain quadraui's
eventual C ABI should be built on, and prove the decision on a minimal,
real slice of the library (not a toy struct). It has two deliverables:

1. **This document** — the toolchain evaluation and the call on how this
   relates to #118 (`quadraui-ipc`).
2. **The PoC** — `src/lib.rs`, `build.rs`, `include/quadraui_ffi.h`,
   `smoke/`: a cbindgen-generated C header over one primitive
   ([`quadraui::Button`]) round-tripped through quadraui's headless `tui`
   backend.

## What the ABI has to carry

Before comparing toolchains it's worth being concrete about what quadraui's
surface actually needs to cross the boundary, because that's what rules
tools in or out:

- **Descriptor types.** Primitives are `#[non_exhaustive]` enums and
  builder-style structs (`Button`, `Rect`, `Modifiers`, …) with public
  fields added over time. A C ABI either needs its generator to tolerate
  that evolution without a recompile-everything break, or it needs the
  descriptor to cross as an opaque, versioned blob (JSON, in this PoC)
  rather than a field-for-field C struct.
- **Vtable callbacks.** A host registers `handle`/`tick` (and, for a future
  real backend, input-source) function pointers plus a `user_data` it
  owns. The generator needs to produce (or let us hand-write) a clean
  `#[repr(C)]` struct of `Option<unsafe extern "C" fn(...)>` fields —
  this is squarely "C ABI" territory, not "expose a Rust trait object."
- **A non-blocking step/wake runner.** quadraui's event loop is
  `handle` → `tick` → `render`, driven one step at a time; nothing here
  blocks waiting on I/O inside the library. The ABI needs a `step()` a
  host can call from its own loop (a GTK main loop, a game engine tick, a
  Python `asyncio` loop) — not a `run()` that takes over the thread.
- **String ownership.** Every string crossing the boundary (error
  messages, serialized descriptors) needs an explicit, unambiguous owner
  and free function. Silent lifetime assumptions are the single easiest
  way to turn a C ABI into a use-after-free.
- **Error propagation.** No exceptions, no `Result<T, E>` — `int` return
  codes plus a way to fetch human-readable detail after the fact
  (`qui_last_error` in this PoC).

## The three candidates

### cbindgen

Generates a **C header from existing `#[repr(C)]`/`extern "C"` Rust**. It
does not generate any Rust — you still hand-write every `#[no_mangle]
pub extern "C" fn`, every opaque handle, every vtable struct, by hand, in
ordinary Rust. cbindgen's only job is turning what you already wrote into
a header a C (or C++, or anything that links C) caller can `#include`.

- **Descriptor types:** doesn't help or hurt — descriptors stay JSON (or
  any serialization) carried as `(*const u8, usize)`, which is exactly the
  shape that tolerates `#[non_exhaustive]` growing new fields without
  touching the ABI. cbindgen only sees the opaque `QuiApp` handle and the
  vtable struct, both of which are stable by construction.
- **Vtable callbacks:** trivial — cbindgen mirrors a `#[repr(C)]` struct
  of `Option<unsafe extern "C" fn(...)>` fields into the equivalent C
  struct-of-function-pointers almost verbatim. This is cbindgen's best
  case.
- **Non-blocking step/wake runner:** no help needed — `qui_step`/`qui_run`
  are ordinary `extern "C" fn`s; cbindgen just declares them.
- **String ownership:** no built-in convention; the crate author designs
  and documents it (`qui_last_error` returns a borrowed pointer valid
  until the next call on that handle or `qui_free`; `qui_free` is the one
  function that frees a `CString::into_raw` pointer). cbindgen does not
  generate any of this — it is hand-written Rust, same as any other
  `extern "C" fn`.
- **Error propagation:** same story — `int` codes are just the return type
  cbindgen sees.
- **Cost:** lowest of the three. One `build-dependencies` entry, no code
  generation beyond a header, no new IDL to learn, no runtime. The crate
  is normal, auditable Rust from top to bottom; a reviewer who knows Rust
  and C already knows 100% of what's going on. The cost moves entirely
  into **hand-writing and hand-reviewing the unsafe boundary** (the null
  checks, the `catch_unwind` placement, the ownership contracts this PoC's
  review round actually caught). cbindgen gives you no guardrails against
  getting that wrong — it only formats what you already decided.

### Diplomat

A Rust **attribute-macro + codegen tool** (`#[diplomat::bridge]`) that
generates idiomatic bindings for C, C++, JS/WASM, Dart, and others from a
restricted dialect of Rust (no generics in the bridged surface, specific
patterns for references/boxes/slices/strings). It is actively used by
ICU4X and is explicitly aimed at crates that need **many** target
languages from one annotated surface.

- **Descriptor types:** Diplomat's model wants data to cross as typed
  structs/opaque boxes it understands, not as an opaque blob a consumer
  must `memcpy`+parse. Carrying descriptors as JSON `(*const u8, usize)`
  (as this PoC does) sidesteps Diplomat's structured-type machinery
  entirely, so most of its value — generating idiomatic per-language
  wrappers for every field — would go unused for quadraui's
  `#[non_exhaustive]`, still-evolving primitive structs. Diplomat would
  only start to pay off once the descriptor types are considered frozen
  enough to hand-model per field, which isn't true yet.
- **Vtable callbacks:** no first-class support for "host registers a
  callback struct" in the bridged dialect; Diplomat's examples are
  call-into-Rust, not call-back-into-host. This would have to be
  hand-rolled in raw `extern "C"` anyway, alongside the generated surface
  — i.e. the exact thing this PoC already proves out with cbindgen, just
  with a second tool bolted on for the rest of the API.
- **Non-blocking step/wake runner:** no issue — ordinary functions on the
  bridge are fine.
- **String ownership:** this is Diplomat's strongest feature — it
  generates the ownership-safe wrapper (owned `DiplomatStr`,
  slice-with-length, free-on-drop) for every target language
  automatically. If quadraui ever ships bindings in several languages at
  once, this is the biggest argument for Diplomat.
- **Error propagation:** supports `Result`-shaped returns across the
  bridge directly — nicer than hand-rolled `int` + `qui_last_error`, but
  only for the part of the surface that goes through the bridge macro.
- **Cost:** meaningfully higher than cbindgen for a project at quadraui's
  current single-target-language (C) stage: a new macro-driven dialect to
  learn, a second build step (`diplomat-tool` runs outside `cargo build`
  in the common setup), and a restricted subset of Rust for anything
  Diplomat has to see. That cost buys multi-language idiomatic bindings
  quadraui doesn't need yet — phase 1 only needs a plain C ABI.

### UniFFI

Mozilla's **IDL-first** binding generator: you write a `.udl` file (or use
proc-macros) describing the interface, and UniFFI generates Kotlin,
Swift, Python, Ruby bindings (notably **no first-class C target** —
UniFFI's scaffolding is Rust-to-{Kotlin,Swift,Python,Ruby}, not Rust-to-C;
getting a C header out of it means going through its C-compatible FFI
layer informally, which is not what UniFFI is built or supported for).

- **Descriptor types:** UniFFI models records/enums/interfaces from the
  IDL and is good at evolving them with its own versioning rules, but
  again only pays off once the type is "frozen enough" to describe in
  UDL — same mismatch as Diplomat for quadraui's still-growing
  `#[non_exhaustive]` primitives.
- **Vtable callbacks:** UniFFI has "callback interfaces" (a Rust trait a
  foreign side implements), which is closer to what quadraui needs than
  Diplomat's model — but it's designed around the foreign side
  implementing a trait object, not a C struct of raw function pointers
  plus a `void* user_data`, which is the actual shape a C host expects.
- **Non-blocking step/wake runner:** fine, no obstacle.
- **String ownership:** handled automatically for its supported target
  languages, but C is not one of them — no benefit here for *this*
  phase's goal (a C ABI).
- **Error propagation:** first-class (`Result<T, UniffiError>` crosses
  the bridge directly) — again only for Kotlin/Swift/Python/Ruby
  consumers, not a plain C caller.
- **Cost:** the wrong tool for this phase on a target-language basis
  alone. UniFFI's entire value proposition is "skip hand-writing FFI for
  these specific managed-language targets"; quadraui's phase-1 ask is a
  plain C ABI, which UniFFI doesn't target as a first-class output.

## Recommendation

**cbindgen**, for phase 1 (and likely beyond, until/unless quadraui
commits to shipping idiomatic bindings in several managed languages at
once).

- It is the only one of the three built to answer the actual question
  asked: "generate a C header from a Rust `extern "C"` surface." Diplomat
  and UniFFI are both "generate an entire multi-language binding layer
  from an annotated/IDL'd surface" tools — strictly more than quadraui
  needs today, at a real cost in new build steps, new dialects, and
  (for UniFFI) no C target at all.
- quadraui's primitives are still evolving (`#[non_exhaustive]`, new
  fields landing regularly per `CHANGELOG.md`). Carrying them across the
  ABI as versioned JSON — which this PoC does — sidesteps the exact
  per-field codegen both Diplomat and UniFFI are built around, so neither
  tool's main selling point applies yet. Revisit Diplomat specifically if
  and when quadraui commits to shipping native bindings in multiple
  managed languages (not just "a C header"); that is the point at which
  its ownership-safe generated wrappers start paying for themselves.
- The real complexity in this PoC — the `catch_unwind` boundary, null
  checks on every raw pointer, the FIFO event queue contract, the
  callee-allocates-caller-frees string convention — is **unsafe Rust the
  crate author has to get right by hand regardless of which of the three
  tools is chosen**, because none of them generate or verify that layer.
  Choosing cbindgen doesn't remove that cost; it just avoids *adding* a
  second, heavier toolchain on top of it for no corresponding benefit at
  this phase.

## The call on #118 (`quadraui-ipc`)

**Diverge — these solve different problems and both stay.**

#118 is tracked as "a language-agnostic JSON bridge," complementary to
#115 (`quadraui-lua`, an in-process Lua bridge): the two are described
as an **out-of-process JSON/stdio bridge** (#118) versus an **in-process
scripting bridge** (#115). `quadraui-ffi` is neither of those — it is an
**in-process C ABI**: a host process links `quadraui-ffi` as a static or
dynamic library and calls into it directly across a function-call
boundary, in the same address space, with no serialization-per-message
transport in between (the one place JSON appears in this PoC is for
carrying a single *descriptor* across the handle, not as the transport
for the whole interaction — `qui_step`/`qui_push_key_event` are plain
function calls, not JSON messages over stdio).

Concretely:

| | `quadraui-ipc` (#118) | `quadraui-ffi` (this crate) |
|---|---|---|
| Process model | separate process, stdio pipe | same process, linked library |
| Transport | JSON messages, every call | plain `extern "C"` calls; JSON only for descriptor payloads |
| Target consumer | any language that can speak JSON over a pipe | any language with a C FFI (Python `ctypes`/`cffi`, Swift, Kotlin/JNI, Go `cgo`, …) |
| Latency/coupling | one message round-trip per interaction | direct call, no serialization round-trip for control flow |

Neither supersedes the other, and there's no reuse opportunity beyond
"both happen to use `serde_json` for payloads" — #118's JSON *is* its
wire protocol; this crate's JSON is an implementation detail of one
function's argument encoding. A future consumer that wants an
out-of-process, language-agnostic bridge wants #118. A consumer that
wants to link quadraui directly as a native library from C, C++, Swift,
or similar wants this crate's eventual real ABI. Both are legitimate,
non-overlapping phase-1-and-beyond deliverables.

## Why `tui`, not `gtk`, for this PoC's backend

The PoC drives [`quadraui::tui::testing::TuiDriver`], quadraui's headless
TUI test harness, rather than the real `gtk` backend:

- It runs in any sandbox or CI host with no real terminal, no X server/
  Wayland compositor, and no `pkg-config`/GTK system dependency — exactly
  the constraint `ci.yml`'s own `--exclude kubeui-gtk` guidance exists
  for, and the same reason this crate's own tests run headlessly rather
  than against a real window.
- The PoC's question is about the **ABI shape** (opaque handle, vtable,
  non-blocking step, string ownership, error codes), which is identical
  regardless of which `Backend` renders the frame underneath — `tui` and
  `gtk` are both just `Backend` impls behind the same `AppLogic` contract
  (`quadraui`'s cross-backend portability commitment). Proving the ABI
  shape against `tui` proves it against `gtk` too, with far less
  incidental setup cost for a phase-1 spike.

## Running the PoC

```bash
cargo test -p quadraui-ffi                 # Rust-side round-trip + header-freshness tests
./generate-header.sh                       # regenerate include/quadraui_ffi.h after an ABI change
cd smoke && ./run.sh                       # Linux-only C smoke harness (see smoke/run.sh's own header)
```
