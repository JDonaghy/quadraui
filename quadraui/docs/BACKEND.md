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

## Error reporting: `Unsupported` vs `PlatformFailure` vs `SurfaceLost`

The trait has no `Result` anywhere by default, and stays that way for
`draw_*` and the four CSD `bool` methods (`begin_window_drag`,
`toggle_window_maximize`, `begin_window_resize`, `set_cursor`) — see
`DECISIONS.md` D-009 for the full reasoning. Two seams do get a minimal
error channel, both additive (existing implementors compile unchanged
until they opt in):

**`Backend::last_error(&mut self) -> Option<BackendError>`** — default
`None`. If your backend can genuinely fail at `begin_frame`, `end_frame`,
`poll_events`, or `wait_events` (a lost D3D device, a closed terminal
file descriptor — TUI and GTK today have no such failure mode and can
leave the default alone), stash a `BackendError` in an internal field
from whichever of those four calls hit it, and return/clear it from
`last_error()`. Callers poll this once per loop iteration (conventionally
right after `end_frame`); don't expect them to `match` on every
individual call's return, because there isn't one.

**`PlatformServices`'s `show_file_open_dialog_result` /
`show_file_save_dialog_result` / `show_message_dialog_result`** — each
defaults to wrapping the existing `Option`-returning method in `Ok(..)`.
Override the `_result` twin (not the original) once your backend can
distinguish "user cancelled" (`Ok(None)`, unchanged meaning) from "the
native call itself failed" (`Err(BackendError::PlatformFailure { context })`)
— e.g. a non-cancel `HRESULT` from `IFileOpenDialog::Show`, or a
`GtkFileChooserNative` response your mapping doesn't recognize. Leave
the original method alone; it stays the simpler path for callers that
only care about cancel-vs-picked.

`BackendError::Unsupported` is for the narrow case where `BackendCaps`
says a surface is supported in general but one specific request can't be
serviced (a dialog shape your native API has no representation for) —
**not** a second way to say "I don't implement this at all." A whole
missing surface is a `BackendCaps` field left `false`; reaching for
`Unsupported` there duplicates a vocabulary `tests/conformance/caps.rs`
already checks mechanically. See `BackendCaps`'s own doc comment
("This is the *only* capability vocabulary") and D-009's capability-vs-
error table.

`draw_*` methods stay infallible. A backend that hits a native paint
failure mid-frame (a Cairo call erroring, a Direct2D call against a lost
device) records into the same `last_error()` field `begin_frame`/
`end_frame` use, and returns normally so the rest of the frame still
paints — `last_error()` after `end_frame` is where that surfaces, not a
per-`draw_*` return value.

## `UiEvent` emission matrix (issue #501)

Not every `UiEvent` variant needs to be emitted by every backend to be
conformant, and until this issue almost nothing said which was which —
`docs/LESSONS.md`'s "all runners must fire all `UiEvent` variants the
consumer pattern needs" rule was unenforceable without a definition of
the required set. This table is that definition. `docs/DECISIONS.md`
D-010 has the full per-variant reasoning and the grep evidence behind
each disposition; this table is the quick-reference a new backend author
should build against.

**Required** — a conformant backend must emit this from real native
input, or the variant is inapplicable to that backend class (TUI has no
OS window: `WindowClose`/`DpiChanged`/`WindowFocused`/`WindowResized`'s
"window" is the terminal's viewport, not an OS window it can close
independently of the process).

