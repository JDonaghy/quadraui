# Lessons Captured

Durable rules that came out of real failures. Each one is load-bearing —
read at session start; apply as you work. New lessons get appended
(date + one-line incident summary). Lessons don't get removed unless
they turn out wrong.

## Paint/click drift is the structural bug class quadraui exists to eliminate

When a primitive's paint code computes one set of coordinates and its
hit_test code computes another, every consumer eventually sees "I
clicked X but Y was selected" — and each consumer works around it in a
different ad-hoc way. The library's job is to make that impossible by
construction:

- **One layout, two consumers.** Paint and hit_test must consume the
  *same* `Layout` instance. Never let either side re-derive bounds.
- **Coordinate-system agreement is structural.** When a backend rounds
  to a discrete unit (TUI cells, integer pixels), the layout itself
  must snap to that unit *before* emitting bounds — not at paint time.
  See `LayoutMetrics::cell_quantum` for the TUI case.
- **Round-trip coverage proves agreement.** A test that paints, finds
  the painted region, and hit_tests at that exact position is the only
  test that catches drift across paint/hit_test formulae. Unit tests of
  either side alone will miss it.

## The band-aid trap

When a consumer hits a paint/click drift bug mid-migration, the
tempting "fix" is to cache layout *inputs* on the consumer's state
(`Cell<T>` or similar) so click re-derives the same layout paint
derived. **This perpetuates the bug class.** Two code paths still
derive independently; if they ever diverge in inputs *or* in
derivation, the bug returns in a new shape.

The structural fix is always one derivation consumed by both paint
and click. Two ways to achieve this:

1. **`Backend::*_layout()` re-derivation** — safe when inputs are
   guaranteed identical (AppLogic runner pattern).
2. **Cache the layout output** — paint computes the layout, stores
   it (`RefCell<Option<Layout>>`); click reads it verbatim. Safe
   when paint and click run in different contexts and inputs may
   differ (vimcode's `terminal.draw()` closure vs event loop).

Option 2 is **not** Cell-smuggling. Cell-smuggling caches *inputs*
to bridge two independent derivations. Layout caching stores the
*output* of one derivation — there is no second derivation to
diverge.

## Theory-only iteration doesn't converge

When a migration breaks and the agent can't run the consumer (common
with TUI apps needing a real terminal), each "plausibly correct from
code reading" fix ships a new bug. The only escape is a harness that
exercises paint->hit_test agreement automatically, in unit-test time,
without a human in the loop. **The harness is the gate.** Don't ship
a primitive change or a consumer migration without one.

## Migration discipline (corollary)

Migrating a consumer onto a primitive is a contract change, not a
refactor. The consumer commits to the primitive's `layout()` as the
source of truth for widget bounds. Every migration MUST add a
*consumer-state* round-trip test before merging:

1. Paint the consumer's MSV / TreeView / etc. into a buffer.
2. Find painted regions in that buffer.
3. Simulate the consumer's click handler at those coordinates.
4. Assert consumer-state mutation matches the painted UI — right
   section activated, right item selected, right scroll offset moved.

A green primitive test does not prove the consumer integration is
correct.

## Shared AppLogic code must not hardcode backend-native units

