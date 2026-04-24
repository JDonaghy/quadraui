# How vimcode's TUI consumes quadraui — a code tour

This document is a reading guide for understanding how the TUI backend
consumes `quadraui` primitives, aimed at a reader who wants to
understand the pattern before extending it (to GTK, Win-GUI, macOS, or
a non-vimcode app).

Built after Session 328's Phase B.4 chrome migration arc — every
example below points at code that shipped during that arc, so the
commits are small and readable in isolation.

---

## What the contract actually is (D6 in one paragraph)

A **primitive** (e.g. `StatusBar`, `Tooltip`, `ListView`) has:

1. A **declarative struct** (e.g. `Tooltip { id, text, placement, … }`)
   describing *what* should appear, with no coordinate information.
2. A **`layout(...)` method** that takes a viewport + a measurement
   closure and returns a fully-resolved `Layout` struct containing
   every coordinate the backend needs to paint.
3. A **`hit_test(x, y)` method** on the layout that resolves a click
   to an event variant (e.g. `StatusBarHit::Segment(id)` or
   `DialogHit::Button(id)`).

Backends are **paint-only**: they receive `&Primitive + &Layout` and
write pixels / cells into their native surface. They do **not** make
layout decisions. This is Decision D6 (resolved 2026-04-22) —
`quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §9 D6 has the full
rationale.

The payoff: paint and click resolution derive from the same `Layout`
data, so "paint position drifts from hit-test position" bugs (a
recurring class in the pre-D6 era) are eliminated by construction.

---

## 1. Start with the contract: one primitive, end-to-end

Pick the **hover popup → Tooltip** pair. Smallest complete loop.

### The primitive itself

- [`quadraui/src/primitives/tooltip.rs`](../src/primitives/tooltip.rs)
  (~200 lines, readable in one sitting)
  - `Tooltip` struct — the declarative description
  - `TooltipMeasure` / `TooltipLayout` — the D6 resolved layout
  - `Tooltip::layout(anchor, viewport, measure, margin) -> TooltipLayout`
    — **the core contract**: given a world + a measurement, return
    fully-resolved coordinates. No backend code here, no painting.

### How an engine state gets adapted into it

- [`src/render.rs::hover_popup_to_quadraui_tooltip`](../../src/render.rs) —
  converts engine-side `HoverPopup { text, anchor_line, anchor_col }`
  into `(Tooltip, TooltipLayout)`. Pure data transform.

### How the TUI paints it

- [`src/tui_main/quadraui_tui.rs::draw_tooltip`](../../src/tui_main/quadraui_tui.rs) —
  takes `&Tooltip + &TooltipLayout + &Theme`, writes cells into a
  ratatui Buffer. Doesn't decide placement (the layout already has
  that) — just rasterises.

### Where it's wired

- [`src/tui_main/render_impl.rs`](../../src/tui_main/render_impl.rs) —
  search for `screen.hover` (around line 476). Three steps: build
  viewport `Rect`, call adapter, call rasteriser. That's the whole
  pattern.

**Read these four files in order and you've seen the whole D6 contract once.**

---

## 2. Add click handling: the StatusBar pattern

**Why StatusBar first for clicks:** the primitive carries `action_id`
per segment, and clicks route through `bar.resolve_click(col,
bar_width)` instead of per-backend column math.

### The primitive

- [`quadraui/src/primitives/status_bar.rs`](../src/primitives/status_bar.rs)
  — whole file worth reading. Pay attention to `resolve_click`:
  **the primitive owns the hit math**.

### The adapter

- [`src/render.rs::debug_toolbar_to_quadraui_status_bar`](../../src/render.rs) —
  each debug button = 2 segments (icon + dim hint) sharing an
  `action_id = "debug:btn:N"`.
- [`src/render.rs::debug_toolbar_action_index`](../../src/render.rs) —
  decodes the `WidgetId` back to a `DEBUG_BUTTONS` index.

### The rasteriser

- [`src/tui_main/quadraui_tui.rs::draw_status_bar`](../../src/tui_main/quadraui_tui.rs) —
  paints `layout.visible_segments`; that's it.

### Click resolution

- [`src/tui_main/mouse.rs`](../../src/tui_main/mouse.rs) — search for
  `debug_toolbar_visible` (around line 1823). Build the bar, call
  `bar.resolve_click(local_col, bar_w)`, decode the id, dispatch.
  **Paint and hit-test derive from the same bar.** Drift bugs
  impossible by construction.

### Same pattern, second example

Breadcrumb bar uses the same StatusBar pattern with different semantics
(segments are path components; action_id `"bc:N"` for clickable
segments, `None` for `" › "` separators):

- [`src/render.rs::breadcrumbs_to_quadraui_status_bar`](../../src/render.rs)
- [`src/tui_main/mouse.rs`](../../src/tui_main/mouse.rs) — search for
  `breadcrumb click` (around line 2641).

---

## 3. Primitive extension: when the existing shape isn't enough

Real primitives grow when real consumers need something the original
shape didn't cover. The extension pattern is always: struct gains a
field, tests update, rasteriser gains a branch, adapters use it.

### Example: Tooltip gains multi-line styled content

- **Commit `e6048d8`** — Tooltip gains `styled: Option<StyledText>`
  for single-line styled spans (signature help needed the active
  parameter highlighted in keyword color).
- **Commit `e4ae90e`** — rename to `styled_lines: Option<Vec<StyledText>>`
  for multi-line (diff peek needed per-line `+`/`-` coloring in the
  hunk body).

### Example: ListView gains bordered mode

- **Commit `85841d2`** — `ListView.bordered: bool`. When `true`,
  layout insets items by 1 cell each side and reserves rows 0 + N-1
  for `╭─╮ ╰─╯` borders; title (when present) overlays the top
  border. Tab switcher modal needs it; quickfix flat panel doesn't.

**Read these commits to see how extensions stay small:** each is
scoped to one primitive + its existing consumers + one new consumer.
No cross-cutting changes.

---

## 4. When the primitive model doesn't fit: the `hit_regions` pattern

Not everything has to be a primitive. The **find/replace overlay** is
the escape-hatch case: heterogeneous `[adornment…] [TextInput]
[adornment…]` rows, 1–2 stacked inside a single bordered box. Nothing
in the primitive set (Form, Dialog, StatusBar, Palette) fits cleanly,
and a speculative new `Toolbar` primitive isn't justified yet (no
second consumer).

**The alternative:** the **data shape itself** is the cross-backend
contract. `FindReplacePanel + FrHitRegion` live in the engine, every
backend walks them for both painting and click resolution.

- [`src/core/engine/mod.rs::compute_find_replace_hit_regions`](../../src/core/engine/mod.rs) —
  engine owns the layout math (lines 1101–1211).
- [`src/tui_main/quadraui_tui.rs::draw_find_replace`](../../src/tui_main/quadraui_tui.rs) —
  walks `panel.hit_regions`, dispatches per `FindReplaceClickTarget`
  variant to paint each region.
- [`src/tui_main/mouse.rs`](../../src/tui_main/mouse.rs) — click routes
  through the same `hit_regions` list via
  `Engine::handle_find_replace_click(target)`.

**The takeaway:** D6 is "one data structure owns layout for both
paint and hit-test." A **quadraui primitive** is one *instance* of
that pattern, but not the only valid shape. If the engine already has
a portable layout struct that works across backends, that IS the
contract.

---

## 5. The decision log (why things are the way they are)

- [`quadraui/docs/DECISIONS.md`](DECISIONS.md) — primitive-distinctness
  principles ("why ListView ≠ TreeView", "what belongs as Decoration
  vs a separate field"). Read before adding a new primitive.
- [`quadraui/docs/BACKEND_TRAIT_PROPOSAL.md`](BACKEND_TRAIT_PROPOSAL.md)
  §9 — D1–D7 resolved decisions with rationale. **D6** is the one
  that drives every migration in this tour.

---

## 6. Optional: one full-feature primitive end-to-end

If you want one example that shows **every** concept (layout,
hit-test, scroll, per-item measurement, rendering options, tests),
read [`quadraui/src/primitives/list.rs`](../src/primitives/list.rs)
end-to-end. ~300 lines. Covers the full shape a mature primitive
takes.

---

## Suggested reading order

Roughly 1–2 hours to genuinely understand:

1. [`quadraui/src/primitives/status_bar.rs`](../src/primitives/status_bar.rs)
   — full file (the shortest complete example of a primitive with
   clicks).
2. `debug_toolbar_to_quadraui_status_bar` adapter +
   `draw_status_bar` rasteriser + its click handler in `mouse.rs`
   (see §2 above).
3. [`quadraui/src/primitives/tooltip.rs`](../src/primitives/tooltip.rs)
   + `hover_popup_to_quadraui_tooltip` adapter (see §1).
4. D1–D7 in [`BACKEND_TRAIT_PROPOSAL.md`](BACKEND_TRAIT_PROPOSAL.md)
   §9 — especially D6.
5. `draw_find_replace` as the "escape hatch" case (see §4).

## After the tour

If you're porting a new backend (GTK / Win-GUI / macOS / web), start
with one primitive. For each primitive in that backend:

1. The **primitive** already exists — reuse verbatim.
2. The **adapter** in `src/render.rs` already exists — reuse
   verbatim.
3. Write a **new rasteriser** `quadraui_{backend}::draw_{primitive}`
   that takes `&Primitive + &Layout + &Theme + &NativeSurface` and
   paints it. Dimensions are in the backend's native unit (pixels for
   GTK, not cells) — pass those through to the primitive's `layout()`
   call via your measurement closure.
4. Route clicks through the primitive's `hit_test(x, y)` method.

**The adapter + primitive are platform-neutral and always shared.**
Only the rasteriser + click-event plumbing are per-backend.

---

## History

- **Sessions 270–300:** Phase A primitives shipped (TreeView,
  ListView, Form, Palette, StatusBar, TabBar, ActivityBar, Terminal,
  TextDisplay).
- **Sessions 320–327:** D6 contract resolved, all 9 existing
  primitives gained `layout()` + `hit_test()`, 11 new Phase B.3
  primitives landed (Tooltip, ContextMenu, Completions, Dialog,
  Panel, Split, Modal, MenuBar, Toast, Spinner, ProgressBar).
- **Session 328 (2026-04-23):** Phase B.4 chrome migration arc —
  22 commits on develop migrating every major TUI overlay to a
  quadraui primitive or shared hit-region data. See PROJECT_STATE.md
  Session 328 entry for the per-migration commit list.
