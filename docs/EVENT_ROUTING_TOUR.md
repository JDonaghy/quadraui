# How vimcode's backends route mouse events through quadraui — a code tour

Companion to [`TUI_CONSUMER_TOUR.md`](TUI_CONSUMER_TOUR.md). That
document covers D6 rendering — how primitives decide layouts and
backends paint them. This one covers **events** — how clicks,
drags, and modal dismissals route through a single cross-platform
code path so both TUI and GTK (and, eventually, Win-GUI / macOS)
share the same decisions.

Built after Session 329's Phase B.4 event-routing arc. Every
example below points at code that shipped in that arc, so the
commits are small and readable in isolation.

---

## The central claim

You can write a quadraui app that handles mouse input correctly
across every backend **without knowing GTK, Cocoa, Win32, or
crossterm**. The app's only responsibility is:

1. Push onto a `ModalStack` when a modal opens, pop when it closes.
2. Call `DragState::begin(DragTarget::ScrollbarY { ... })` when the
   user starts dragging a scrollbar; `DragState::end()` on release.
3. Match on the `UiEvent`s the dispatcher returns and update engine
   state.

Everything else — platform event capture, modal precedence, drag
math — lives in quadraui. This is the event-side equivalent of D6's
"primitives return Layout, backends paint verbatim."

---

## 1. The three types that make this work

- **`ModalStack`** (`quadraui/src/modal_stack.rs`) — a top-down stack
  of `(WidgetId, Rect)` pairs. Backends hold one instance on their
  root app struct.
- **`DragState`** (`quadraui/src/dispatch.rs`) — `Option<DragTarget>`
  wrapper. Tracks at most one active drag.
- **`UiEvent`** (`quadraui/src/event.rs`) — the enum every dispatcher
  function returns. Already existed from Phase B.1; we just started
  actually using it.

Read these three first. They're small, no dependencies between
them, and the whole routing story falls out of understanding how
they compose.

---

## 2. The dispatcher functions

Three free functions in `quadraui/src/dispatch.rs`. Pure — no
backend deps, no I/O, no mutation of anything outside their
arguments:

```rust
pub fn dispatch_mouse_down(
    stack: &ModalStack,
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
) -> Vec<UiEvent>;

pub fn dispatch_mouse_drag(
    drag: &DragState,
    position: Point,
    buttons: ButtonMask,
) -> Vec<UiEvent>;

pub fn dispatch_mouse_up(
    stack: &ModalStack,
    drag: &mut DragState,
    position: Point,
    button: MouseButton,
) -> Vec<UiEvent>;
```

Each takes a *raw* event (just position + button / buttons) and
returns a list of semantic events. Backends don't reason about
precedence; they just feed position to the dispatcher and match on
what comes back.

### What `dispatch_mouse_down` does

Three cases, in order:

1. **Click landed on an open modal.** Returns `[MouseDown { widget:
   Some(id), .. }]`. App matches on `id`, refines with primitive
   inner hit-test.
2. **Click landed outside any open modal, but a modal is open.**
   Returns `[MouseDown { widget: None }, Palette(topmost_id,
   Closed)]`. App matches on `Palette::Closed` and closes the modal.
3. **No modal open.** Returns `[MouseDown { widget: None }]`. App
   falls through to base-layer click handling.

Case #2 is the "click outside to dismiss" convention every desktop
platform uses. Implementing it inline per-backend is where "forgot
to handle this in one backend" bugs come from (see issue #192,
which motivated the pilot commit).

### What `dispatch_mouse_drag` does

If `DragState` is active *and* the drag target is `ScrollbarY`, it
computes a scroll offset from the mouse y via linear interpolation
against the track geometry and emits `PaletteEvent::ScrollOffsetChanged`.
Always emits `MouseMoved`.

### What `dispatch_mouse_up` does

Clears the drag state and emits `MouseUp`.

---

## 3. Two backend consumers, one dispatcher

The payoff: read `src/gtk/mod.rs` and `src/tui_main/mouse.rs` side
by side and notice they make the **same calls in the same order**
with different coordinate-system inputs.

### GTK picker click (`src/gtk/mod.rs`, around the `picker_open`
branch)

```rust
self.modal_stack.borrow_mut().push(picker_id.clone(), popup_rect);
let events = quadraui::dispatch_mouse_down(
    &self.modal_stack.borrow(),
    quadraui::Point { x: x as f32, y: y as f32 },
    quadraui::MouseButton::Left,
    quadraui::Modifiers::default(),
);
// Match events → hit_modal | dismiss_modal → act
```

### TUI picker click (`src/tui_main/mouse.rs::handle_mouse`)