The first shared `AppLogic` refactor (#14) shipped with
`STATUS_BAR_PX = 24.0` — a pixel-based constant that zeroed out
the sidebar on TUI (viewport is ~24 cells, so `24 - 24 = 0`).
The fix: `backend.line_height() * 1.5` — portable across cells,
pixels, and DIPs.

Anywhere a shared `AppLogic` computes a rect size or position,
it MUST derive from `backend.line_height()`, `backend.viewport()`,
or the layout returned by `backend.msv_layout(...)` /
`backend.tree_layout(...)`. **No hardcoded px / cell / DIP
constants in shared render or event-handling code.** Constants
belong on the backend, not on the consumer.

## All runners must fire all UiEvent variants the consumer pattern needs

GTK's runner had a `gdk_motion_to_uievent` translator helper but
never wired an `EventControllerMotion` to fire it. So
`UiEvent::MouseMoved` (needed for drag tracking) was a dead path
on GTK. Fixed in #14 by adding the motion controller to `gtk::run`.

When adding a consumer pattern that consumes a particular
`UiEvent` variant, verify that **every runner** (TUI, GTK, and
future Win-GUI / macOS) actually produces that event.

## Backend `_layout` methods must work outside the frame scope

GTK's `handle()` runs from signal handlers (click, key, motion)
which fire **outside** the `draw` callback. The Cairo `Context`
and `pango::Layout` only exist inside the draw callback (the
"frame scope"). So `Backend::foo_layout(&self, ...)` methods —
which apps call from `handle()` to hit-test clicks — **cannot
use pango or cairo handles**.

Use stored metrics instead: `current_line_height`,
`current_char_width`, etc. These are set once per
`set_current_line_height` / `set_current_char_width` call and
remain valid across the entire event cycle.

The `draw_*(&mut self, ...)` methods *can* use the frame scope
because they're called from `render()`, which runs inside the
draw callback. This asymmetry is structural to GTK's rendering
model and will apply to any retained-mode backend (Win-GUI,
macOS). TUI doesn't have this constraint because ratatui's
`Frame` is available throughout the event loop.

First hit: `menu_bar_layout` panicked on GTK because it called
`current_frame_refs()` from a click handler. Fixed by switching
to `current_char_width`-based measurement.

## Real apps need layout caching, not layout re-derivation

The `Backend::*_layout()` methods (e.g. `msv_layout`, `tree_layout`,
`menu_bar_layout`) exist so click handlers can get the same layout
paint used — **but only when inputs are guaranteed identical.**
In practice, real apps beyond the demo stage often can't guarantee
this: paint runs inside ratatui's `terminal.draw(|frame| ...)` closure
or GTK's draw callback, click runs in the event loop outside it.

The fix: cache the layout paint produced, read it in click.
`RefCell<Option<MultiSectionViewLayout>>` on the host state, set
by paint, consumed by click. One derivation, zero drift — by
construction.

This is NOT the "band-aid trap" (caching inputs to bridge two
independent derivations). Layout caching stores the output of one
derivation. There is no second derivation that can diverge.

## Backend draw_* and *_layout must agree on which dimensions they use

When a `Backend` trait method pair (`draw_foo` / `foo_layout`) uses
different dimensions for the same parameter, paint and hit-testing
diverge. First hit: GTK's `draw_menu_bar` passed
`self.current_line_height` to the rasteriser while `menu_bar_layout`
used `rect.height`. When consumers passed a bar_rect taller than
line_height, the dropdown anchored below the full rect in hit-testing
but painted at line_height — producing a gap and broken clicks.

Rule: for every `(draw_foo, foo_layout)` pair, audit that they
pass the same rect/height/width to the underlying layout
computation.

## Layout helpers must return coords in the same frame across backends

A `*_layout()` helper's `hit_regions` and `visible_*.bounds` must be
in the **same coordinate frame** as its TUI/GTK twin — typically
*local to the layout area* (origin `(0, 0)`), so the consumer
subtracts `area.x` / `area.y` from absolute click coords before
calling `hit_test`.

First hit: `mac_tree_layout` and `mac_form_layout` both shifted
their hit_regions by `(area.x, area.y)` to absolute coords (so the
internal paint loop could iterate `bounds` directly). TUI/GTK twins
returned local coords, and every consumer (`tree_controller` compose
helper, `SidebarSystem`, AppLogic) localised position before
`hit_test` per the documented contract — so on macOS the click
drifted by `area.y`. Surfaced by `macos_search_panel` (#44) — the
first non-zero-offset consumer of `mac_tree_layout`. Existing tests
all used `area=(0, 0)` so the shift was a no-op and the bug
invisible.

Rule: layout helpers return local-frame coords. Paint loops shift
inline (`bounds.x + area.x`). Every layout helper needs at least
one regression test using non-zero `area.y` — `area=(0, 0)` is the
case where the bug doesn't manifest.

## Dropdown item sizing must use backend-native units

`MenuSystem::dropdown_layout()` computes item heights from
`backend.line_height()`. Adding a constant (`lh + 4.0`) assumes
pixel units — in TUI where `lh = 1.0` cell, `lh + 4.0 = 5.0`
cells per item. Multiplicative scaling (`lh * 1.4`) is safe
across unit systems, but fractional values (1.4 cells) cause
items to land on non-integer rows with blank gaps. Fix: round
to the nearest integer (`(lh * 1.4).round()`). For TUI this
snaps to 1 cell; for GTK the rounding is sub-pixel and invisible.

## What NOT to do

- **Don't re-derive layouts with different inputs in click vs paint.**
  Two independent layout computations that can diverge is the
  structural bug class quadraui eliminates. Two safe patterns:

  1. **AppLogic runner (demos, simple apps):** `render()` and
     `handle()` call the same `backend.*_layout()` with identical
     inputs. Works when app state hasn't changed between render
     and handle.

  2. **Layout caching (vimcode, complex apps):** Paint stores the
     layout output (`RefCell<Option<Layout>>`), click reads it
     verbatim. One derivation, two consumers. NOT Cell-smuggling —
     smuggling caches *inputs* to bridge two derivations; layout
     caching stores the *output* of one.

- **Don't import vimcode-specific patterns.** The library is pre-1.0
  but we still want it API-stable enough that downstream consumers
  besides vimcode can adopt it.

- **Don't migrate a consumer onto a primitive without writing the
  paint/click harness first.** The harness is the gate.
