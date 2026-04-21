# Implementing a quadraui backend

Audience: you want to render quadraui primitives onto a target your
favourite primitive doesn't yet support — a new GPU surface, a webview,
a different terminal library, an embedded display, the next major
desktop toolkit. Or you want to understand why the existing backends
look the way they do.

This guide covers the patterns and pitfalls. The per-primitive contracts
themselves live in each primitive's rustdoc; treat this doc as the
"how to think about backends" complement to those contracts.

## 1. Mental model

quadraui has three rules — they're worth internalising before reading
any primitive code:

1. **Apps build primitives from state, every frame, by value.** No
   retained widget tree. No `widget.set_text(...)` between frames. The
   app holds canonical state; primitives are throwaway snapshots.
2. **Backends rasterise primitives.** A backend is a set of `draw_<P>`
   functions, one per primitive it renders. Each takes a primitive plus
   theme/layout and writes to its target (Cairo context, GPU command
   buffer, ratatui buffer, …).
3. **Events flow back as data.** When the user clicks/types/scrolls,
   the backend matches the input against the rendered primitive's
   geometry, emits a `*Event` enum referencing the primitive's
   `WidgetId`, and the app handles it. **No closures cross the
   primitive boundary.**

```
   app state ─build──▶ primitive ─paint──▶ pixels
                                              │
                                              │ user input
                                              ▼
   app state ◀─update── *Event {id, kind} ◀── backend
```

This is unusual if you've used GTK/Qt/SwiftUI (retained-mode) or
React/Dioxus (virtual DOM diffing). It's closer to immediate-mode
GUIs (egui, imgui) but with serializable primitives instead of
opaque widget calls.

The wins:

- **Same primitive, three or more rendering targets.** No
  per-backend coordination.
- **Plugin-safe.** Primitives are `Serialize + Deserialize`, so a Lua
  script can describe UI as a JSON-ish table and the backend renders
  it without trusting Lua to manage Rust object lifetimes.
- **Trivial undo, time-travel, snapshot-testing.** Primitives are
  plain data. Diff them, log them, replay them.

The losses:

- **Per-frame allocation.** You build new primitive structs every
  paint. In practice this is cheap (vimcode does it at 60 fps without
  measurable cost), but if you target ≥10k items you'll want to use
  partial-rebuild patterns (see `TextDisplay`).
- **No retained focus/selection state in the primitive.** The app
  has to remember focus across frames (`focused_field` on
  [`Form`], `selected_path` on [`TreeView`]).

## 2. The three contracts every backend must honour

### Contract A: owned data, no closures, serde-friendly

