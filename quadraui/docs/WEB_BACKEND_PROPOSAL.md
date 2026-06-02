# `quadraweb` — web backend proposal

A browser backend for quadraui: take an existing backend-generic
quadraui app (`AppLogic` / `ShellApp`) and run it in a phone or
desktop browser with **zero app changes**, an **adaptive** (not
terminal-grid) layout, and no new wire format to invent.

First consumer + acceptance target: **claude-coordinator's `CoordApp`**,
viewable and usable from a phone browser over Tailscale (no TLS, no
external hosting).

Status: **proposal**. Tracked by the `quadraweb` epic (#314); phases
#315 (spike), #316 (MVP), #317 (adaptivity), #318 (polish).

---

## TL;DR

- A web port is **not an app rewrite**. `CoordApp` is already a
  `quadraui::ShellApp`; its TUI and GTK entry points are ~10-line shims
  over `run_with_shell(...)`. quadraweb adds a *third shim*:
  `quadraui::web::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config())`.
  Same `win`/`macos` story — `src/win/` is only 4 files.
- Three properties of the codebase make this cheap:
  1. **Apps are backend-generic** — one `ShellApp` impl drives every backend.
  2. **Every primitive + every `UiEvent` already derives `Serialize`/`Deserialize`**
     (36/37 primitive files; `mod.rs` is the only holdout). The
     server can ship the *actual primitive structs* as JSON; the browser
     can ship raw events back. The wire format already exists — this is
     the `quadraui-ipc` seam.
  3. **Hit-testing is already server-side** (`ScreenLayout::draw` →
     `FrameHitMap`; `*_layout()` methods return hit regions). The browser
     stays *dumb*: it sends raw `(x, y)`; the server resolves it to a
     `WidgetId`.
- Consequence: the **Rust half of the web backend is unusually cheap.**
  Unlike GTK's bespoke Cairo per `draw_*`, most of the ~40 trait methods
  collapse to `self.push(WebItem { rect, prim })`. The real work is the
  **browser renderer**.
- **Chosen renderer: DOM/TypeScript, phased.** The server ships a serde
  display list over a WebSocket; the browser maps each primitive to HTML
  via one small component per primitive (`<qw-tree>`, `<qw-list>`, …),
  AppShell-zone-aware so CSS can re-arrange chrome responsively
  (sidebar → drawer on narrow viewports). Cost accepted: the
  per-primitive rasteriser is duplicated in TypeScript rather than living
  once in Rust. The *app* and the *primitive definitions* stay
  single-source.

---

## 1. Goals & non-goals

**Goals**

1. Run an unmodified backend-generic quadraui app in a browser.
2. Adaptive layout — re-flows and re-arranges for screen size, rather
   than emulating a fixed terminal grid.
3. Phone-first, over Tailscale: `ws://<tailscale-ip>:<port>`, no certs,
   single user, single client.
4. Stay *close* to the cross-backend portability commitment even though
   this backend is explicitly allowed to bend it (see §10).

**Non-goals (for v0)**

- Competing with Flutter/React on visual polish. "Doesn't need to feel
  that modern" is a stated requirement, not a regression.
- TLS, auth, multi-tenant, or public hosting. Tailscale provides the
  network boundary and encryption.
- Pixel-identical fidelity with the GTK backend. The web backend is a
  *new* rasteriser, not a screenshot of an existing one.
- Compiling the app to WASM. The app runs on the server (it has a SQLite
  DB, spawns subprocesses, etc.); only the *renderer* lives in the browser.

---

## 2. The three enabling facts (why this is cheap)

### 2.1 The app is already backend-generic

`CoordApp` implements `ShellApp`. Its entry points:

```rust
// coord/tui/src/main.rs
quadraui::tui::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());

// coord/tui/src/bin/gtk.rs
quadraui::gtk::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
```

quadraweb adds:

```rust
// coord/tui/src/bin/web.rs
quadraui::web::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
```

No changes to `CoordApp`, its render path, or its event handling.

### 2.2 Serde is the wire format

`UiEvent`, `Rect`, `Point`, `Viewport`, and 36/37 primitives derive
`Serialize`/`Deserialize`. The server serializes what it would paint;
the browser serializes what the user did. We do not design a protocol —
we reuse the one that already exists for `quadraui-ipc`/`quadraui-lua`.
(Closing the `mod.rs` gap, if any concrete type there needs to cross the
wire, is a small follow-up.)

### 2.3 Hit-testing already lives on the server

`ScreenLayout::draw` returns a `FrameHitMap`; layout-passthrough methods
(`status_bar_layout`, `tab_bar_layout`, `tree_layout`, …) return hit
regions in surface coordinates; the app routes clicks against them. So
the browser never needs to know widget identity. The web backend builds
the same hit map during render and, in `poll_events`, translates a raw
browser `(x, y)` into a `UiEvent` carrying the resolved `Option<WidgetId>`
— exactly the contract every other backend honours.

---

## 3. The abstraction-cut spectrum (where xterm sits)

"xterm.js vs alternatives" is really one question: **at what level do we
cut the abstraction for the browser?** The cut determines how adaptive we
can be.

| Cut point | Ships to browser | Adaptivity | App change | Renderer cost |
|---|---|---|---|---|
| Cell grid (xterm.js) | ANSI / cell buffer | None — it *is* a terminal | None | ~zero (reuse `TuiBackend`) |
| **Positioned primitives** | `[{rect, Prim}]` | Reflows to width | None | TS renderer, no layout logic |
| **Semantic zones** (AppShell-aware) | named zones + per-zone prims | Genuine (sidebar→drawer) | None | TS renderer + CSS layout |
| Domain model (rewrite) | app data | Fully native | Total rewrite | A whole app |

xterm.js is the lowest cut: fastest to "something on my phone", but it
delivers exactly the terminal-grid look the project wants to avoid, and
is hostile to touch (cell-coordinate taps, tiny monospace, no soft
keyboard). A full rewrite throws away the property that makes this cheap.
quadraweb targets the **middle two cuts**, migrating upward zone by zone
(§7).

---

## 4. Chosen renderer: DOM/TypeScript

The server ships the serde primitive list; the browser holds one small
component per primitive that maps it to HTML.

```
Server (Rust)                         Browser (TypeScript)
─────────────                         ────────────────────
CoordApp (unchanged)
  draw_tree(rect, &tree) ─┐
  draw_list(rect, &list) ─┤ accumulate Vec<WebItem>
  ...                     │ { rect, prim }
WebBackend.end_frame  ────┘
        │   ws ▼  JSON display list
        │                              <qw-tree>  → <ul>…
        │                              <qw-list>  → <div>…
        │                              <qw-form>  → <form>…
        ▲   ws ▲  { type:"tap", x, y }
        │
  FrameHitMap resolves (x, y) → WidgetId → UiEvent → app.handle
```

**Why DOM over the alternatives we considered:**

- **vs xterm.js** — crisp proportional text, real tap targets, soft
  keyboard via a focused hidden `<input>`, accessibility for free, and
  CSS-driven adaptive layout. xterm gives none of these.
- **vs WASM canvas (Rust rasteriser in the browser)** — the WASM option
  keeps the rasteriser in Rust (closest to the portability commitment)
  and is the most *faithful*, but canvas is just pixels: layout must
  still be computed server- or wasm-side, accessibility is poor, and
  mobile text rendering is mediocre. It is *more* work for *less* of what
  this project wants (adaptive + phone-friendly). Recorded as the
  rejected alternative in §11.
- **vs htmx** — great for request/response page apps; `CoordApp` is a
  live, stateful, ~60 Hz event loop (drag, scroll, key, ticks). Pushing
  full HTML per frame and routing fine-grained input through htmx's
  request model fights the grain. htmx may serve the initial page shell
  and nothing more.

---

## 5. Architecture

### 5.1 The runner loop (identical structure to `tui::run`)

The existing runners are blocking single-thread loops: `wait_events(timeout)`
→ `app.handle` → redraw. The web runner keeps that loop and puts a network
between the backend and the human.

- An HTTP server (axum or similar) serves the static frontend and upgrades
  to a WebSocket.
- A socket-reader task feeds an `mpsc<UiEvent>`. `WebBackend::wait_events`
  is `recv_timeout`; `poll_events` is `try_recv`.
- `WebBackend::end_frame` serializes the accumulated `Vec<WebItem>` and
  pushes it down the socket.
- `WebBackend::begin_frame` takes the `Viewport` from the browser's
  last-reported size, so a resize / phone rotation emits `WindowResized`
  and the app re-lays-out **for free**.

`AppLogic`/`ShellApp` and `Reaction` are unchanged. The "frame loop" and
"event drain" are the same as `tui/run.rs`; only the transport differs.

### 5.2 `WebBackend` — the display-list accumulator

Most `draw_*` methods push a variant of an internal accumulation enum:

```rust
enum WebItem {
    Tree   { rect: Rect, tree: TreeView },
    List   { rect: Rect, list: ListView },
    Form   { rect: Rect, form: Form },
    StatusBar { rect: Rect, bar: StatusBar,
                hovered: Option<WidgetId>, pressed: Option<WidgetId> },
    // … one per primitive
}
```

This is the `AnyPrimitive` enum that `BACKEND_TRAIT_PROPOSAL.md` §6.1
deliberately kept *out of the trait* — but here it is an *internal*
accumulation type for a single backend, which is idiomatic and fine.
Draw order is preserved, so z-order (modals, toasts, tooltips, context
menus pushed last by `ScreenLayout`) maps directly to DOM order /
`z-index`.

The layout-returning methods (`draw_status_bar -> StatusBarLayout`,
`tree_layout`, …) still return real layouts: the web backend picks its
metrics (`line_height()` ≈ 18 px, `char_width()` ≈ 8 px — the "GTK-like
pixel backend" model the `macos` backend already proves) and reuses the
shared `*Measure` helpers, so server-side hit regions stay correct.

### 5.3 The browser renderer

A tiny TS app: open the WebSocket, on each frame replace/patch the DOM
from `Vec<WebItem>`, and on user input send `{type, x, y, key, …}`. One
component per primitive. Start with **only the primitives `CoordApp`
actually uses** (tree, list, form, status bar, tab bar, toast, context
menu, sidebar panel, pipeline view, message list) and grow the set as
other apps need them — same incremental discipline as adding a primitive
to a native backend.

---

## 6. Wire format (v0)

Down (server → browser), per frame:

```jsonc
{ "viewport": { "width": 390, "height": 780, "scale": 2.0 },
  "theme":    { /* serde Theme → CSS variables */ },
  "items": [ { "rect": {…}, "prim": { "Tree": { /* TreeView */ } } }, … ] }
```

Up (browser → server), per input:

```jsonc
{ "kind": "pointer", "phase": "down", "x": 120, "y": 44, "button": "left", "mods": {…} }
{ "kind": "key", "key": "Enter", "mods": {…} }
{ "kind": "char", "ch": "h" }
{ "kind": "scroll", "x": 120, "y": 44, "dx": 0, "dy": -3 }
{ "kind": "resize", "width": 390, "height": 780, "scale": 2.0 }
```

The web backend converts the *up* messages into `UiEvent` (resolving
`WidgetId` via the `FrameHitMap`); the app already understands the rest.

**Bandwidth:** shipping the full list each frame is fine for a low-Hz
dashboard. If it bites, diff frames and send only changed items. Note
`diff/mod.rs` is Myers *text* diff (for `DiffView`), not a frame diff —
frame diffing would be new code, deferred until measured.

---

## 7. The fidelity ⇄ adaptivity tension (the one real design risk)

In the **positioned-primitives** cut (Phase 1), the server computes every
rect with metrics *it* chose, and hit-testing is server-side. For this to
work, **the browser must render at exactly those metrics** — if the server
thinks a row is 18 px tall but the browser draws it 22 px, clicks drift.
So the first cut feels like a fixed pixel grid that *reflows to width* —
responsive, not yet adaptive.

True adaptivity (sidebar → drawer) means moving layout **up into the
browser** for the chrome — and the moment the browser owns a zone's
layout, it must own that zone's hit-testing too. That is why the
migration is **zone by zone**: `AppShell` has named zones (sidebar / main
/ bottom panel / status / title). The web runner ships those as semantic
containers, lets CSS arrange them, and renders primitives inside each via
the Phase-1 renderer. We raise the cut one zone at a time, never touching
`CoordApp`.

This is the single thing to get right architecturally: **fidelity
(server layout + server hit-test) and adaptivity (browser layout) pull in
opposite directions; quadraweb's roadmap is the controlled migration of
the cut upward.**

---

## 8. Mobile / Tailscale specifics

- **No TLS needed for transport** — Tailscale encrypts; `ws://<ts-ip>:port`
  works. **But** the async Clipboard API requires a *secure context*, and
  plain `http://` to a private IP is not one. `PlatformServices::clipboard()`
  will be limited on web; fall back to `execCommand('copy')` or defer
  clipboard. (Web cousin of the TUI clipboard-env trap.)
- **Touch → mouse mapping happens in the browser** before sending:
  tap = down+up, long-press = right-click / context menu, drag =
  `MouseMoved` + button mask, two-finger = `Scroll`.
- **Soft keyboard:** focus a hidden `<input>` when a quadraui
  `TextInput`/`Editor` gains focus; relay `CharTyped` / `KeyPressed`.
- **Single client.** One `CoordApp`, one socket. A second browser mirrors
  the same frame or is rejected. No multi-tenant until needed.

---

## 9. Phased roadmap

- **Phase 0 — transport spike (throwaway).** xterm.js, or even a `<pre>`
  cell dump, over a WebSocket. Goal: *see and click the live `CoordApp`
  from my phone over Tailscale.* De-risks the network only; deleted once
  Phase 1 lands.
- **Phase 1 — display-list MVP.** `WebBackend` accumulates `Vec<WebItem>`
  via serde and ships it; minimal TS renderer absolutely-positions each
  primitive at the server rect (scaled px); browser sends raw
  pointer/key/scroll; server hit-tests with the existing `FrameHitMap`.
  `CoordApp` unchanged. Real web widgets, fixed layout.
- **Phase 2 — adaptivity.** Make the web runner AppShell-zone-aware; CSS
  arranges zones; narrow viewport collapses sidebar → drawer. Hit-testing
  for re-arranged chrome moves browser-side (per §7).
- **Phase 3 — polish.** Per-primitive responsive variants (tree →
  accordion on mobile), touch gestures, soft-keyboard handling, `Theme` →
  CSS variables, frame diffing if bandwidth demands it.

Repo shape: `quadraui/src/web/{backend,run,shell_runner}.rs` + a
`quadraui/web-frontend/` dir (TS/HTML/build), behind a `web` cargo
feature — structurally a peer of `src/gtk/` and `src/macos/`.

---

## 10. Relationship to the portability commitment

CLAUDE.md's commitment: *a future agent should be able to write a whole
backend by implementing the `Backend` trait, with zero consumer changes.*
quadraweb honours the consumer half completely (`CoordApp` is untouched)
and the trait half on the Rust side (it implements `Backend`). The bend it
accepts: **the per-primitive rasteriser is written a second time, in
TypeScript, because the renderer runs in the browser.** That is the
deliberate, allowed exception ("the web code does not have to honour
working across all backends with no modifications"). The WASM-canvas
alternative would avoid even that bend — recorded in §11 as the rejected
option, available to revisit if a future native canvas backend makes a
shared Rust rasteriser pay for itself.

---

## 11. Decisions log

- **D1 — Renderer: DOM/TypeScript, phased. RESOLVED (2026-06-02).**
  Chosen over xterm.js (terminal-grid, anti-goal), WASM-canvas (faithful
  + Rust-pure but weak adaptivity/accessibility/mobile-text), and htmx
  (request/response model fights a live event loop). Rationale: best fit
  for adaptive + phone-first + app-unchanged; cost (TS rasteriser
  duplication) is acceptable under the §10 exception.
- **D2 — Transport: WebSocket, single client. RESOLVED (2026-06-02).**
  Frames down, raw events up. Multi-client deferred.
- **D3 — Hit-testing stays server-side in Phase 1. RESOLVED (2026-06-02).**
  Reuse `FrameHitMap`; browser sends raw coords. Migrates browser-side
  zone by zone in Phase 2 (§7).
- **D4 — Web metrics: pixel-ish, GTK-like. RESOLVED (2026-06-02).**
  `line_height()`/`char_width()` return CSS px; reuse shared `*Measure`
  helpers for layout. The `macos` backend proves the pixel-metric path.

**Open questions**

- Q1 — Frontend stack: vanilla TS + Web Components, or a micro-framework
  (lit / preact)? Leaning vanilla for v0 to keep the build trivial.
- Q2 — Theme bridge: map `Theme` → CSS custom properties on the server,
  or ship `Theme` and resolve in the browser? (Leaning: ship serde
  `Theme`, resolve to CSS vars in the browser.)
- Q3 — Does any non-serde type in `primitives/mod.rs` need to cross the
  wire? (Audit during Phase 1.)
- Q4 — Frame diffing: needed for a phone link, or is full-frame send
  adequate at dashboard Hz? (Measure in Phase 1 before building.)

---

## 12. References

- `BACKEND_TRAIT_PROPOSAL.md` — the `Backend` trait + `UiEvent` contract
  quadraweb implements (esp. §4 trait shape, §6.1 `AnyPrimitive`, §6.2
  where layout lives).
- `ARCHITECTURE.md` — workspace layout, two-layer split, compose helpers.
- `quadraui/src/frame.rs` — `ScreenLayout` / `FrameHitMap` (server-side
  hit-test reused by the web backend).
- `quadraui/src/tui/run.rs` — the runner loop quadraweb mirrors.
- `coord/tui/src/{main.rs,bin/gtk.rs}` — the ~10-line shims a web shim joins.
