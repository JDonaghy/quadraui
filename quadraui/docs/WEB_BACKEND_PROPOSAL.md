# `quadraweb` — web backend proposal

A browser backend for quadraui: take an existing backend-generic
quadraui app (`AppLogic` / `ShellApp`) and run it in a phone or
desktop browser with **near-zero hand-written JavaScript**, an
**adaptive** (not terminal-grid) layout, and **no app changes**.

First consumer + acceptance target: **claude-coordinator's `CoordApp`**,
viewable and usable from a phone browser over Tailscale (no TLS, no
external hosting).

Status: **proposal**. Tracked by the `quadraweb` epic (#314); phases
#315 (spike), #316 (MVP), #317 (adaptivity), #318 (polish).

---

## TL;DR

- A web port is **not an app rewrite**. `CoordApp` is already a
  `quadraui::ShellApp`; its TUI and GTK entry points are ~10-line shims
  over `run_with_shell(...)`. quadraweb adds a *third shim*. Same
  `win`/`macos` story — `src/win/` is only 4 files.
- **Chosen model: lean, server-rendered semantic HTML.** The Rust web
  backend renders each primitive to real semantic HTML (`<button>`,
  `<a>`, `<form>`, `<input>`, `<ul>`…) carrying `data-widget-id`s, and
  pushes it over a WebSocket. The browser is a **generic, write-once
  client**: hold the socket, swap incoming HTML fragments, forward
  inputs. No per-primitive JavaScript, no framework.
- **The load-bearing mechanism — *identity-in, coordinate-reconstructed*.**
  The browser resolves *what* was hit for free (the clicked element *is*
  the target — `event.target.dataset.widgetId`). The server reconstructs
  *where* by mapping that id back to the rect it computed during render,
  and hands `CoordApp` a normal `UiEvent` with a coherent `position`. So
  the app's existing coordinate-based routing keeps working **unchanged**,
  while layout + rendering move to the browser.
- **Why this over a TypeScript display-list renderer (the rejected D1):**
  rendering logic stays in **Rust** (closer to the portability
  commitment — no per-primitive rasteriser duplicated in TS); native HTML
  controls handle typing / scroll / focus / soft-keyboard **locally**
  (less JS, feels instant); accessibility comes free; and the
  fidelity↔adaptivity tension largely *dissolves* because the browser
  owns layout and still knows what was clicked.
- **Cost accepted:** the web view is an HTML rendering of the primitives,
  not pixel-identical to the native backends. The project explicitly
  doesn't need it to "feel that modern," so this is a feature, not a
  regression. A few primitives resist semantic HTML (`Chart`, `DiffView`,
  `Terminal`, `Editor`) and keep a small canvas/JS island when their turn
  comes — none are central to `CoordApp`.

---

## 1. Goals & non-goals

**Goals**

1. Run an unmodified backend-generic quadraui app in a browser.
2. Adaptive layout — re-flows and re-arranges for screen size (sidebar →
   drawer), rather than emulating a fixed terminal grid.
3. Phone-first, over Tailscale: `ws://<tailscale-ip>:<port>`, no certs,
   single user, single client.
4. **Minimal hand-written JS.** The browser is a generic client; all
   rendering and app logic live in Rust on the server.

**Non-goals (for v0)**

- Competing with Flutter/React on visual polish. "Doesn't need to feel
  that modern" is a stated requirement.
- TLS, auth, multi-tenant, or public hosting. Tailscale is the boundary.
- Pixel-identical fidelity with the native backends. The web view is
  semantic HTML, not a screenshot of GTK.
- Compiling the app to WASM. The app runs on the server (SQLite, spawns
  subprocesses); only the *transport + a dumb client* live in the browser.

---

## 2. The three enabling facts (why this is cheap)

### 2.1 The app is already backend-generic

`CoordApp` implements `ShellApp`. Its entry points are ~10-line shims:

```rust
// coord/tui/src/main.rs
quadraui::tui::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
// coord/tui/src/bin/web.rs  (new)
quadraui::web::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
```

No changes to `CoordApp`, its render path, or its event handling.

### 2.2 Every interactive widget already has an identity

The app assigns `WidgetId`s to interactive primitives — status-bar
segments (`action_id`), toolbar/button widgets, tab closes, tree/list
rows, activity-bar items. Those identities are exactly what semantic
HTML needs: each becomes a `data-widget-id`, and the browser hit-tests
**for free** by DOM. No pixel hit-map to ship, no "browser must match
server metrics" constraint. (Serde on primitives — still derived — is no
longer the *transport*; the transport is HTML. Serde stays for
`quadraui-ipc`/`quadraui-lua`.)

