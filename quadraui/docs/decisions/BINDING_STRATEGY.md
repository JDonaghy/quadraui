# quadraui binding strategy — which languages, over which bridge

**Status:** Decided. 2026-11-01.

**Scope:** Which language bindings quadraui invests in near-term, and the
bridge each one sits on. Supersedes the implicit assumption in epic #1096
("Language bindings") that a C ABI is the foundation *every* binding is
layered on.

---

## Question

quadraui ships one binding today — `quadraui-py` (PyO3, binds `quadraui`
directly). A C ABI spike crate (`quadraui-ffi`, `publish = false`) also
exists. The open question was architectural: should every future language
binding be **layered on the C ABI** — and `quadraui-py` re-platformed onto
it — or should bindings bind `quadraui` **directly**, each through its own
language-native Rust bridge?

Candidate target languages, by likelihood of near-term demand:

- **Likely:** Python, JavaScript/TypeScript, Go, Java, C#, Lua
- **Less likely:** C, C++

## Decision

**Near-term, invest only in `Python` (PyO3) and `JavaScript/TypeScript`
(napi-rs), both binding `quadraui` directly. Park the C ABI
(`quadraui-ffi`) until a concrete C-only-language consumer creates demand.**

- `quadraui-py` stays on PyO3 and is **not** re-platformed onto the C ABI.
  Its parity gap with the Rust API is tracked and closed under epic #1866
  (milestone #28), not by routing it through `quadraui-ffi`.
- A future `quadraui-js` binding, when demand appears, uses **napi-rs**
  (native Node addon), binding `quadraui` directly — same pattern as PyO3.
- `quadraui-ffi` is kept as a **de-risked, proven-viable spike** (see
  below), not deleted, but no further investment until a C-only language
  (Go / C# / Lua / Java) is actually wanted. At that point this decision is
  revisited with a real consumer in hand.

## Why

### The C ABI is the lowest common denominator — by construction

A C ABI is the universal FFI substrate: every candidate language can call
C. But it is universal *because* it is impoverished. C has no objects,
closures, generics, tagged enums, or ownership, so to be C-callable the API
must be flattened to **bytes + integers + function pointers**. Universality
and poverty are the same property.

The consequence is structural: **whatever richness the C ABI does not
expose, no language-side binding can add back** — a binding can only
re-wrap what C hands it. The C ABI's ceiling becomes every binding's
ceiling. That is precisely the "paint-and-hit-test shim" failure mode the
#1866 audit found in `quadraui-py` today — a C-ABI-for-everything strategy
risks making that ceiling permanent and shared.

### A C ABI needs adapters on both sides — N+1 for N languages

- **Rust side:** one adapter (`quadraui-ffi`), built once, shared. The flat
  re-projection — JSON descriptors, opaque `i64` handles, a C vtable of
  callbacks, error codes + a thread-local last-error string, semantic-event
  JSON emission.
- **Language side:** one adapter *per language* (N of them), each
  non-trivial: declare the C signatures in that language's FFI, marshal
  native types ↔ the JSON/handle wire form, wire the callback vtable, manage
  error-string ownership, and build an idiomatic facade so nobody hand-writes
  JSON. The spike's `ctypes` `munmap_chunk()` SIGABRT (see
  `quadraui-ffi/SPIKE_FINDINGS.md`) is the kind of ownership trap every such
  adapter author hits once.

### Rich native bridges beat the C ABI for the languages we actually want

PyO3 and napi-rs also flatten Rust to a C-level boundary under the hood (the
CPython C-API, Node N-API). The decisive difference is **when and with how
much type knowledge** the flattening happens:

- **Flat C ABI (`quadraui-ffi`):** flatten *once, up front, in Rust, to the
  LCD*. Every language inherits the already-impoverished surface.
- **PyO3 / napi-rs:** flatten *per-language, at compile time, with full
  type knowledge, in the same address space*. Real Rust structs, methods,
  `Result` errors, and the `compose/` interaction controllers can be
  projected as first-class native objects. Richness is preserved, not
  pre-discarded.

For Python and JS specifically, the rich bridge is the better tool, full
stop.

### A C ABI is only *mandatory* for languages we don't need yet

Several candidate languages cannot bind Rust directly at all — they can only
reach it through a C-level boundary:

| Language | Direct Rust bind? | Path to Rust |
|---|---|---|
| Python | **Yes** | PyO3 (native extension) |
| JS/TS | **Yes** | napi-rs (native addon) |
| Go | No | cgo → **C** |
| C# | No | P/Invoke → **C** |
| Lua | No | LuaJIT `ffi` / C module → **C** |
| Java | Barely | JNI / Panama → **C**-level |

So the C ABI is genuinely *required* only once Go, C#, Lua, or Java is
wanted — and none is, near-term. Building and maintaining the shared C-ABI
re-projection now would be paying the LCD tax for consumers that don't
exist, while the two languages we do want are better served without it.

### The spike already de-risked the future C-ABI path

The `quadraui-ffi` interactive spike (#1887, verdict **Go** —
`quadraui-ffi/SPIKE_FINDINGS.md`) proved a foreign host can, over the C ABI
only, run a real `TuiRunner` loop, build an interactive primitive, receive a
hit-tested semantic event, and update a descriptor in place. So when a
C-only consumer does appear, we start from a proven-viable base with the P0
gaps already enumerated (no hit-zone registry, no handle removal, full-repaint
per mutation, modifier-insensitive keys). Parking the C ABI costs us nothing
we'd have to rediscover.

## Consequences

- **Epic #1096 ("Language bindings") is narrowed:** its "C ABI as the
  foundation under every binding" framing is set aside. The C-ABI /
  descriptor-store work remains valuable groundwork but is not a prerequisite
  for the bindings we're actually building.
- **Epic #1866 ("quadraui-py parity") is the active binding workstream** —
  closing the PyO3 binding's parity gap with the Rust API, directly, via
  `compose/` controllers rather than a flattened wire format.
- **`quadraui-ffi` stays in-tree** at `version = "0.0.0"`, `publish =
  false`, as a parked, proven spike. No deletion, no further investment
  until this decision is revisited with a real C-only consumer.
- **A `quadraui-js` (napi-rs) binding** is the expected second binding when
  JS/TS demand appears; it follows the PyO3 pattern, not the C-ABI pattern.

## Revisit when

A concrete consumer in a C-only language (Go, C#, Lua, or Java) is actually
wanted. At that point, weigh building out `quadraui-ffi` (from the spike's
proven base, closing its P0 gaps) against that language's best available
native bridge — with a real use case to judge against, not a hypothetical.
