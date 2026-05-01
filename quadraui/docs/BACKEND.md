# Backend implementation guide

How to implement [`quadraui::Backend`] for a new platform.
**Two reference backends exist today**:

- **`TuiBackend`** (`src/tui_main/backend.rs`, Phase B.4) — terminals
  via crossterm. **Fully consuming the trait**: every native event
  flows through `wait_events`; click dispatch goes through
  `dispatch_mouse_*`; accelerator registry drives keybindings;
  generic `paint::<B>` paths drive the quickfix panel and other
  cross-backend primitives.
- **`GtkBackend`** (`src/gtk/backend.rs`, Phase B.5) — desktop via
  GTK4. **Plumbing in place; runtime migration tracked at vimcode
  issue #249.** The trait surface, the `Rc<RefCell<VecDeque<UiEvent>>>`
  event queue, the GDK→`UiEvent` translation helpers, the
  accelerator registry, and `is_modal_open()` are all wired up. But
  the running GTK app still routes events / clicks / keys through
  Relm4 `Msg::*` flow — only the quickfix panel actually consumes
  the trait. The B.5b stages in #249 finish the runtime port.

After reading this guide and the existing TUI implementation you
should be able to drop in a fresh `WinBackend` / `MacBackend` /
`AndroidBackend` end-to-end. **Don't model your impl on `GtkBackend`
yet** — its runtime side is mid-migration.

This doc is descriptive: the architectural rationale (why the trait
looks the way it does, what gets normalised vs. left native) lives in
`BACKEND_TRAIT_PROPOSAL.md`. Read that first if you're writing a
backend from scratch.

## Event-loop shape: poll-driven trait, queue adapter for callback-driven backends

The trait is **poll-driven**: backends implement
[`Backend::wait_events`] / [`Backend::poll_events`] returning
`Vec<UiEvent>`. TUI's crossterm fits this naturally — it has a
synchronous poll API. Callback-driven backends (GTK, Win32 partial,
Cocoa, Android, web) use the **option A event queue** adapter:

```rust
struct GtkBackend {
    events: Rc<RefCell<VecDeque<UiEvent>>>,
    // ...
}
```