### 2.3 The server still computes layout — repurposed for coordinates

`ScreenLayout::draw` + the `*_layout()` methods compute each primitive's
rect with server metrics. In the display-list model that drove
server-side *pixel hit-testing*; in the lean model it drives
**coordinate reconstruction** (§4): id → server-rect → synthesized
`UiEvent.position`. The layout machinery isn't discarded — it's used in
reverse so the app's coordinate-based `handle()` keeps working.

---

## 3. The abstraction-cut spectrum (and the chosen cut)

"xterm.js vs alternatives" is one question: **at what level do we cut the
abstraction for the browser?** The cut determines adaptivity *and* JS volume.

| Cut point | Ships to browser | Adaptivity | Hand-written JS | App change |
|---|---|---|---|---|
| Cell grid (xterm.js) | ANSI / cell buffer | None — it *is* a terminal | low (xterm does it) | None |
| Positioned primitives (JSON display list) | `[{rect, prim}]` | Reflows to width | medium (TS component per primitive) | None |
| **Semantic HTML (CHOSEN)** | semantic HTML + ids | Genuine (CSS-driven) | **near-zero, generic** | None |
| Domain model (rewrite) | app data | Fully native | a whole SPA | Total rewrite |

xterm is the terminal-grid anti-goal. The JSON-display-list + TS-renderer
cut (the previously-chosen D1) works but pushes a per-primitive renderer
into the browser. **The chosen cut is semantic HTML**: it maximises
adaptivity (CSS owns layout) and minimises JS (the browser renders
nothing bespoke), while keeping rendering logic in Rust.

---

## 4. Architecture

```
Server (Rust)                                  Browser (generic client)
─────────────                                  ────────────────────────
CoordApp (unchanged)
  draw_tree(rect, &tree) ─┐ each draw_* emits
  draw_list(rect, &list) ─┤ semantic HTML +
  draw_form(rect, &form) ─┤ records id → rect
WebBackend.end_frame  ────┘
        │  ws ▼  HTML fragment(s)
        │                                       swap fragment(s) into DOM
        │                                       (CSS arranges zones;
        │                                        sidebar→drawer on narrow)
        ▲  ws ▲  { activate, widgetId, offset? }
        │
  id → server-rect → UiEvent{ widget:Some(id), position } → app.handle
```

### 4.1 The runner loop (same structure as `tui::run`)

A blocking loop with a network in the middle: an HTTP server (axum or
similar) serves the static client and upgrades a WebSocket; a reader task
feeds an `mpsc<UiEvent>` (`wait_events` = `recv_timeout`, `poll_events` =
`try_recv`); `end_frame` pushes HTML down the socket; `begin_frame` takes
the `Viewport` from the browser's reported size so a resize / rotation
re-lays-out for free. `AppLogic`/`ShellApp`/`Reaction` are unchanged.

### 4.2 `WebBackend` — semantic-HTML emitter

Each `draw_*` method renders its primitive to semantic HTML and records
the primitive's rect under each emitted `data-widget-id` in a per-frame
**id → rect map**. Draw order = DOM order, so z-order (modals, toasts,
tooltips pushed last by `ScreenLayout`) maps to stacking order directly.
The layout-returning methods (`tree_layout`, `status_bar_layout`, …) still
return real layouts (web metrics ≈ CSS px) — both for the app's own use
and to populate the id → rect map.

### 4.3 The browser client — generic, write-once

The entire client: open the WebSocket, apply incoming HTML (full document
on first frame, fragment swaps by element id thereafter), and a single
**delegated** listener that, on interaction, walks to the nearest
`[data-widget-id]` and sends `{ activate, widgetId, offset? }`. Native
`<input>`/`<textarea>` handle typing/IME/soft-keyboard locally and send
`{ input, widgetId, value }` on change. Target: low hundreds of lines,
**no per-primitive code, no framework.**

