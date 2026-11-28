# `quadraui-wasm` — a WASM/Canvas backend proposal

A fourth pixel backend: compile the existing Rust engine to `wasm32` and
paint through `<canvas>`, so an unmodified quadraui app runs in a browser
tab with no server process, true pixel fidelity with GTK/macOS/Win, and
(per the widget-contract consolidation) near-zero new rendering code.

Status: **rejected for shipping apps (D-030, 2026-11-27).** This
resolved `docs/WEB_BACKEND_PROPOSAL.md` / D-015's semantic-HTML-vs-
pixel-canvas fork toward pixel canvas — then, walking through what it
would actually buy (§6a), turned out to solve the one named personal
case (coord-tui over Tailscale) worse than a tool that already exists
(`ttyd`/`gotty` over the existing TUI binary), and to not clear the bar
for general use for the reasons §4 already listed (canvas text
input/IME, zero accessibility, no native text selection). Not queued.
The doc stays as the record of the pixel-canvas design and the one
narrow use left genuinely open: CI/visual-regression infra (§6a). See
D-030 in `docs/decisions/DECISIONS.md` for the full decision.

---

## TL;DR

- **This is not a JS rewrite.** The question that prompted this (“could
  a JS backend let a quadraui-node app run everywhere, including the
  browser?”) has a false premise: `quadraui-node` is a native `napi-rs`
  leaf over the Rust engine (`quadraui-binding-core`) — a `.node`
  addon. There is no way to run that in a browser; a hand-written JS
  rendering engine would instead duplicate the Primitive/Interactive
  contract the widget-contract RFC just spent a release cycle
  *consolidating into one Rust implementation* specifically to kill
  per-backend drift (TreeTable forking the contract is the RFC's own
  root-cause example). Don't reopen that.
- **The actual path is a `wasm32-unknown-unknown` build of the same
  core**, with a new `WasmBackend` implementing `PaintSurface` (the
  ~15-verb trait already shared by GTK/macOS/Win, `quadraui/src/paint_surface.rs`)
  against `CanvasRenderingContext2D`, plus `WindowControl`/
  `PlatformServices` stubs for the browser. One Rust implementation,
  same as every other backend; a thin `wasm-bindgen`-generated JS
  loader, not a renderer.
- **This is cheaper now than when quadraweb was shelved, measurably:**
  `PaintSurface` already exists and **52 of 54 primitive files already
  paint through it** (`grep -rl native_surface_paint quadraui/src/primitives/
  | wc -l`, checked 2026-11-26). A `WasmBackend` implementing that one
  trait gets correct rendering for ~96% of primitives for free, via the
  same generic `native_surface_paint::paint` helpers GTK/macOS/Win
  already share. `Backend` itself still has 91 `draw_*`/`*_layout`
  methods today; the widget-contract collapse (0.12/0.13, in flight) is
  independently shrinking that to ~66 platform-only methods — a smaller
  target by the time this would start.
- **PaintSurface's verbs map almost 1:1 onto Canvas2D**: `surface_fill_rect`
  → `fillRect`, `surface_stroke_rect` → `strokeRect`, `surface_draw_text_run(_styled)`
  → `fillText` + `font`/`textBaseline`, `surface_measure_text` →
  `measureText`, `surface_draw_path` (`MoveTo`/`LineTo`/`Close`) →
  `beginPath`/`moveTo`/`lineTo`/`closePath`, `surface_push_clip`/`pop_clip`
  → `save`/`clip`/`restore`, `surface_draw_image` → `drawImage`,
  `surface_draw_shadow` → the existing composed-rounded-rect default,
  unchanged (no native shadow primitive needed). There is no
  impedance mismatch to design around.