The intended pattern is: signal callbacks clone the queue handle
into their captures and `events.borrow_mut().push_back(translated_event)`;
`wait_events` drains the queue. **GtkBackend ships the API but the
producer wiring is B.5b stage 1 work** (issue #249). When that lands,
the GTK runtime fragments behind one queue and the TUI's
synchronous `event_loop()` stays greppable end-to-end — same trait,
same consumer code.

**Forward-compatibility:** every callback-driven backend (Cocoa
delegate methods, Android NDK ALooper, Win32 WindowProc, web JS event
listeners) uses the same queue pattern. macOS may need its
`NSApplication.run()` started on the main thread with delegate
methods pushing to a `Mutex<VecDeque<UiEvent>>`; web pushes from
JS event listeners and drives paint from `requestAnimationFrame`.

## What the backend owns

A backend struct holds the per-app state the trait requires:

| Field | Why it lives on the backend |
|---|---|
| Viewport (`width × height × scale`) | Backends measure the active drawing surface in their native units (TUI cells, GTK DIPs, Win-GUI DIPs, Cocoa points). The trait reports it via [`Backend::viewport`] so generic layout code can reach the active size without the trait knowing about pixels vs. cells. |
| `quadraui::ModalStack` | Backends need to consult it on every mouse-down to decide whether the click hit a modal or fell through to the base layer. `quadraui::dispatch::dispatch_mouse_down` does the hit-test and emits the right `UiEvent` shape; the backend just hands it the stack reference. |
| `quadraui::DragState` | Holds at most one in-flight scrollbar drag (`ScrollbarY`/`ScrollbarX` variants). Mouse-down on a scrollbar arms it; `dispatch_mouse_drag` reads it on every mouse-move to emit `ScrollOffsetChanged`; mouse-up clears it. |
| Accelerator registry | A `HashMap<AcceleratorId, Accelerator>` populated via [`Backend::register_accelerator`]. The backend's event-poll path matches incoming key events against the registry and emits `UiEvent::Accelerator(id, mods)` instead of raw `KeyPressed` for matched keystrokes. |
| `dyn PlatformServices` | Clipboard, file dialogs, notifications, URL opener — the things that genuinely differ across platforms. Wraps each backend's native API behind one trait-object surface. |

## The four hooks

Every backend implements these four-ish hook points; the rest of the
trait's `draw_*` methods are mechanical. **Get these right and the rest
falls into place.**

### 1. Frame ownership

The backend owns whatever object its native API uses to mutate the
screen during a paint pass: `&mut ratatui::Frame<'_>` for TUI,
`&cairo::Context` for GTK, `&ID2D1RenderTarget` for Win-GUI,
`&mut CGContext` for Cocoa.

These are typically only valid inside a closure / scope yielded by the
native draw API, so the backend can't hold one across method calls.
The pattern is: stash a type-erased pointer in a `Cell<*mut ()>` set at
scope entry, run the caller's painting code, clear the pointer on exit.
Trait `draw_*` methods reach the frame via a safe accessor that returns
`None` when the pointer is null (i.e., outside the scope).

`TuiBackend`'s implementation:

```rust
pub fn enter_frame_scope<R>(
    &mut self,
    frame: &mut Frame<'_>,
    f: impl FnOnce(&mut Self) -> R,
) -> R {
    let ptr = frame as *mut Frame<'_> as *mut ();
    let prev = self.current_frame_ptr.replace(ptr);
    let result = f(self);
    self.current_frame_ptr.set(prev);
    result
}
```

Each `draw_*` method then:

```rust
fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
    let area = q_rect_to_ratatui(rect);
    let theme = self.current_theme;
    let frame = self
        .current_frame_mut()
        .expect("draw_palette called outside enter_frame_scope");
    quadraui::tui::draw_palette(frame.buffer_mut(), area, palette, &theme, …);
}
```

The `expect` is a programmer-error tripwire — a misuse from app code,
not a runtime input.

### 2. Event poll / wait

The trait surfaces two methods. [`Backend::wait_events`] blocks up to
`timeout` for the next native event and returns one or more `UiEvent`s.
[`Backend::poll_events`] is the non-blocking variant — used in render
loops that poll on every frame (GTK, Win-GUI, Cocoa) where `wait_events`
on a vsync timer is the wrong shape.

The body of either method is:

1. Read the native event(s).
2. Translate to one or more `UiEvent`s. (`TuiBackend` uses
   [`super::events::crossterm_to_uievents`]; GTK will use
   `gtk_event_to_uievents`; etc.)
3. Run the resulting vec through the accelerator matcher so registered
   key bindings surface as `UiEvent::Accelerator` instead of
   `UiEvent::KeyPressed`.
4. Return the vec.

```rust
fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent> {
    if let Ok(true) = ratatui::crossterm::event::poll(timeout) {
        if let Ok(ev) = ratatui::crossterm::event::read() {
            let mut out = super::events::crossterm_to_uievents(ev);
            self.apply_accelerators(&mut out);
            return out;
        }
    }
    Vec::new()
}
```

`apply_accelerators` is an inherent helper on `TuiBackend`; the same
shape will work on every backend. It iterates registered accelerators
in insertion order, parses each binding into a `(modifiers, key_name)`
pair via `quadraui::parse_key_binding`, and rewrites matching
`UiEvent::KeyPressed` events to `UiEvent::Accelerator(id, mods)`.

### 3. Modal stack + drag dispatch

Mouse events go through three stages:

1. The backend's native event-translation layer turns a click into
   `UiEvent::MouseDown { widget: None, button, position, modifiers }`.
2. The backend hands the modal stack and the mouse coords to
   `quadraui::dispatch::dispatch_mouse_down(&stack, position, button, modifiers)`,
   which returns the right `UiEvent` shape — either filling in
   `widget: Some(modal_id)` if the click landed on a modal, or
   emitting a `Closed` event when the click fell on the backdrop.
3. The same applies for mouse-drag (consults `DragState` to emit
   `ScrollOffsetChanged`) and mouse-up (clears the drag, fills in
   `widget` from the stack).

`TuiBackend` exposes `drag_and_modal_mut()` so the click handler can
borrow both at once without conflicting `&mut self` calls.

### 4. PlatformServices

Wrap the platform's native API behind a small `Clipboard` /
`FileDialog` / `Notification` / `URL opener` set of impls. The trait
just hands them out via [`Backend::services`]; the `&dyn PlatformServices`
return is an erased borrow so backends can mix-and-match (e.g. a TUI
backend on macOS uses the same Cocoa clipboard impl the macOS native
backend uses).

`TuiBackend`'s `TuiPlatformServices` lives in `services.rs` and is the
minimal stub set; real backends will replace each method with a
platform-native call.

## Glossary

- **Accelerator**: a stable `AcceleratorId` + `KeyBinding` registered
  with the backend. The backend matches incoming key events against
  the registry; the app dispatches on `id` instead of raw key strings.
- **DragTarget**: what's being dragged. Today: `ScrollbarY` (vertical
  scroll-thumb drag) and `ScrollbarX` (horizontal). Both carry the
  track geometry, viewport size, total content size, and a
  `grab_offset` (cursor's offset from the thumb start at click-down,
  so the thumb doesn't snap out from under the cursor on grab).
- **ModalStack**: the LIFO stack of currently-open modals (palette,
  dialog, tooltip, completion popup, …). `dispatch_mouse_down` walks
  it in reverse order on every click so events landing inside an
  open modal can't fall through to widgets behind it.
- **WidgetId**: a stable string identifier the app uses to route
  primitive-specific events (`tui:terminal_scrollback`,
  `tui:editor:3:vsb`, `picker`, `explorer:sb`). Convention: bin /
  primitive-specific id namespaces, colon-separated.

## Worked example: where `tui:editor:3:vsb` flows

When a user clicks the vertical scrollbar of editor window id 3 in
TuiBackend:

1. Crossterm emits `MouseEvent { kind: Down(Left), col, row }`.
2. `events::crossterm_mouse_to_uievent` translates to
   `UiEvent::MouseDown { widget: None, button: Left, position, modifiers }`.
3. The event loop pulls it from `backend.wait_events(timeout)` and feeds
   the synthesised crossterm event back to the legacy mouse handler
   via `events::uievent_to_crossterm`.
4. `mouse.rs::handle_mouse` finds the click is on a window's rightmost
   column (the v-scrollbar), arms the backend's drag state with
   `DragTarget::ScrollbarY { widget: WidgetId::new("tui:editor:3:vsb"), … }`,
   and immediately runs `dispatch_mouse_drag` against the click
   position so the click-time offset uses the same thumb-aware math
   subsequent drags will use (no thumb jump).
5. The dispatch's `ScrollOffsetChanged` event is matched in
   `apply_scrollbar_drag`'s `tui:editor:N:<axis>` arm; that calls
   `engine.set_scroll_top_for_window(WindowId(3), new_offset)`.
6. On every subsequent mouse-drag while the button stays down, the
   same `apply_scrollbar_drag` call fires and updates the scroll.
7. On mouse-up, the legacy handler calls `drag_state.end()` and the
   drag is cleared.

The same flow works for any scrollbar in any backend; the only piece a
new backend implements is steps 1–2 (its native event → `UiEvent`
translation). Steps 3–7 are app code that's already shared.