### 4.4 Identity-in, coordinate-reconstructed (the key mechanism)

The browser knows *what* (the element's `data-widget-id`) and optionally a
within-element `offset`. The web backend maps the id to the rect it
recorded during render and synthesizes `UiEvent::MouseDown { widget:
Some(id), position: rect.origin + offset }`. `CoordApp`'s existing routing
— both `ShellContext::in_sidebar(x,y)` zone checks and per-primitive
hit-region dispatch — sees a coherent coordinate and runs **unchanged**,
even though the browser's *visual* layout (a drawer, a reflowed column)
diverged from the server's. Nothing visual depends on the server layout
anymore; it exists only to manufacture consistent event coordinates.

---

## 5. Wire format (v0)

**Down (server → browser):** semantic HTML.

```jsonc
// first frame
{ "v": 1, "frame": "full", "html": "<main id=app> … </main>",
  "theme": { /* serde Theme → applied as CSS custom properties */ } }
// subsequent frames — targeted fragment swaps by element id
{ "v": 1, "frame": "patch", "swaps": [ { "id": "sidebar", "html": "<aside id=sidebar>…</aside>" } ] }
```

(htmx's WS/SSE swap conventions are a drop-in for the patch envelope; a
~40-line custom swap loop is the alternative. Either is the whole "framework".)

**Up (browser → server):** identity-based.

```jsonc
{ "kind": "activate", "widgetId": "tree.node.42", "offset": { "x": 6, "y": 2 }, "mods": {…} }
{ "kind": "input",    "widgetId": "filter.field", "value": "deploy" }
{ "kind": "key",      "key": "Enter", "mods": {…} }
{ "kind": "resize",   "width": 390, "height": 780, "scale": 2.0 }
{ "kind": "pointer",  "x": 120, "y": 44, "phase": "down", "button": "left" } // raw fallback
```

The backend turns `activate`/`input`/`key` into `UiEvent`s (reconstructing
`position` per §4.4). The raw `pointer` form is a fallback for the few
primitives with intra-element positional meaning that need true
coordinates (see Q1).

---

## 6. Adaptivity is a CSS problem, not a JS one

The adaptive goal needs **no JavaScript**: media/container queries,
flexbox/grid, `:has()`, and the `<details>`/checkbox pattern give
sidebar↔drawer, reflow, and collapse natively. The web runner is
AppShell-zone-aware: it emits named semantic containers (`<aside
id=sidebar>`, `<main>`, `<footer id=status>`, …) and a stylesheet arranges
them — wide screens place them at the server's zone rects, narrow screens
override to a drawer/sheet. Because hit-testing is identity-based (§4.4),
**there is no fidelity↔adaptivity tension to manage**: the browser may
re-arrange a zone freely and the server still reconstructs a coherent
coordinate from the clicked id. (This is the central simplification the
lean model buys over the display-list cut.)

---

## 7. Mobile / Tailscale specifics

- **No TLS needed for transport** — Tailscale encrypts; `ws://<ts-ip>:port`
  works. **But** the async Clipboard API needs a *secure context*, and
  plain `http://` to a private IP is not one → `PlatformServices::clipboard()`
  is limited; fall back to `execCommand('copy')` or defer.
- **Native controls do the heavy lifting on touch.** Real `<input>`/
  `<textarea>` give the soft keyboard, IME, and selection with no JS;
  taps are native clicks; scroll is native (browser-owned, not synced to
  the server — see Q2). Only custom gestures (drag-to-resize, mouse
  text-selection) need bespoke JS, and most are droppable on a phone.
- **Single client.** One `CoordApp`, one socket. A second browser mirrors
  or is rejected. No multi-tenant until needed.

---

## 8. Phased roadmap

- **Phase 0 — transport spike (throwaway).** xterm.js, or a `<pre>` cell
  dump, over a WebSocket. Goal: *see and click the live `CoordApp` from my
  phone over Tailscale.* De-risks the network only. Best done by hand (a
  worker can't verify it on your phone). Deleted once Phase 1 lands.
- **Phase 1 — semantic-HTML MVP.** `WebBackend` emits semantic HTML per
  primitive + the id→rect map; WebSocket runner; generic browser client;
  identity-in/coordinate-reconstructed events. `CoordApp` unchanged. Real
  HTML widgets, interactive, on a phone.
- **Phase 2 — adaptivity.** Zone-aware containers + a responsive
  stylesheet; sidebar→drawer, bottom panel→sheet on narrow viewports.
  (Mostly CSS — see §6.)
- **Phase 3 — polish.** Touch gestures, clipboard fallback, fragment-swap
  granularity tuning, and the canvas/JS islands for `Chart`/`DiffView`/
  `Terminal`/`Editor` if/when a consumer needs them on web.

Repo shape: `quadraui/src/web/{backend,run,shell_runner}.rs` + a small
`quadraui/web-frontend/` (one HTML page + one generic client script),
behind a `web` cargo feature — peer of `src/gtk/` and `src/macos/`.

---

## 9. Relationship to the portability commitment

CLAUDE.md's commitment: *a future agent writes a whole backend by
implementing the `Backend` trait, zero consumer changes.* quadraweb
honours the consumer half completely (`CoordApp` untouched) and the trait
half in Rust. The lean model **shrinks the bend** the earlier DOM/TS
design accepted: per-primitive rendering stays in **Rust** (HTML emitters
in `WebBackend`), not duplicated in TypeScript. The only browser code is a
generic, primitive-agnostic client.

---

## 10. Decisions log

- **D1 — Renderer: DOM/TypeScript display list. SUPERSEDED by D7
  (2026-06-02).** Originally chosen; replaced same day by the lean
  HTML-semantic model after weighing JS volume + portability. Kept for the
  record.
- **D2 — Transport: WebSocket, single client. RESOLVED (2026-06-02).**
- **D3 — Hit-testing is identity-based, with server-side coordinate
  reconstruction. RESOLVED (2026-06-02).** Supersedes the earlier
  "server-side pixel hit-test" plan; see §4.4.
- **D4 — Web metrics: pixel-ish, GTK-like. RESOLVED (2026-06-02).**
  Used for the app's own layout + the id→rect map, not for browser
  hit-testing.
- **D5 — Frontend stack: vanilla, generic, no framework. RESOLVED
  (2026-06-02).** A single write-once client script (swap + delegated
  input). **No per-primitive components** — that work lives server-side in
  Rust (D7). htmx is an acceptable drop-in for the swap layer.
- **D6 — Theme bridge: ship serde `Theme`, resolve to CSS custom
  properties in the browser. RESOLVED (2026-06-02).**
- **D7 — Rendering model: lean, server-rendered semantic HTML. RESOLVED
  (2026-06-02).** Each `draw_*` emits semantic HTML with `data-widget-id`s;
  the browser is a generic client. Rationale: near-zero hand-written JS,
  rendering stays Rust, native controls handle hard inputs, adaptivity
  becomes a pure-CSS concern (§6). Cost: HTML rendering, not pixel-fidelity
  with native backends — acceptable per non-goals.

**Open questions**

- Q1 — Pure-positional events (empty-space clicks, drag, text selection,
  editor caret placement, chart hover-x) have no `data-widget-id`. Use the
  raw `pointer` fallback + per-primitive offset translation? (Resolve per
  primitive; none block the `CoordApp` MVP.)
- Q2 — Scroll ownership: browser-native scroll (decouple from the server's
  scroll model) vs sync scroll offset back. Leaning browser-native for v0.
- Q3 — Fragment-swap granularity: which elements get stable ids for
  targeted patches, and full-frame vs per-zone diff. (Measure in Phase 1.)

---

## 11. References

- `BACKEND_TRAIT_PROPOSAL.md` — the `Backend` trait + `UiEvent` contract
  (esp. §4 trait shape, §6.2 where layout lives).
- `quadraui/src/frame.rs` — `ScreenLayout` (the layout reused for the
  id→rect map).
- `quadraui/src/tui/run.rs` — the runner loop quadraweb mirrors.
- `coord/tui/src/{main.rs,bin/gtk.rs}` — the ~10-line shims a web shim joins.
