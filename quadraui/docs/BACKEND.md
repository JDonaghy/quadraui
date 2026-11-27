# Backend implementation guide

How the [`quadraui::Backend`] trait is implemented, for a contributor adding
or changing one of the in-tree backends. Read [`../BACKEND.md`](../BACKEND.md)
first: it covers the model (widgets paint themselves through the widget
contract; a backend provides one paint target, text metrics, native event
translation, platform services, a runner and a test driver) and the
implementer's checklist. This page is the reference for the trait's
event-loop shape, the state a backend owns, its four hook points, error
reporting, and which `UiEvent` variants a backend must emit.

Four backends implement the trait, each under `src/<platform>/`:

| Backend | Platform | Paint target |
|---|---|---|
| `TuiBackend` | terminals, via ratatui + crossterm | `CellSurface` (`tui/cell_surface.rs`) |
| `GtkBackend` | GTK4, Cairo + Pango | `PaintSurface` (`gtk/surface.rs`) |
| `MacBackend` | macOS, Core Graphics + Core Text | `PaintSurface` (`macos/surface.rs`) |
| `WinBackend` | Windows, Direct2D + DirectWrite | `PaintSurface` (`win/surface.rs`) |

`docs/decisions/BACKEND_TRAIT_PROPOSAL.md` records the original design
rationale for the trait's shape — what gets normalised and what is left
native.

## Event-loop shape

The trait is **poll-driven**: [`Backend::wait_events`] blocks up to a
timeout and [`Backend::poll_events`] drains without blocking, both returning
`Vec<UiEvent>`. Native toolkits are not all poll-driven, so the four
runners use two shapes:

- **TUI polls.** crossterm has a synchronous poll API, so `tui::run`'s
  loop calls `wait_events`, which translates crossterm events with
  `tui::events::crossterm_to_uievents`.
- **GTK, macOS and Win dispatch directly.** Each toolkit input callback
  (a GTK event controller, an `NSView` responder method, a `WndProc`
  message) translates its native event and calls the runner's
  `dispatch_event`, which runs `crate::runtime`'s shared preprocessing and
  then the app.

Each GUI backend also holds an `Rc<RefCell<VecDeque<UiEvent>>>` queue
(`events_handle()` / `push_event`), drained by `poll_events`. It carries
events that arrive outside an input callback — `waker()` payloads, tray
clicks, macOS native-menu activations — and events a callback had to defer
because the backend was already borrowed by an outer dispatch. A new
callback-driven backend (a webview, an Android `ALooper`) uses the same
pair: direct dispatch from callbacks, a queue for everything else.

## What the backend owns

| State | Why it lives on the backend |
|---|---|
| Viewport (`width × height × scale`) | Measured in the backend's native unit (TUI cells, GTK/Win DIPs, macOS points) and reported by [`Backend::viewport`], so layout code reaches the active size without knowing the unit. |
| `ModalStack` (`modal_stack_handle()`) | The stack of open modals. While it is non-empty, the shared runtime suspends widget routing, so nothing behind a modal receives input. A modal's owner pushes its bounds when it opens and pops them when it closes. `quadraui::dispatch::dispatch_mouse_down` hit-tests a click against the stack for app or controller code that routes its own modal. |
| `DragState` (`drag_state_handle()`) | At most one in-flight scrollbar drag (`DragTarget::ScrollbarY` / `ScrollbarX`). `quadraui::dispatch::dispatch_mouse_drag` reads it on each move to emit `ScrollOffsetChanged`; mouse-up clears it. |
| `BackendCore` | The accelerator registry and the text-selection bookkeeping, shared code every backend embeds as one field (`src/backend_core.rs`). |
| `dyn PlatformServices` (`services()`) | Clipboard, file dialogs, notifications, URL opener, trash, secrets and the rest: what genuinely differs across platforms, behind one trait object. |

## The four hooks

Get these right and everything else in the trait is a thin accessor.

### 1. Frame ownership