```rust
modal_stack.push(picker_id.clone(), popup_rect_in_cells);
let events = quadraui::dispatch_mouse_down(
    modal_stack,
    quadraui::Point { x: col as f32, y: row as f32 },
    quadraui::MouseButton::Left,
    quadraui::Modifiers::default(),
);
// Same match → same act
```

**Differences**: the wrapping (`Rc<RefCell<_>>` vs direct `&mut`)
and the coordinate units (pixels vs cells). The quadraui call
itself is identical — same function, same arguments, same
semantics. The drag + up dispatchers are the same story.

This is what "written once, runs on every backend" actually looks
like in practice.

---

## 4. How drags extend the shape

Before Phase B.4, sidebar scrollbars had one-state-per-scrollbar
inline in mouse handlers:

```rust
dragging_sidebar_search: &mut Option<SidebarScrollDrag>,
dragging_debug_sb: &mut Option<DebugSidebarScrollDrag>,
dragging_picker_sb: &mut Option<SidebarScrollDrag>,
dragging_settings_sb: &mut Option<SidebarScrollDrag>,
dragging_generic_sb: &mut Option<SidebarScrollDrag>,
// ... ad nauseam
```

Each had its own state type with subtly different fields; each drag
path reimplemented the same linear-interpolation math.

After Phase B.4: **one `DragState` covers every scrollbar-like
drag**. Migrating a sidebar scrollbar is a three-line change:

```rust
// Before
*dragging_foo_sb = Some(SidebarScrollDrag { track_abs_start, track_len, total });
// After
drag_state.begin(DragTarget::ScrollbarY {
    widget: WidgetId::new("foo"),
    track_start: track_abs_start as f32,
    track_length: track_len as f32,
    visible_rows,
    total_items: total,
});
```

And the drag-handler body shrinks from inline ratio math to a
`dispatch_mouse_drag` call plus a match on the returned
`ScrollOffsetChanged` event. The math itself moves *once* into
quadraui, gets unit-tested there, and every consumer benefits from
the tests.

---

## 5. What's still missing (near-term follow-ups)

- **More drag targets.** `ScrollbarY` is the only `DragTarget`
  variant today. Window-divider dragging, tab-reorder dragging, and
  terminal-split dragging all fit the same shape — each a new
  variant + a match arm in the dispatcher.
- **More modal primitives.** `PaletteEvent::Closed` is currently
  used as the universal "backdrop click dismiss" event regardless
  of what primitive type is topmost on the stack. When a second
  modal type needs the convention (tab switcher dismiss, dialog
  dismiss on click-outside), generalise to a `ModalDismissed(WidgetId)`
  variant or per-primitive events.
- **Keyboard focus routing.** The event enum already has
  `UiEvent::KeyPressed { key, modifiers }` and `Accelerator`.
  Dispatching those through a `FocusStack` is the symmetric story
  to modal-mouse routing. Phase B.4 doesn't touch it yet;
  Session 325's D7 resolved the design but no infrastructure has
  shipped.
- **Scroll-wheel amplification.** Events come through as
  `UiEvent::Scroll { delta, .. }` but each backend still handles
  them inline. Centralising the delta → rows mapping (and handling
  platforms that emit line-deltas vs pixel-deltas) is a natural
  next extension of the dispatcher.

---

## 6. Reading order for new backend authors

Roughly an hour to understand events end-to-end:

1. `quadraui/src/modal_stack.rs` — full file (~250 lines including
   tests). The smallest complete example of the "data + tests in
   quadraui, consumers in backends" split that drives the whole
   event-routing story.
2. `quadraui/src/dispatch.rs` — full file (~500 lines including
   tests). Three free functions + `DragState`; each function has
   unit tests showing the expected `Vec<UiEvent>` for representative
   inputs.
3. GTK picker click site: search `src/gtk/mod.rs` for
   `dispatch_mouse_down`.
4. TUI picker click site: search `src/tui_main/mouse.rs` for
   `dispatch_mouse_down`. Compare to step 3.
5. GTK + TUI picker drag sites: search both files for
   `dispatch_mouse_drag`.

The claim "writing a cross-platform app doesn't require knowing
GTK" is proven by how similar steps 3+5 look to steps 4+5 once you
subtract the platform-specific wrapping.

---

## 7. History

- **Session 325** (`2026-04-23`) — Design decision D7 resolved: focus
  stack model, click + Tab + programmatic transitions, modal-capture
  convention. Informed by the same analysis that drove this event
  arc.
- **Session 328** (`2026-04-23`) — 22-commit rendering arc. Chrome
  migrations land for TUI across every major primitive surface. Sets
  up the question "if rendering is cross-platform, why aren't
  events?"
- **Session 329** (`2026-04-24`) — Event-routing pilot arc: 4
  commits. `ModalStack` + `DragState` + `dispatch_mouse_*` functions
  ship; both GTK and TUI picker events route through them. Closes
  #190, #192.
