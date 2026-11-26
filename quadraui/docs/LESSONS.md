# Lessons Captured

Durable rules that came out of real failures. Each one is load-bearing —
read at session start; apply as you work. New lessons get appended
(date + one-line incident summary). Lessons don't get removed unless
they turn out wrong; when the code a lesson names changes shape, the
rule is restated against the new shape.

The rules below are written against the widget contract. A widget is one
type that implements `Primitive` (layout and paint, `src/primitive.rs`)
and `Interactive` (behaviour, `src/interactive.rs`). It is painted
through `DrawsPrimitives::draw_dyn` or `Surface::Widget`, and routed
through a `FrameTree`. Backends are platform services and rasterisers;
they have no per-widget methods.

## Paint/click drift is the structural bug class quadraui exists to eliminate

When a widget's paint code computes one set of coordinates and its
hit-test code computes another, every consumer eventually sees "I
clicked X but Y was selected" — and each consumer works around it in a
different ad-hoc way. The library's job is to make that impossible by
construction:

- **One layout, every consumer.** `Primitive::layout` resolves
  `Self::Layout` once. `paint`, `paint_cells`, `Interactive::handle` and
  `Interactive::sub_regions` all receive that same value. None of them
  re-derives bounds on its own.
- **`layout` is pure.** Same `LayoutCtx`, same `rect`, same layout.
  `draw_dyn` records a second resolution of the painted layout into
  `PaintedWidgets`, and the routing engine re-lays out each `FrameTree`
  node when an event arrives; both rely on this. A layout that reads a
  clock, a counter or interior-mutable state breaks hit-testing in a way
  no paint test sees.
- **Coordinate-system agreement is structural.** When a backend rounds
  to a discrete unit (TUI cells, integer pixels), the layout itself must
  snap to that unit *before* emitting bounds — not at paint time. A
  layout branches on `ctx.units()` (`Units::Cells` on TUI) and resolves
  cell geometry against whole cells, so a fractional rect from a ratio
  split does not hit-test one cell wider than it painted.
- **Round-trip coverage proves agreement.** A test that paints, finds
  the painted region, and clicks at that exact position is the only test
  that catches drift across the paint and hit-test paths. Unit tests of
  either side alone will miss it.

## The band-aid trap

When a consumer hits a paint/click drift bug mid-migration, the
tempting "fix" is to cache layout *inputs* on the consumer's state
(`Cell<T>` or similar) so click re-derives the same layout paint
derived. **This perpetuates the bug class.** Two code paths still
derive independently; if they ever diverge in inputs *or* in
derivation, the bug returns in a new shape.

The structural fix is always one derivation consumed by both paint
and click. The contract gives three ways to get it:

1. **Let the routing engine do it.** Put the widget in the frame's
   `FrameTree` (`AppLogic::frame_tree`, usually
   `ScreenLayout::into_frame_tree`). The runtime lays it out against the
   backend's own metrics and hands that layout to `Interactive::handle`.
   The app writes no hit-test code at all.
2. **Re-derive with identical inputs.** `DrawsPrimitives::layout_primitive`
   resolves a layout exactly as `draw_primitive`/`draw_dyn` do (same row
   height, char width and text measurer). Safe when the widget value and
   the rect are the ones that painted.
3. **Read back the painted output.** `draw_dyn` returns the layout it
   painted, and `primitive::with_painted_widgets(|p| p.painted_layout(&id))`
   returns this frame's painted rect and layout for any identified
   widget. Safe when paint and click run in different contexts and
   inputs may differ (a host whose paint runs inside `terminal.draw()`
   and whose event loop runs outside it).

Option 3 is **not** Cell-smuggling. Cell-smuggling caches *inputs* to
bridge two independent derivations. Reading back the painted layout
uses the *output* of one derivation — there is no second derivation to
diverge.

## Theory-only iteration doesn't converge