The native drawing handle — `&mut ratatui::Frame<'_>` for TUI,
`&cairo::Context` for GTK, a `CGContextRef` for macOS, an
`ID2D1RenderTarget` for Win — is only valid inside the scope the toolkit's
draw API yields, so the backend cannot hold one across calls. The pattern:
stash a type-erased pointer for the duration of a closure, and hand the
paint target out only while it is set. `TuiBackend::enter_frame_scope`
(excerpt; `src/tui/backend.rs` also captures a screenshot when frame
capture is enabled):

```rust,ignore
// Crate-internal excerpt: `current_frame_ptr` is a private
// `Cell<*mut ()>` field of `TuiBackend`.
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

[`Backend::cell_surface`] then wraps the stashed frame's buffer in a
`CellSurface` while the pointer is set and returns `None` outside the
scope; a pixel backend's [`Backend::paint_surface`] does the same for its
`PaintSurface`. Widgets never see the native handle: `draw_dyn` and
`draw_primitive` ask for the paint target and hand it to the widget's
`paint` / `paint_cells`, and a call made outside the scope still resolves
the widget's layout but paints nothing.

### 2. Event poll / wait

A `poll_events` / `wait_events` body:

1. Reads or drains the native event(s).
2. Translates them to `UiEvent`s — with the shared constructors in
   `crate::event` (`mouse_down`, `scroll`, `window_resized`, …), see
   [`../BACKEND.md`](../BACKEND.md#use-the-shared-uievent-constructors).
3. Runs the batch through `BackendCore::apply_accelerators`, so a
   registered key binding arrives as `UiEvent::Accelerator(id, mods)`
   instead of `UiEvent::KeyPressed`.
4. Appends any queued `waker()` payloads, and returns the batch.

The runner then passes each event to the shared preprocessing, which does
everything that is the same on every platform: hover/press tracking,
double-click folding, routing into the frame's widgets, accelerator
matching for directly-dispatched keys, Ctrl-C copy, Ctrl-V paste, Tab focus
cycling. None of that belongs in a backend.

### 3. Modal stack and drag state

The backend's part is to own the two handles (`modal_stack_handle()`,
`drag_state_handle()`, both `Rc<RefCell<…>>`) so the runtime, the compose
controllers and app code can share them. It does not dispatch against them
itself. A controller that opens a modal — `MessageDialogController`,
`FormDialogController` — pushes its bounds onto the stack and pops them on
close; see [`COMPOSE.md`](COMPOSE.md).

### 4. PlatformServices

Wrap the platform's native APIs behind [`PlatformServices`] and hand them
out from [`Backend::services`]. Every method either does the real thing or
returns `ServiceResult::Err(BackendError::Unsupported)` — never a faked
success. `PlatformServices::supports(kind)` answers up front for a caller
that wants to choose a degrade path before calling. `TuiBackend`'s
`TuiPlatformServices` (`tui/services.rs`) is the reference for degrading
honestly: in-canvas pickers and dialogs where a terminal has no native
one.

## Error reporting: `BackendError` + polled `last_error()`

`begin_frame`, `end_frame`, `poll_events`, `wait_events` and the four CSD
`bool` methods (`begin_window_drag`, `toggle_window_maximize`,
`begin_window_resize`, `set_cursor`) are infallible by design — see D-009 in
[`decisions/DECISIONS.md`](decisions/DECISIONS.md) for why. Failure is
reported through two additive seams:

- **`BackendError`** (`src/backend.rs`) — a `Clone`, `PartialEq` enum:
  `Unsupported` (this backend cannot service a request), `PlatformFailure
  { context: String }` (a native call failed; `context` names it, e.g.
  `"SetClipboardData"`), and `SurfaceLost` (the render surface or device was
  lost mid-frame and must be recreated before the next `begin_frame`).
- **`Backend::last_error(&mut self) -> Option<BackendError>`** — polled and
  cleared on read; the default always answers `None`. `WinBackend`
  overrides it: `end_frame` records `SurfaceLost` whenever
  `ID2D1RenderTarget::EndDraw` fails and drops the render target, and
  `WinBackend::ensure_surface` recreates it lazily before the next frame
  (called from `win::run`'s `WM_PAINT`/`WM_SIZE` handlers and from
  `WinDriver::render`). `end_frame` also reports the failure through
  `crate::diagnostics::emit`, so a host with a diagnostics sink sees it
  even if nothing polls `last_error()`. `TuiBackend`, `GtkBackend` and
  `MacBackend` use the default.
- **`Clipboard::write_text_result(&self, text) -> ServiceResult<()>`** —
  the fallible twin of `write_text`. The default calls `write_text` and
  returns `Ok(())`; `WinClipboard` overrides it with the real
  `OpenClipboard`/`GlobalAlloc`/`SetClipboardData` failure.

Not implemented: fallible `_result` twins of the `PlatformServices` dialog
methods, and routing GTK's swallowed Cairo `fill().ok()` /
`stroke().ok()` results through `last_error()`. D-009 lists both as
follow-up.

## `UiEvent` emission matrix

Not every `UiEvent` variant has to be emitted by every backend to be
conformant. This table is the definition a new backend builds against;
D-010 in [`decisions/DECISIONS.md`](decisions/DECISIONS.md) has the
per-variant reasoning.

**Required** — a conformant backend emits this from real native input, or
the variant is inapplicable to its class (TUI has no OS window it can close
independently of the process).

| Variant | TUI | GTK | macOS | Win | Note |
|---|---|---|---|---|---|
| `KeyPressed` | ✅ | ✅ | ✅ | ✅ | The text-input path — see `CharTyped` below. |
| `MouseDown` / `MouseUp` / `MouseMoved` | ✅ | ✅ | ✅ | ✅ | |
| `Scroll` | ✅ | ✅ | ✅ | ✅ | |
| `WindowResized` | ✅ | ✅ | ✅ | ✅ | TUI's "window" is the terminal viewport. |
| `DoubleClick` | ✅ | ✅ | ✅ | ✅ | Folded from two `MouseDown`s by the shared `DoubleClickDetector`, not translated from a native double-click message. The GUI backends fold in the shared preprocessing (`fold_double_click`); TUI folds once per batch of translated crossterm events, before preprocessing. |
| `Accelerator` | ✅ | ✅ | ✅ | ✅ | A `KeyPressed` matching a registered binding, rewritten by `BackendCore` / the shared preprocessing. macOS maps the universal bindings to Cmd. |
| `ClipboardPaste` | ✅ | ✅ | ✅ | ✅ | The paste keystroke is swallowed and the system clipboard read by the shared preprocessing. TUI also translates crossterm's bracketed paste (`Event::Paste`) directly. |
| `WindowClose` | N/A | ✅ (`connect_close_request`) | ✅ (`windowShouldClose:` + `applicationShouldTerminate:`) | ✅ (`WM_CLOSE`) | The app returns `Reaction::Exit` to allow the close; anything else vetoes it. macOS needs both hooks: Cmd-Q and the app menu's Quit go to `applicationShouldTerminate:` and bypass `windowShouldClose:`. |
| `OpenRequested` | N/A | ✅ (`HANDLES_OPEN` + `Application::connect_open`) | ✅ (`application:openURLs:` / `application:openFiles:`) | ✅ (argv at startup; `win::run::RunConfig::single_instance` adds `WM_COPYDATA` forwarding) | macOS gap: a URL that arrives before AppKit finishes launching needs an earlier `kAEGetURL` handler, not implemented. Registering as the OS handler for a scheme or extension is packaging, not something this crate does. |

**Optional** — declare the gap through `BackendCaps` or a doc comment; do
not silently no-op and do not fake it.

| Variant | Status | Note |
|---|---|---|
| `TextCopied` | Every backend | Synthesised by the shared preprocessing: Ctrl-C with an active text selection writes the selection to the clipboard and delivers `TextCopied`. |
| `CharTyped` | No backend emits it | Reserved for IME-committed composed text, **not** a second way to report a keystroke. `KeyPressed { key: Key::Char(_) }` is the text-input event; a consumer that also handled `CharTyped` for the same keystroke would insert twice. |
| `MouseEntered` / `MouseLeft` | No backend emits them | Pointer enter/leave reaches widgets as `PointerKind::Enter` / `Leave` routed events from the runtime's hover tracking instead. |
| `FilesDropped` | GTK ✅ (`DropTarget`), macOS ✅ (`NSDraggingDestination`), Win ✅ (`WM_DROPFILES`), TUI N/A | |
| `DpiChanged` | GTK ✅ (`notify::scale-factor`), macOS ✅ (`NSWindowDidChangeBackingPropertiesNotification`), Win ✅ (`WM_DPICHANGED`), TUI N/A | The shared preprocessing forces a redraw on it, so every GUI backend re-measures. |
| Native menu events (`MenuActivated`, `ContextMenuItemActivated`, `ContextMenuDismissed`) | Backends with `BackendCaps::native_menu` | |
| `TrayClicked` | macOS ✅, Win ✅, GTK ✅ (`StatusNotifierItem`), TUI N/A | Only with `BackendCaps::tray`, and only while no menu is attached to the tray item; with a menu attached, `ContextMenuItemActivated` / `ContextMenuDismissed` arrive instead. |

### Conformance tier C2

`tests/conformance/c2.rs` is the executable half of this table: per-backend
native-injection recipes that call each backend's real native→`UiEvent`
translation function with a synthetic native input and assert the resulting
`UiEvent`'s shape. It covers the mouse/key/scroll/resize core on TUI, GTK
and Win. `win::events`'s translators are plain functions with no WinAPI
call, so the Win column runs on the `ubuntu-latest --features win` leg.

Every required row appears in the printed matrix, with one of three cell
kinds:

- `pass` — a real native-injection assertion ran.
- `n/a` — the variant is inapplicable to that backend class.
- `pass*` — declared but not measurable at this tier, with a footnote
  naming the production wiring. `window_close` on GTK/Win needs a live
  window, and `c2.rs`'s `DISPATCH_ROWS` (`double_click`, `accelerator`,
  `clipboard_paste`) are produced by the shared preprocessing, not by a
  translator, so asserting them needs a live backend and `AppLogic`. TUI's
  `clipboard_paste` is a real `pass`, from bracketed paste.

`TextCopied` is optional and has no row. See
[`TESTING.md`](TESTING.md)'s conformance-tiers section for how C2 relates
to the other tiers.

## Glossary

- **Accelerator** — a stable `AcceleratorId` + `KeyBinding` registered with
  the backend. Matching key events arrive as `UiEvent::Accelerator`, so the
  app dispatches on the id instead of raw key strings.
- **DragTarget** — what a `DragState` drag is moving: `ScrollbarY` or
  `ScrollbarX`, each carrying the track geometry, viewport and content
  sizes, and a `grab_offset` (the cursor's offset from the thumb start at
  press, so the thumb doesn't jump under the cursor).
- **ModalStack** — the LIFO stack of open modals. While non-empty, widget
  routing is suspended; `dispatch_mouse_down` walks it top-down for code
  that routes a modal itself.
- **Paint target** — the `PaintSurface` (pixel backends) or `CellSurface`
  (TUI) a widget paints onto, handed out by `paint_surface()` /
  `cell_surface()` inside the frame scope.
- **WidgetId** — a stable string identifier the app uses to route a
  widget's events and the runtime uses to key its persistent state.
  Convention: colon-separated namespaces (`explorer:tree`,
  `plugin:my-ext:send`).

[`quadraui::Backend`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html
[`Backend::wait_events`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#tymethod.wait_events
[`Backend::poll_events`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#tymethod.poll_events
[`Backend::viewport`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#tymethod.viewport
[`Backend::services`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#tymethod.services
[`Backend::cell_surface`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#method.cell_surface
[`Backend::paint_surface`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html#method.paint_surface
[`PlatformServices`]: https://docs.rs/quadraui/latest/quadraui/trait.PlatformServices.html