| Variant | TUI | GTK | macOS | Win | Note |
|---|---|---|---|---|---|
| `KeyPressed` | ✅ | ✅ | ✅ | ✅ | Canonical text-input path — see `CharTyped` below. |
| `MouseDown` / `MouseUp` / `MouseMoved` | ✅ | ✅ | ✅ | ✅ | |
| `Scroll` | ✅ | ✅ | ✅ | ✅ | |
| `DoubleClick` | ✅ | ✅ | ✅ (`MacBackend::fold_double_click`, #486) | ✅ (`WinBackend::fold_double_click`, #729) | Win: still *not* `WM_*BUTTONDBLCLK` — #729 folds two `MouseDown`s in `win::run::dispatch_event` via the shared `DoubleClickDetector`, the same pattern macOS uses. The `❌` here was stale as of quadraui#742. |
| `WindowResized` | ✅ | ✅ | ✅ (`macos/run.rs:560`, #486) | ✅ | |
| `WindowClose` | N/A (no OS window) | ✅ (this issue — `gtk::run::activate`'s `connect_close_request`) | ❌ | ✅ (`WM_CLOSE`, pre-existing) | Veto mechanism: app returns `Reaction::Exit` to allow, anything else vetoes. macOS wiring is a D-010 follow-up (may fold into #486). The ✅ here is proven end-to-end for `gtk_terminal` only (`examples/common/terminal_app.rs`'s `WindowClose` arm); the rest of `examples/gtk_*` have no opinion on it yet, so their catch-all's `Reaction::Continue` vetoes their own "×" button until D-010 follow-up 5 lands — `gtk::run::window_close_tests` proves the `dispatch_event` funnel itself is correct regardless. |
| `Accelerator` | ✅ | ✅ | ✅ (`MacBackend::match_keypress`, #486) | ✅ (`WinBackend::match_keypress`, #707) | Win: wired via `win::run::dispatch_event`'s global-accelerator rewrite (#707) — the "no accelerator matching wired yet" note here was stale as of quadraui#742. |
| `ClipboardPaste` | ✅ | ✅ | ✅ (`macos/run.rs:266`, #486) | ✅ (`win::run::dispatch_event`'s Ctrl-V branch, #728) | Win: Ctrl-V reads `CF_UNICODETEXT` through `WinBackend::services` and delivers `ClipboardPaste` instead of the raw key event. The `❌` here was stale as of quadraui#742. |
| `TextCopied` | ✅ | ✅ | ❌ | ❌ | Neither macOS nor Win wire a Ctrl-C-with-selection → `TextCopied` path yet. Out of this issue's scope. |

Verified directly against `quadraui/src/{tui,gtk,macos,win}` on this
issue's branch, superseding `SMELL_AUDIT_2026-07.md`'s PORT-04 table
(dated 2026-07-25) where they disagree — #486 landed most of the macOS
column since that audit ran; re-verify before trusting either table
against a much later `develop`.

**Optional** (declare the gap via a doc comment / `BackendCaps` once one
exists for it — do not silently no-op and do not fake it):

| Variant | Status | Note |
|---|---|---|
| `CharTyped` | Emitted by **no backend today** | Reserved exclusively for IME-committed composed text (epic #481, IME story #502) — **not** a second way to report a plain keystroke. `KeyPressed{Key::Char}` is the always-on text-input event every backend already emits; two in-tree consumers (`compose::sidebar_system`, `compose::tree_controller`) will double-insert a character if a future backend ever emits both for the same keystroke. See D-010. |
| `MouseEntered` / `MouseLeft` | Emitted by no backend | Zero consumers anywhere today; kept for future hover-driven features (tooltip auto-show). See D-010. |
| `FilesDropped` | Emitted by no backend | Zero consumers today; kept for future drag-and-drop file import. See D-010. |
| `DpiChanged` | Win: ✅ (`WM_DPICHANGED`). GTK: read once at smoke-check time, never on a live runtime change. TUI: N/A (`scale` is always `1.0`). macOS: ❌, not wired. | GTK's live-runtime case is PORT-12's scope, not this issue's. See D-010. |
| Native menu events (`MenuActivated`, `ContextMenuItemActivated`, `ContextMenuDismissed`) | Backend-dependent, out of this table's scope | Only meaningful on a backend with `BackendCaps::native_menu`. |

`❓` = not verified as part of this pass (out of scope for issue #501;
don't infer either way from this table). `❌` on a windowed backend for
a **required** row is a tracked gap, not a design choice — every such
cell above is either wired in this PR or has a named follow-up issue in
D-010.

### Conformance tier C2

`quadraui/tests/conformance/c2.rs` (quadraui#501; Win column quadraui#742)
is the executable half of this table: per-backend "native-injection
recipes" that call each backend's real native→`UiEvent` translation
function (not the Tier-1 scenario suite's higher-level `AppLogic`
replay) with a synthetic native input and assert the resulting
`UiEvent` has the expected shape. It covers the mouse/key/scroll/resize
core on TUI+GTK+Win — `win::events`'s translators are plain
host-independent functions (no WinAPI call in sight), so `win_case`/the
`win` column run on the `ubuntu-latest --features win` leg with no live
`HWND` required, unlike Tier C0/C1's `WinFactory` registration, which
stays `target_os = "windows"`-gated.

Every **required** row of the table above appears in the printed matrix.
Cells fall into three kinds, and the distinction matters when reading it:

- `pass` — a real native-injection assertion ran.
- `n/a` — the variant is *inapplicable* to that backend class
  (`window_close` on TUI: no OS window, D-010).
- `pass*` — declared but *unmeasurable at this tier*, with a footnote
  naming the production wiring and where its real coverage lives. Two
  groups: `window_close` on GTK/Win (needs a live window/`wndproc`), and
  `c2.rs`'s `DISPATCH_ROWS` — `double_click`, `accelerator`,
  `clipboard_paste` — which no backend produces from a native→`UiEvent`
  translator at all. They're synthesised one layer up in each backend's
  `pub(crate)` `dispatch_event` (fold two `MouseDown`s; rewrite a
  `KeyPressed` against the accelerator registry; swallow Ctrl-V and read
  the system clipboard), so they need a live backend + `AppLogic`, not a
  translation-function call. TUI's `clipboard_paste` is the one
  exception and is a real `pass`: crossterm surfaces bracketed paste as a
  native `Event::Paste`, which `crossterm_to_uievents` translates
  directly.

`TextCopied` is not in the required set and has no row. Promoting any
`pass*` to a real assertion is D-010 follow-up work, and needs a
dispatch-level fixture — it is *not* blocked on production wiring, which
has landed on every backend the table marks ✅. See `docs/TESTING.md`'s
"Conformance tiers" section for how C2 relates to C0/C1/C3/C4.

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