When a migration breaks and the agent can't run the consumer (common
with TUI apps needing a real terminal), each "plausibly correct from
code reading" fix ships a new bug. The only escape is a harness that
exercises paint→hit-test agreement automatically, in unit-test time,
without a human in the loop. **The harness is the gate.** Don't ship
a widget change or a consumer migration without one. See
[`TESTING.md`](TESTING.md) for the drivers.

## Migration discipline (corollary)

Migrating a consumer onto a widget is a contract change, not a
refactor. The consumer commits to the widget's `layout()` as the
source of truth for its bounds. Every migration MUST add a
*consumer-state* round-trip test before merging:

1. Drive the consumer's `AppLogic` through `TuiDriver` (and the pixel
   drivers where the consumer runs there).
2. Locate the painted target (`find`, `find_bounds`, or
   `primitive::painted_sub_center` for a sub-part with no unique text).
3. Click, type or drag at that location.
4. Assert the consumer's state matches the painted UI — right
   section activated, right item selected, right scroll offset moved.

A green widget test does not prove the consumer integration is
correct.

## Shared app code must not hardcode backend-native units

The first shared `AppLogic` refactor (#14) shipped with
`STATUS_BAR_PX = 24.0` — a pixel-based constant that zeroed out
the sidebar on TUI (viewport is ~24 cells, so `24 - 24 = 0`).
The fix: `backend.line_height() * 1.5` — portable across cells,
pixels, and DIPs.

Anywhere shared app or widget code computes a rect size or position, it
MUST derive it from the metrics it was given: `LayoutCtx`'s
`line_height`, `chrome_line_height()`, `editor_line_height()`,
`text_width()` and `units()` inside a `Primitive::layout`, or
`backend.line_height()` and `backend.viewport()` in an `AppLogic`.
**No hardcoded px / cell / DIP constants in shared layout or
event-handling code.** Constants belong on the backend, not on the
consumer.

A `Px`/`Cell` newtype would not have caught this one or the dropdown
sizing incident below: both are "hardcoded a literal instead of
deriving", not type confusion. `docs/decisions/DECISIONS.md` D-016 is the
record of `Rect`/`Point`'s unit contract, and `quadraui/src/event.rs`'s
module doc states it on the types themselves.

## All runners must fire all UiEvent variants the consumer pattern needs

GTK's runner had a `gdk_motion_to_uievent` translator helper but
never wired an `EventControllerMotion` to fire it. So
`UiEvent::MouseMoved` (needed for drag tracking) was a dead path
on GTK. Fixed in #14 by adding the motion controller to `gtk::run`.

When adding a consumer pattern that consumes a particular
`UiEvent` variant, verify that **every runner** (TUI, GTK, macOS,
Win-GUI) actually produces that event. Hover and press tints depend on
`MouseMoved` too: the ambient pointer state `draw_dyn` paints with is
fed from the runner's motion events, so a runner that drops them paints
no hover at all.

## Layout must work outside the frame scope

GTK's `handle()` runs from signal handlers (click, key, motion)
which fire **outside** the `draw` callback. The Cairo `Context`
and the frame's `pango::Layout` only exist inside the draw callback
(the "frame scope").

Layout runs outside it. The routing engine lays out every `FrameTree`
node when an event arrives, and a host calls `layout_primitive` from its
click handler. So `Primitive::layout`, and every `LayoutCtx::text_width`
call it makes, must not need a frame-scope handle. The measurer
`DrawsPrimitives` attaches is `Backend::measure_text`, which on GTK
measures against the stored widget-realized `pango_ctx`, not the
per-frame layout. A backend's `measure_text` must keep that property:
it answers the same width inside and outside a frame.

Paint (`paint` / `paint_cells`) *can* use the frame scope, because it is
called from `render()`, inside the draw callback. This asymmetry is
structural to GTK's rendering model and applies to any retained-mode
backend (Win-GUI, macOS). TUI doesn't have this constraint because
ratatui's `Frame` is available throughout the event loop.

First hit: the GTK menu-bar layout panicked because it reached for the
frame's Pango layout from a click handler. Fixed by measuring with
stored metrics instead.

## Paint and hit-test must be handed the same rect

First hit: GTK's menu-bar paint passed `self.current_line_height` to the
rasteriser while its hit-test layout used `rect.height`. When consumers
passed a bar rect taller than the line height, the dropdown anchored
below the full rect in hit-testing but painted at line height —
producing a gap and broken clicks.

Under the contract a widget has one `layout(ctx, rect)`, so the remaining
place for this bug is the host: the rect a widget is painted at
(`Surface::Widget { rect, .. }`, or `draw_dyn(rect, ..)`) and the rect
its `FrameTree` node carries must be the same rect. Build both from one
place: a helper that returns the frame's `ScreenLayout` for a given rect,
which `render` draws and `frame_tree` turns into routing nodes with
`ScreenLayout::into_frame_tree`. When the rect depends on backend
metrics `frame_tree` cannot see (a pixel line height), `render` records
the rect it painted at (a `Cell<Rect>` on the app) and `frame_tree` reads
that back. That records paint's *output*, not an input to a second
derivation. Inside `layout`, size from `rect` and the `LayoutCtx`
metrics, never from a backend field a second caller cannot see.

## Every layout states its coordinate frame, and is tested off the origin

A layout's rects must be in a frame every consumer agrees on. First
hit: the macOS tree and form layouts shifted their hit regions by
`(area.x, area.y)` to absolute coordinates while their TUI/GTK twins
returned local ones, and every consumer localised the click before
hit-testing — so on macOS the click drifted by `area.y`. Surfaced by
`macos_search_panel` (#44), the first consumer with a non-zero
`area.y`; every earlier test used `area = (0, 0)`, where the shift is a
no-op and the bug is invisible.

The contract fixes the frames:

- `RoutedEvent::Pointer`'s `local` and `Interactive::sub_regions` are in
  the **widget's own local space** (origin at its painted rect).
- `Self::Layout` may bake in absolute geometry its `paint` needs. A
  layout that holds both converts once, in one helper (as
  `ShellViewLayout`'s `local` does), not at each call site.
- `PaintedWidgets` translates sub-regions to **absolute** space, the
  space a `UiEvent::MouseDown` arrives in.

Every widget needs at least one test laid out at a non-zero origin.
`area = (0, 0)` is exactly the case that hides a frame mix-up.

## Dropdown item sizing must use backend-native units

Menu dropdown rows compute their height from the line height. Adding
a constant (`lh + 4.0`) assumes pixel units — in TUI where `lh = 1.0`
cell, `lh + 4.0 = 5.0` cells per item. Multiplicative scaling
(`lh * 1.4`) is safe across unit systems, but fractional values (1.4
cells) cause items to land on non-integer rows with blank gaps. Fix:
round to the nearest integer (`(lh * 1.4).round()`). For TUI this
snaps to 1 cell; for GTK the rounding is sub-pixel and invisible.

## `[patch.crates-io]` in quadraui's manifest does not reach downstream consumers

Cargo only honors `[patch]` sections declared in the **root manifest of
the workspace actually being built**. A `[patch.crates-io]` entry in
quadraui's own `Cargo.toml` (e.g. the vendored `vt100` patch added for
#452) has **zero effect** on a separate top-level crate/workspace (like
vimcode) that depends on quadraui via path, git, or crates.io — even
though it fixes dependency-resolution conflicts *inside* quadraui's own
workspace. Proven by direct repro: a scratch crate depending on quadraui
(patched) + `ratatui = "=0.29"` still hit the identical
`unicode-width` resolution error quadraui's patch was meant to prevent.

Rule: if a fix requires a `[patch]` to make a downstream consumer's
build resolve, quadraui vendoring the patch is necessary but not
sufficient — the consumer's own root `Cargo.toml` must add a matching
`[patch.crates-io]` entry pointing at quadraui's vendored path/git
location. Document that companion requirement explicitly wherever the
quadraui-side patch is added; don't assume it propagates.

## Paint and measurement resolve the font from one `FontRole`

Incident (#416, #624): GTK chrome painted Nerd-Font icon glyphs through
a Pango layout whose font description was the bare `ui_font` (e.g.
`"Sans 11"`). Icon glyphs live in Private-Use-Area codepoints that no
ordinary UI font covers, so Pango's implicit per-character fallback
guessed at whichever installed font claimed that range — correct on one
machine, tofu or the wrong glyph on another, with no error anywhere.
Some rasterisers also painted in whatever font the editor content had
left on the shared layout.

A second failure hid behind the first. When one toolbar's paint moved to
the chrome font, its clicks broke: the hit-test layout still measured
with the old font, so the button the user clicked was no longer the one
the hit test reported. The test harness missed it, because with no Pango
context it measured `chars().count() * char_width`, which is right only
for a monospace font.

The rules:

- **One font per role, for paint and measurement alike.** A widget
  declares `Primitive::font_role()`. Paint resolves that role on the
  surface (`draw_dyn` switches an `Editor`-role widget onto the editor
  font), and `LayoutCtx::text_width(text, role)` measures through
  `Backend::measure_text(text, role)`, which resolves the same
  description. A widget never builds its own font description.
- **Chrome glyphs get an explicit fallback family.** On GTK the chrome
  role's description is `gtk::chrome_font_description(ui_font)`, which
  *appends* `Symbols Nerd Font` (`gtk::NERD_FONT_FALLBACK_FAMILY`) to the
  family list. Pango consults the appended family only for characters
  the primary one can't cover, so ordinary text is unaffected. Any GTK
  code that paints a glyph outside the common BMP range goes through
  `chrome_font_description` / `with_nerd_font_fallback`, never a bare
  `pango::FontDescription::from_string`.
- **Prefer production parity in the harness over a "close enough"
  measurement fallback.** `GtkDriver` seeds `pango_ctx` the way
  `gtk::run` does. A fallback that is accurate only for the font you
  happened to be using is a test that stops testing the moment the font
  changes.

## Ambient hover is per-thread state; reset it in tests that read tints

`draw_dyn(.., &())` paints with the ambient pointer state
(`interaction::ambient_interaction()`), which the runtime updates from
pointer events. It is thread-local and survives across frames, so a
driver test that clicks a widget and then reads its colour sees the
hover or press the click left behind. Under `--test-threads=1` every
test runs on one thread and inherits the previous test's pointer state.

Rule: a test whose assertions read hover or press tints, or paint
output under a pointer an earlier step left on screen, calls
`interaction::reset_ambient_interaction()` first (when it builds its
driver, or before the repaint it samples). Never weaken a tint
assertion to tolerate leftover hover. A test that needs a specific hover
pins it instead: pass an explicit `InteractionState` as `draw_dyn`'s
`state`, or build a `PaintEnv` with `with_interaction`.

## What NOT to do

- **Don't re-derive layouts with different inputs in click vs paint.**
  Two independent layout computations that can diverge is the
  structural bug class quadraui eliminates. Use the routing engine, an
  identical-input `layout_primitive`, or the painted-layout record (see
  *The band-aid trap*).

- **Don't add a per-widget method to `Backend`.** A widget's layout,
  paint, cell paint and behaviour live on the widget, in one module.
  When a widget needs a capability no `PaintSurface` / `CellSurface`
  verb offers, add the verb (one backend-neutral drawing operation that
  every backend implements), not a `draw_<widget>`.

- **Don't import vimcode-specific patterns.** The library is pre-1.0
  but we still want it API-stable enough that downstream consumers
  besides vimcode can adopt it.

- **Don't migrate a consumer onto a widget without writing the
  paint/click harness first.** The harness is the gate.