This is the [§10 plugin invariants](docs/UI_CRATE_DESIGN.md#10-plugin-invariants)
made operational:

| Rule | What it looks like in practice |
|------|--------------------------------|
| `WidgetId` is `String` | Backends can store `WidgetId` in events, send it across threads, hand it to plugins. Never take `&'static str`. |
| Events are plain enums | `TabBarEvent::TabActivated { index }`, `StatusBarEvent::SegmentClicked { id }`. No `Box<dyn Fn>`. |
| Primitives implement `Serialize + Deserialize` | Plugin can declare a panel as JSON. App deserializes → primitive → backend renders. |
| WidgetIds are namespaced | `"plugin:my-ext:send"`, not `"send"`. Prevents collisions when plugins bring their own UI. |
| No global event handlers | Every event references a `WidgetId`. The app's dispatch table is `HashMap<WidgetId, fn>` or similar. |
| Primitives don't borrow app state | Owned data or explicit `'a` lifetimes. No `Arc<Mutex<AppState>>` smuggled through `WidgetId`. |

If your backend respects these, plugins (current and future) can
declare UI without app code changes. If you violate one, you'll find
out the day the first plugin crashes.

### Contract B: measurer-parameterised algorithms

**The most important pattern in quadraui**, and the source of the
hardest-to-debug class of bug.

When a rendering algorithm needs a "width" or per-element measurement
(`fit X within Y` / `where does Z scroll to` / `which slice fits in N
units`), it lives in quadraui *parameterised over a measurement
closure*. Two examples shipped:

```rust
// In quadraui:
StatusBar::fit_right_start<F: Fn(&StatusBarSegment) -> usize>(
    &self, bar_width: usize, min_gap: usize, measure: F,
) -> usize;

TabBar::fit_active_scroll_offset<F: Fn(usize) -> usize>(
    active: usize, tab_count: usize, available_width: usize, measure: F,
) -> usize;
```

Each backend supplies its native measurer:

| Backend | Measurer for `&str` length | Unit |
|---------|----------------------------|------|
| TUI (ratatui) | `s.chars().count()` | cells |
| GTK (Cairo + Pango) | `layout.set_text(s); layout.pixel_size().0 as usize` | pixels |
| Win-GUI (Direct2D + DirectWrite) | DirectWrite `IDWriteTextLayout::GetMetrics().widthIncludingTrailingWhitespace as usize` | pixels |
| macOS (Core Text) | `CTLineGetTypographicBounds(line, ...).width as usize` | pixels |

The shared algorithm doesn't care about units — it just compares
`measure(i) + measure(j) ≤ width` in whatever unit the closure
returns. Each backend uses its own.

**When you add a new "fit/scroll/elide" primitive method, follow the
same pattern. Do NOT bake a unit into shared code.** The cost of
getting this wrong was 5 commits and 3 layered band-aids before the
right architecture surfaced (vimcode issue #158). Symptom: one
backend's tabs / segments / list items appear off-screen or clipped
in ways that look like timing bugs but aren't.

**Detection signal for new contributors:** if you find yourself
wanting to write `let w = name.len() * 8` or `let cell_width = 1` in
shared code, stop. That constant is a unit assumption that will
silently break some backend. Take a measurer instead.

### Contract C: per-primitive backend contract

Some primitives are purely declarative — give the backend the data,
it paints. Others have measurement-dependent state that the backend
MUST maintain each frame, otherwise the primitive renders fine but
interactions break in subtle ways.

Two primitives ship with non-trivial contracts:

- **[`TabBar`]** — measure each tab → call `fit_active_scroll_offset`
  → write the result back to wherever the app stores `scroll_offset`
  → repaint if it changed. Skipping any step makes the active tab land
  off-screen after layout changes. Read [`TabBar`]'s "Backend contract"
  rustdoc section.
- **[`StatusBar`]** — call `fit_right_start` → render only the visible
  slice → click handlers must skip dropped segments. Skipping makes
  narrow bars overlap/touch, and ghost-clicks fire on hidden segments.
  Read [`StatusBar`]'s "Backend contract" rustdoc section.

The other seven primitives ([`TreeView`], [`ListView`], [`Form`],
[`Palette`], [`ActivityBar`], [`Terminal`], [`TextDisplay`]) are
mostly declarative — render the data, route events. Their rustdoc
calls out any per-primitive convention worth knowing (e.g.
`Palette` MUST intercept all clicks when open).

**Always read a primitive's "Backend contract" section before
implementing it.** It's two paragraphs and saves a debugging
session.

## 3. The two-pass paint pattern

Backends without **mid-draw mutability** (you can't change app state
while a paint is in progress) need a way to react to measurements
they take during a paint. The pattern:

```text
              ┌──────────────────────┐
              │  Pass 1: paint with  │
              │  current app state   │
              └─────────┬────────────┘
                        │ (measure tabs / fit segments / …)
                        ▼
              ┌──────────────────────┐
              │  Apply measurements  │
              │  → mutate app state  │
              └─────────┬────────────┘
                        │
                if state changed?
                        │
                ┌───────┴───────┐
                │ no            │ yes
                ▼               ▼
            we're done     ┌──────────────────────┐
                           │  Pass 2: paint again │
                           │  with corrected state│
                           └──────────────────────┘
```

Pass 2 measures the same widths and reaches the same corrected
state, so its apply step changes nothing — converges in 2 frames.

**Per-backend wiring:**

- **TUI (ratatui)**: pass 2 is just calling `terminal.draw(...)` again
  in the same iteration of the main loop. Cheap (≤1ms for a typical
  terminal). See `examples/tui_demo.rs::run` for the pattern.
- **GTK (Cairo)**: pass 2 happens **inside the same `set_draw_func`
  callback** — drop the immutable engine borrow, mutate state, then
  re-enter `draw_editor` overdrawing the same Cairo context. Critical:
  do NOT defer pass 2 via `glib::idle_add_local_once`. During
  continuous events (window drag-resize), GTK's idle queue is
  starved and the deferred callback never fires until the events
  stop — the user sees a stale frame the entire time.
- **Win-GUI (Direct2D)**: every `WM_PAINT` message produces a paint;
  invalidate the area at the end of pass 1 if state changed, the
  next `WM_PAINT` is pass 2. Reliable because Windows posts
  `WM_PAINT` whenever the paint area is invalidated, regardless of
  what other messages are pending.
- **GPU / web / custom backends**: whatever your "request another
  frame" mechanism is, make sure it fires reliably during continuous
  input. If your engine has the equivalent of `requestAnimationFrame`
  scheduling, prefer that over an idle queue.

The two-pass pattern only applies to primitives with the contract-C
"measure-then-correct" requirement. For purely declarative primitives,
a single paint per state change is enough.

## 4. Click intercept hierarchy

When the user clicks anywhere, your backend must check potential
targets **in z-order, top-to-bottom**, and route to the first match.
A common bug class: missing one layer makes clicks fall through to
hidden widgets, triggering actions the user can't see.

Recommended dispatch order:

1. **Modal overlays first** — open `Palette`, dialog, context menu.
   If any is visible, **all clicks** go through it. Don't even
   compute editor / panel hit-tests.
2. **Floating popups** — tooltips, hover popups, autocomplete.
   These have priority over the underlying editor.
3. **Chrome** — tab bar (segments + buttons), status bar (segments),
   activity bar, sidebars. Each has its own `*Event` channel.
4. **Editor / main content area** — the catch-all when nothing else
   matched.

For each chrome layer, the backend stores per-frame hit zones (e.g.
`Vec<(Rect, WidgetId)>` populated during paint, consulted during
click handling). The pattern in vimcode's GTK backend is `Rc<RefCell<HashMap<...>>>`
filled inside `set_draw_func` and read inside the click controller.

**Modal click intercept is mandatory** for `Palette` per its
rustdoc — skipping it is a backend bug class documented in
`NATIVE_GUI_LESSONS.md` §10.

## 5. Minimal backend walkthroughs

The best way to grok the patterns is to read a working backend that
exercises the contracts. Two runnable examples ship with the crate,
demonstrating the **same demo app** in two different rendering
backends — proving the app side is fully backend-agnostic.

**Shared structure:** the two demos share their app code via
`examples/common/mod.rs` (~240 lines: `AppState`, `build_tab_bar`,
`build_status_bar`, event-dispatch helpers). Each demo is then ~350-450
lines of pure backend code. Side-by-side `diff tui_demo.rs gtk_demo.rs`
shows exactly what differs between two backends rendering the same
declarative UI — that's the abstraction quadraui exists to provide.

### `examples/tui_demo.rs` — TUI / ratatui (~350 lines)

```bash
cargo run --example tui_demo
```

Cell-unit measurement (TUI is the easy case — units match the
engine's defaults). Demonstrates:

- **Building primitives from app state** — `build_tab_bar(state)` /
  `build_status_bar(state, focused_id)`.
- **TabBar contract** — `draw_tab_bar` measures each tab in cells,
  calls `fit_active_scroll_offset`, returns the corrected offset.
  Main loop compares to app state, runs pass 2 if it changed.
- **StatusBar contract** — `draw_status_bar` calls
  `fit_right_start_chars(width, 2-cell gap)` and renders only the
  visible slice.
- **Event flow** — Tab cycles `focused_status_idx`, Enter dispatches
  via `handle_status_action(&id)`, state mutation reflected on next
  frame.
- **No hidden state** — every paint rebuilds primitives from
  `AppState`. No retained widget references.

### `examples/gtk_demo.rs` — GTK4 / Cairo + Pango (~430 lines)

```bash
cargo run --example gtk_demo --features gtk-example
```

(Requires GTK4 development libraries — `libgtk-4-dev` on Debian /
Ubuntu, `gtk4-devel` on Fedora, `gtk4` on Homebrew. Off by default
so `cargo build` works on platforms without them.)

Pixel-unit measurement (the harder case — every backend other than
TUI needs this pattern). Same demo app as `tui_demo.rs`, with the
**identical AppState struct, identical primitive builders**, and a
GTK-flavoured backend layer. Demonstrates everything the TUI demo
does, plus:

- **Pango pixel measurer** — each tab's full slot width is
  `tab_pad + label_pixels + close_pixels + tab_pad + outer_gap`
  (~6 chars-equivalent of overhead per tab; this is the under-
  estimation that motivated lesson §12 in NATIVE_GUI_LESSONS.md).
- **Two-pass paint inline within `set_draw_func`** — pass 1 paints,
  the post-paint apply mutates app state via `RefCell::borrow_mut`
  after dropping the immutable borrow, pass 2 overdraws the same
  Cairo context. **No `idle_add`** — the deferred-callback approach
  is unreliable during continuous resize (drag the window narrow
  while many tabs are open to see this matter).
- **Per-frame interaction state passed alongside the primitive** —
  the focused status segment's bold styling is computed from a
  separate `focused_id` parameter, not stored on the primitive
  struct.

The hint row at the bottom shows `[pass-2 fired]` when the demo's
last paint triggered a second pass — flip the flag on/off by
resizing the window or opening/closing tabs.

### Read both before implementing a new backend

Comparing the two demos side-by-side is the fastest way to see
which parts are app code (identical) and which are backend code
(different). The structure transfers near-verbatim to Direct2D /
Core Graphics / wgpu / browser canvas — only the `draw_*`
internals change.

## 6. Backend-implementer checklist

Run through this when you stand up a new backend. Each item is
"implement and test it works"; missing one is a bug class waiting
for a smoke test.

### Per-primitive

- [ ] **Every primitive your app uses has a `draw_<primitive>`
      function.**
- [ ] **For [`TabBar`]**: implement the measure → fit → correct →
      repaint contract. See its rustdoc.
- [ ] **For [`StatusBar`]**: implement the fit → render-slice →
      click-skip contract. See its rustdoc.
- [ ] **For [`Palette`]**: when one is open, intercept ALL mouse
      events (clicks AND scrolls AND drags) and ALL keys.
- [ ] **For [`Form`]**: route Tab / Shift-Tab to focus-change
      events; route printable keys to the focused field's mutation.
- [ ] **For [`Terminal`]**: per-cell `bg` + `fg` + `bold/italic/underline`
      attrs; cursor cell inverts fg/bg; selection / find overlays use
      theme accent colours.
- [ ] **For [`TextDisplay`]**: when `auto_scroll == true`, pin to
      bottom; otherwise respect `scroll_offset`. User scroll-up sets
      `auto_scroll = false`.

### Cross-cutting

- [ ] **Owned `WidgetId` everywhere** — never `&'static str` in event
      types or hit-region tables.
- [ ] **No closures in events** — only plain data with `WidgetId`
      references.
- [ ] **Modal overlays intercept first** in all click handlers.
- [ ] **Two-pass paint** wired for any primitive with a contract-C
      requirement.
- [ ] **Measurer closures use the backend's native units** —
      no shared constants leaking between backends.
- [ ] **Every per-frame interaction state** (hover, drag, focus-within)
      lives **outside** the primitive — passed as a parameter to your
      `draw_*` function.
- [ ] **Click hit-tests use the SAME geometry as draw** — extract a
      shared measurement function so they can't drift.
- [ ] **All popup/dialog content is clipped to the popup bounds** —
      otherwise text bleeds past borders.
- [ ] **All popup positions are clamped to screen bounds** —
      especially right and bottom edges.
- [ ] **Focus flags cleared on every competing click** — clicking
      the editor must clear sidebar/terminal focus, etc. (See
      vimcode's `NATIVE_GUI_LESSONS.md` §7 for the patterns.)

### Recommended testing

- [ ] **Snapshot test** the primitives your app builds at known
      states. Plain-data primitives are trivial to compare with
      `assert_eq!` or `insta`.
- [ ] **Manual resize test** — drag your window narrower and wider
      while many tabs / many right-segments are open. Active tab
      stays visible? Status bar drops segments cleanly? Both
      symptoms of contract-C correctness.
- [ ] **Multi-pane test** — if your app has split layouts, test that
      each pane's chrome is independent (one tab bar's scroll doesn't
      affect another's).
- [ ] **Modal overlay test** — open a `Palette`, click "below" it on
      what would normally be a button — the click must be eaten by
      the palette, not the button.

## 7. When to extend quadraui itself

You'll inevitably want a primitive that doesn't exist yet. Options
in increasing scope:

1. **Use existing primitives creatively.** A simple "Notification
   list" can be a `ListView` with custom decoration. A "log viewer"
   is a `TextDisplay`. A "config screen" is a `Form`. Try this
   first — you'll find ~80% of "I need a new primitive" goes away.

2. **Pass extra per-frame state alongside the primitive** to your
   `draw_*` function. Hover indices, focus rings, drag overlays,
   highlight ranges — these are typically per-frame interaction
   state, not primitive state. Don't bloat the primitive.

3. **Extend an existing primitive.** Add a field, add a `*Event`
   variant. Make sure it satisfies all six §10 invariants. Prefer
   `Option<T>` + `#[serde(default)]` for backward compatibility.

4. **Add a new primitive.** Mirror the existing module structure
   (`src/primitives/your_primitive.rs`), add `pub use` to `lib.rs`,
   write rustdoc with the "Backend contract" section, add a
   `*_roundtrip_serde` test in `lib.rs`. If it has fit/scroll/elide
   logic, parameterise over a measurer (Contract B above).

5. **Ask whether it's app-specific.** Highly app-specific UI (a
   markdown editor, a kubernetes resource graph) is probably
   better as app code that consumes the existing primitives, not
   a new primitive. The bar for "this is reusable across multiple
   apps" should be high.

When in doubt, look at `quadraui/docs/DECISIONS.md` for the
existing rationale on which primitives shipped, which were deferred,
and why. Add to that log when adding new ones.

## 8. Reference implementations

Three production backends live in the [vimcode] repository:

- `src/tui_main/quadraui_tui.rs` — TUI (ratatui)
- `src/gtk/quadraui_gtk.rs` — GTK4 (Cairo + Pango)
- `src/win_gui/quadraui_win.rs` — Windows (Direct2D + DirectWrite)

The naming convention is `quadraui_<backend>::draw_<primitive>` for
the rasteriser plus an adapter in the app's render layer
(`render::*_to_*`) that converts app state to the primitive. Both
patterns are stable; copy them when standing up a new backend.

[vimcode]: https://github.com/JDonaghy/vimcode

## See also

- [`docs/UI_CRATE_DESIGN.md`](docs/UI_CRATE_DESIGN.md) — full design
  rationale, the 13 §7 decisions, the §10 plugin invariants.
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — running log of
  primitive decisions (which shipped, which deferred, why).
- vimcode's `docs/NATIVE_GUI_LESSONS.md` — production lessons from
  shipping three backends. §12-14 cover the unit-mismatch / idle-add
  / debugging-instinct lessons that motivated this guide; §1-11
  cover backend bugs we hit in Win-GUI specifically.