- **Cost accepted:** no filesystem; no subprocess/pty, so `Terminal`'s
  *input source* (`portable-pty`) doesn't work in-browser even though
  its *painting* does (it's one of the 52 migrated primitives) — a
  browser `Terminal` would need a remote shell over WebSocket, which is
  a separate, optional piece, not a backend blocker. Font metrics come
  from the browser's text shaper, not the OS's, so expect the same kind
  of per-backend pixel-fidelity chase already tracked for GTK/macOS/Win
  (cf. quadraui#3038) — one more backend to converge, not a new problem
  class.

---

## 1. Why this, not the JS-backend framing

The question this answers was: *“the widget contract plus the Node
package — could we replace the Rust backend with a JS backend that
draws to a browser, so a quadraui-node app runs everywhere, including
the browser?”* Two separate things are being conflated:

1. **Where app/engine logic executes.** Today, exactly once — in Rust.
   `quadraui-node` and `quadraui-py` are thin per-language FFI leaves
   (`napi-rs` / `PyO3`) over one shared core, `quadraui-binding-core`
   — proven out already (see `docs/decisions/BINDING_STRATEGY.md`).
   That core cannot run in a browser as a `.node` addon. It *can* run
   in a browser as `wasm32` — same source, different `--target`.
2. **Where pixels get drawn.** Today: GTK (Cairo), macOS (CoreGraphics),
   Windows (Direct2D), TUI (cell grid) — all through `Backend`, and the
   three pixel backends increasingly through the shared `PaintSurface`
   seam. A browser canvas is a fifth painter of the same shape as the
   first three, not a different paradigm.

A hand-written JS backend would mean re-implementing (2) in TypeScript
— and, to make a JS-authored app logic actually portable, likely (1)
too. Both already exist, in one place, in Rust, and the widget-contract
RFC is actively finishing the job of making (2) a ~15-verb trait instead
of 71+ duplicated methods. Forking that into a second language reopens
the exact failure mode the RFC exists to prevent, for a feature (an
all-JS engine) nobody asked for independent of “run in a browser.”

**Conclusion: keep the engine singular. Add `wasm32` as a build target
and a canvas painter as a fifth backend.** The “node package runs in
the browser” framing becomes, correctly: *a `wasm-bindgen` package with
the same (or a very similar) JS API surface as `quadraui-node`, built
from the same Rust core, runs in the browser; the native `.node`
package still runs in Node.* Two artifacts, one source of truth — the
same relationship `quadraui-node` and `quadraui-py` already have to each
other.

---

## 2. The three enabling facts (why this is cheap *now*)

### 2.1 `PaintSurface` already exists and already covers almost everything

`quadraui/src/paint_surface.rs` is a ~15-verb trait, extracted from
private helpers each of GTK/macOS/Win already had, specifically so "a
future pixel backend... now only needs to implement this ~15-verb
trait, plus `Services`/`WindowHost`" (its own module doc, written before
this proposal, for a different reason — reducing `Backend`'s surface —
but it is exactly the seam a browser backend needs).

Measured 2026-11-26: **52 of 54 `quadraui/src/primitives/*.rs` files**
already route their painting through `native_surface_paint` helpers
generic over `&mut dyn PaintSurface`, including `chart.rs`,
`diff_view.rs`, and `terminal.rs` — the exact primitives
`WEB_BACKEND_PROPOSAL.md` (§“Cost accepted”) expected to need bespoke
per-primitive handling under the *semantic-HTML* model. Under the
pixel-canvas model they need nothing special; they already paint
through the shared trait like everything else.

### 2.2 The verb set maps onto Canvas2D with no translation layer

Every verb (frame/viewport, measure, fill/stroke (+rounded), text run
(+styled/+role), icon glyph, line, shadow (composed, no primitive
needed), path (straight segments only), clip push/pop, image) has a
direct Canvas2D equivalent. There's no verb here that assumes a desktop
2D API feature Canvas lacks (no gradients, no custom fonts beyond
`FontFace`, no blur primitive are used — `surface_draw_shadow`'s default
is already composed from `surface_fill_rounded_rect` for exactly this
kind of portability).

### 2.3 Multi-target-from-one-core is already the bindings' proven pattern

`quadraui-binding-core` already demonstrates "write the host-layer
wiring once in Rust, expose it through a thin per-target leaf"
(`napi-rs` for Node, `PyO3` for Python — `docs/decisions/BINDING_STRATEGY.md`).
A `wasm-bindgen` leaf is a third instance of the same pattern, not a new
one: same core crate, same descriptor/event model, different glue.

---

## 3. Architecture

```
Rust core (unchanged)                 Browser
──────────────────────                ───────
AppLogic / ShellApp (unchanged)
  every widget's Primitive::paint ───┐
  (DrawsPrimitives::draw_dyn)        │ each verb → Canvas2D call
    → &mut dyn PaintSurface ─────────┤
WasmBackend: impl PaintSurface ──────┘
  + WindowControl (resize/title→
    document.title, no real window)
  + PlatformServices (clipboard via
    navigator.clipboard behind a
    secure-context check; no fs)
compiled: wasm32-unknown-unknown
        │  wasm-bindgen
        ▼
  quadraui-wasm.js  (thin loader —
  instantiate module, forward
  resize/pointer/key/IME events in,
  request a repaint, nothing else)
```

- **Event loop:** mirrors `tui::run`/the existing pixel backends' run
  loops — a `requestAnimationFrame`-driven `step()`, matching the
  `step()`/`pump()` runner already shared across all 4 backends
  (`quadraui-py`'s binding foundation). No new runner model.
- **`UiEvent` routing is unchanged.** Pointer/key events come in with
  real coordinates from the DOM (`event.offsetX/Y`), unlike quadraweb's
  identity-in/coordinate-reconstructed trick — a canvas has one
  coordinate space, so this is simpler than the semantic-HTML model,
  not harder.
- **DPI:** `window.devicePixelRatio` feeds `Viewport.scale`, the same
  field GTK/Win/macOS already populate from their own live DPI reads
  (D-016) — no new concept.
- **No server, no WebSocket.** Unlike quadraweb (which kept `CoordApp`'s
  Rust process server-side because it needs SQLite/subprocess access),
  a generic quadraui app with no such server-side dependency runs
  entirely client-side. An app that *does* need a server (file I/O,
  subprocesses) either keeps doing that over its own API, same as any
  other web app, or isn't a candidate for this backend — same
  constraint the user already named ("obvious limitations of no local
  filesystem"), extended to "no subprocess/pty."

---

## 4. What's out of scope for v0

- **`Terminal`'s input side.** Painting works (migrated primitive);
  `portable-pty` cannot spawn a local process in a browser sandbox. A
  real terminal in this backend needs a remote shell bridged over
  WebSocket — a separate, optional feature, not a backend requirement.
  Ship without it; revisit if a consumer needs it.
- **Filesystem-backed `PlatformServices`** (open/save dialogs, trash,
  reveal-in-Finder-equivalent). No browser equivalent exists for most
  of these; stub to no-ops/`Err`, same as any platform-services gap on
  an existing backend.
- **The 9 primitive files not yet migrated onto `PaintSurface`**
  (`grep -rL native_surface_paint quadraui/src/primitives/*.rs`, 2 found
  at measurement time, both already partway there per the module doc).
  Those need their own `Backend::draw_*` path implemented on
  `WasmBackend` directly, same as any backend would — small, bounded,
  shrinking further as the widget-contract collapse finishes.
- **Pixel-identical fidelity with native backends on day one.** Expect
  a fidelity-chase period like every other backend has had (font
  metrics, hinting, subpixel positioning) — not a blocker, a known
  shape of work (cf. quadraui#3038).

---

## 5. Phased roadmap

- **Phase 0 — gate.** Do not start before the widget-contract collapse
  (0.12/0.13, in flight — see `docs/decisions/DECISIONS.md` and
  `quadraui-widget-contract-rfc`/`quadraui-012-013-release-plan`
  tracking) lands. Writing this backend against the ~66-platform-method
  post-collapse `Backend` is strictly less work than against the
  91-method trait today, and the contract is specifically designed so
  a new backend implements a small surface, not 71+ draw methods.
- **Phase 1 — spike.** `WasmBackend` implementing just `PaintSurface` +
  a minimal `WindowControl`/`PlatformServices`, `wasm32-unknown-unknown`
  build, hand-verify one real app (quadraui-gallery is the natural
  target — it already exercises every primitive × every backend) opens
  and paints in a browser tab. No event handling yet.
- **Phase 2 — interaction.** Pointer/key/IME events in, resize/DPI,
  clipboard behind the secure-context check. `quadraui-gallery` fully
  interactive in-browser.
  (The existing `RecordingBackend`/`MockBackend` test infrastructure in
  `quadraui/src/testing/` is the template for headless `WasmBackend`
  acceptance tests — no browser automation needed to test the painter
  itself, only to confirm the `wasm-bindgen` glue loads.)
- **Phase 3 — bindings parity.** A `quadraui-wasm` npm package (built
  alongside, not instead of, `quadraui-node`'s native build — see
  `docs/decisions/BINDING_VERSION_POLICY.md` for the versioning
  precedent `quadraui-node`/`quadraui-py` already set) so an app
  authored against the shared JS/TS API can target either runtime.
  Gate on `quadraui-node` itself actually shipping (0.13 per the
  current release plan) — don't build the browser leaf before the
  Node leaf it's meant to parallel exists.
- **Phase 4 — the fidelity chase + `Terminal`-over-WebSocket**, if a
  real consumer wants either.

---

## 6. Relationship to `WEB_BACKEND_PROPOSAL.md` / D-015

D-015 shelved quadraweb (CoordApp-on-phone, solved another way) but
explicitly left the semantic-HTML-vs-pixel-canvas choice open for a
future revival, with this trade table:

| | Semantic HTML (D7, quadraweb) | Pixel canvas (this doc) |
|---|---|---|
| What it ships | 42 hand-written HTML emitters | `WasmBackend` impl of 1 trait (~15 verbs), already written for GTK/macOS/Win |
| Reflow | Genuine, CSS-driven | None — fixed layout like GTK, computed in Rust |
| Accessibility / text selection | Native DOM, free | None without extra work |
| Fidelity with native backends | Not a goal (HTML, not pixels) | High — same painter shape as GTK/macOS/Win |
| App logic location | Server (Rust process stays up) | Client (compiled to wasm, runs in-tab) |
| What it is | A web app rendering of the primitives | The GUI app itself, in a browser tab |

This proposal **resolves that fork toward pixel canvas**, but for a
different reason than D-015 anticipated: not "phone access to one
stateful server app" (quadraweb's problem, already solved) but "the
same portable app binary, compiled once more, with no server at all."
If a future need actually wants adaptive reflow and free accessibility
over fidelity, `WEB_BACKEND_PROPOSAL.md`'s D7 model is still the right
answer for *that* need — the two are not mutually exclusive, they solve
different problems, and the trade table above still applies when
choosing between them for a specific consumer.

---

## 6a. Why this is rejected for shipping apps (D-030, 2026-11-27)

Feasibility (§§1–3) was never the problem — the design above is sound
and cheap relative to when quadraweb was shelved. What kills it is that
neither case it would serve is actually worth having.

**The named personal case is already solved, with no new code.**
coord-tui already runs as a terminal program. `ttyd`/`gotty` wraps any
TUI binary as an xterm.js session served over HTTP — "coord-tui, in a
browser tab, over Tailscale" exists today with zero lines written, and
it has *none* of this backend's real costs: xterm.js already solved
text input/IME, native terminal text selection, and doesn't need
accessibility bolted on (a terminal is already screen-reader-legible).
Building `WasmBackend` to reach the same destination would mean taking
on this doc's hardest open problems (canvas IME, zero accessibility, no
native text selection or find-in-page — see §4 and the discussion that
produced this decision) to arrive somewhere a 15-year-old class of tool
already gets you for free.

**The general case doesn't clear the bar either.** Every real "canvas
GUI in a browser tab" success (Figma is the standard reference) was a
single product with a dedicated rendering team making a deliberate,
product-specific bet — not a reusable multi-backend framework that is
*also* trying to be a way to build web apps. Adding this backend adds a
fifth backend's permanent cost (pixel-fidelity chasing forever, a wasm
toolchain, permanent IME/accessibility debt) without extending
quadraui's actual pitch ("one app, native everywhere, for apps that
already have native targets"). Nobody surveyed in reaching this
decision — including the person who'd be the first user — came up with
a reason to reach for it over the existing TUI/GTK/native builds, or
over wrapping the TUI in a terminal-in-browser tool.

**What's left standing, narrowly, and still undecided:** a
headless-Chrome wasm build as CI/visual-regression infrastructure, not
an app target. The gallery's screenshot matrix has a real, already-felt
gap — GTK and Windows have no capture leg and sit "pending" (native GUI
capture is fragile: macmini AX/tmux issues, dell64 sleep stalling
Windows smoke). A headless render sidesteps all of that, and the
limitations that sink the app-shipping case (no filesystem, no
accessibility, no IME) are irrelevant to a test harness. This is real,
but it's a thin justification on its own — whether it's worth a new
backend's permanent maintenance cost versus just fixing the native
capture legs directly is a close call. **Not decided, not queued.** If
it's ever picked up, it should be scoped explicitly as test
infrastructure from the start, not as "the WASM backend, which happens
to also be useful for CI" — the two framings lead to very different
scope.

---

## 7. Open questions

- **Q1 — JS API surface for the wasm leaf.** Hand-authored to mirror
  `quadraui-node`'s API, or generated from the same descriptor/schema
  source `quadraui-binding-core` already uses for its wire format? The
  widget-contract RFC's "schemars-derived wire schema" for the bindings
  leaf (see `quadraui-widget-contract-rfc`) is the natural shared source
  if it lands first.
- **Q2 — bundle size.** No measurement taken yet; `wasm32` builds of
  comparable Rust GUI cores (egui, iced) run from several hundred KB to
  a few MB gzipped. Worth a real number before Phase 3, not a blocker
  for the Phase 1 spike.
- **Q3 — does this want its own repo** (like `quadraui-files` did for
  the Python file manager) or live in-tree as a `web`/`wasm` cargo
  feature, peer of `src/gtk/`? Precedent exists for both; decide at
  Phase 1 based on how large `WasmBackend` + its loader actually get.
- **Q4 — is there a real consumer yet?** Resolved, no: walking through
  the two candidate consumers (coord-tui over Tailscale; general app
  shipping) in the discussion that produced D-030, neither held up —
  see §6a. Superseded by the decision; kept here for the record of how
  the question was originally framed.

---

## 8. Disposition (see D-030)

**Rejected for shipping apps, 2026-11-27.** See §6a and D-030 in
`docs/decisions/DECISIONS.md` for the reasoning. Not queued, no issues
filed. The one thing left genuinely open is the narrow CI/visual-
regression use in §6a — if that's ever picked up, treat it as a new,
differently-scoped proposal (test infra, not an app backend), not a
revival of this one.

---

## 9. References

- `quadraui/docs/WEB_BACKEND_PROPOSAL.md` — the shelved semantic-HTML
  design this supersedes-for-a-different-case, not invalidates.
- `quadraui/docs/decisions/DECISIONS.md` — D-015 (web backend shelved,
  the fork this resolves), D-016 (unit contract / DPI scale, reused
  unchanged here), and D-030 (this proposal rejected for shipping apps).
- `quadraui/src/paint_surface.rs` — the `PaintSurface` trait this
  backend implements.
- `quadraui/docs/decisions/BINDING_STRATEGY.md` — the native-per-language
  bindings decision this proposal's "third leaf, same core" shape
  mirrors.
