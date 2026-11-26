# Implementing a quadraui backend

**`Backend` is sealed — see [`quadraui::Backend`]'s rustdoc.** It cannot
be implemented outside this crate: it requires a private supertrait, and a
`compile_fail` doctest proves an out-of-crate `impl Backend` is rejected.
If your target isn't one of the four in-tree backends (TUI, GTK, Win-GUI,
macOS — `quadraui/src/{tui,gtk,win,macos}/`), the supported path is
**contributing a fifth in-tree backend to this repo**, not writing an
out-of-tree crate against `Backend`. A backend plugs into crate-private
seams an outside crate cannot reach (the shared event preprocessing in
`crate::runtime`, the `BackendCore` accelerator and text-selection state,
`crate::platform`'s desktop plumbing), and the trait gains platform-service
methods in ordinary PRs.

**A backend never learns about widgets.** You do not write a `draw_<widget>`
method or a per-widget file. Every widget lays itself out and paints itself
through the widget contract ([`Primitive`] + [`Interactive`]) onto one paint
target your backend provides. Adding a widget to quadraui changes no backend;
adding a backend makes every widget work on it.

Audience: you're opening a PR against this repo to add a new render
backend — a new GPU surface, a webview, a different terminal library, an
embedded display, the next desktop toolkit — or you want to understand why
the existing backends look the way they do. [`docs/BACKEND.md`](docs/BACKEND.md)
is the companion reference for the trait's event-loop, error-reporting and
`UiEvent`-emission conventions.

## 1. Mental model

Three rules, worth internalising before reading any backend code:

1. **Apps build widget descriptors from state, every frame, by value.** No
   retained widget tree, no `widget.set_text(...)` between frames. The app
   holds canonical state; descriptors (`TabBar`, `TreeView`, `Dialog`, …)
   are cheap snapshots. What must survive between frames — a hover index,
   a half-finished drag — is the widget's [`Interactive::State`], which the
   runtime keeps in its widget-state store keyed by [`WidgetId`].
2. **Widgets paint themselves; backends rasterise paint verbs.** A widget's
   [`Primitive::layout`] resolves every geometry decision, and
   [`Primitive::paint`] (pixels) or [`Primitive::paint_cells`] (terminal
   cells) turns that layout into calls on a [`PaintSurface`] or
   [`CellSurface`]. Your backend implements one of those two traits; that is
   the whole of its rendering job.
3. **Events flow back as data.** The backend translates native input into
   [`UiEvent`]s. The shared runtime hit-tests them against the frame's
   widgets, delivers a [`RoutedEvent`] to the widget's
   [`Interactive::handle`], and hands the app the widget's semantic result
   as another `UiEvent` carrying the widget's `WidgetId`. **No closures cross
   the widget boundary.**

```text
 app state ─build─▶ widget descriptor ─layout/paint─▶ PaintSurface / CellSurface ─▶ screen
     ▲                                                                                │
     │                                                                          native input
     │                                                                                ▼
 AppLogic::handle ◀─ UiEvent::<Widget>(id, action) ◀─ Interactive::handle ◀─ runtime ◀─ backend
```

Who owns what:

| Layer | Owns | Lives in |
|---|---|---|
| Widget | layout, paint, hit regions, focus stops, event handling | `src/primitives/`, `src/compose/`, or your own crate |
| Shared runtime | `draw_dyn`/`draw_primitive`, z-order, hit-testing, focus, popups, widget state, accelerator rewrite, Ctrl-C copy | `src/backend.rs` (`DrawsPrimitives`), `src/runtime.rs`, `src/frame_tree.rs`, `src/popup.rs`, `src/widget_state.rs` |
| Backend | one paint target, text metrics, native event translation, platform services, a runner, a test driver | `src/<platform>/` |

This is closer to immediate-mode GUIs (egui, imgui) than to retained-mode
toolkits (GTK, Qt) or virtual-DOM diffing (React), but with serializable
descriptors instead of opaque widget calls. The wins: the same widget on
every target with no per-backend code; descriptors a plugin can declare as
data; snapshot testing on plain values. The cost: descriptors are rebuilt
every frame, which is cheap in practice.

## 2. What a backend implements

`Backend` is platform services only. Grouped by job:

| Job | Methods |
|---|---|
| Frame + viewport | `viewport`, `cell_grid`, `begin_frame`, `end_frame` |
| Theme, style, fonts | `set_theme`, `theme`, `set_style`, `style`, `set_nerd_fonts`, `nerd_fonts_enabled`, `set_editor_font`, `set_ui_font`, `default_fonts`, `register_font_from_memory`, `set_nerd_font_fallback`, `has_font_family` |
| Paint target | `paint_surface` (pixel backends) or `cell_surface` (cell backends), `push_clip`, `pop_clip`, `draw_focus_ring` |
| Metrics | `line_height`, `char_width`, `chrome_line_height`, `set_current_line_height`, `set_current_char_width`, `measure`, `measure_text`, `wheel_delta_rows`, `scales_text_rows`, `snap_height` |
| Events + scheduling | `poll_events`, `wait_events`, `waker`, `request_frame_in`, `request_full_repaint`, `invalidate_rect`, `invalidation_log`, `register_accelerator`, `unregister_accelerator` |
| Structural observation | `register_text_region`, `register_zone`, `cancel_text_selection_drag`, `modal_stack_handle`, `drag_state_handle`, `focus_tree`, `set_focus`, `frame_capture`, `last_frame`, `screenshot`, `last_error` |
| Native chrome | `install_menu_bar`, `show_context_menu`, `menu_style`, `set_menu_style`, `effective_menu_style`, `begin_window_drag`, `toggle_window_maximize`, `begin_window_resize`, `set_cursor`, `titlebar_control_inset`, `set_caret_shape`, `window`, `tray` |
| OS services | `services` (a `dyn PlatformServices`: clipboard, dialogs, notifications, URLs, trash, secrets, …), `backend_caps` |

<!-- TODO(0.12 final pass): after W9 (#2981) confirm `trait Backend` has exactly
the 66 methods tabulated above (`tests/backend_method_ratchet.rs` asserts the
count by equality) and that no `draw_<widget>`/`<widget>_layout` method remains.
If a merged PR moved one of these to another seam, update the table. -->

Beyond the trait, an in-tree backend ships:

- **A paint target.** A `PaintSurface` impl (`gtk/surface.rs`,
  `macos/surface.rs`, `win/surface.rs`) or a `CellSurface` impl
  (`tui/cell_surface.rs`), handed out by `paint_surface()`/`cell_surface()`
  inside the frame scope and `None` outside it. Section 3.
- **Native event translation** into `UiEvent`, using the shared
  constructors. Section 6.
- **A runner** — `<platform>::run(app)` and
  `<platform>::shell_runner::run_with_shell(app, config)` — that owns the
  native event loop, feeds each event through the shared preprocessing, and
  paints each frame. Section 4.
- **A test driver** — `TuiDriver`, `GtkDriver`, `MacDriver`, `WinDriver` in
  `<platform>/testing.rs` — that runs an `AppLogic` headlessly, so the
  conformance suite and every widget test can drive your backend. See
  [`docs/TESTING.md`](docs/TESTING.md).
- **`BackendCaps`** declaring honestly what the platform supports
  (`mouse`, `scroll`, `drag`, `text_selection`, `native_menu`,
  `window_chrome`, `pointer_cursor`).

## 3. The paint target

### Pixel backends: `PaintSurface`

[`PaintSurface`] is a small set of drawing verbs. The required ones:

| Verb | What it does |
|---|---|
| `surface_begin_frame` / `surface_end_frame` / `surface_viewport` | frame bracket and size |
| `surface_line_height` / `surface_char_width` / `surface_measure_text` | text metrics in your native unit |
| `surface_fill_rect` / `surface_fill_rounded_rect` / `surface_stroke_rect` | rectangles |
| `surface_draw_text_run` | one run of text in the current font |
| `surface_draw_line` | a straight stroke |
| `surface_push_clip` / `surface_pop_clip` | a clip stack (must nest) |
| `surface_draw_image` | a decoded image, or `ImagePaintResult::Unsupported` |

The rest have portable defaults built from the required verbs — alpha fills,
rounded strokes, styled and role-specific text runs, shaped text lines
(`surface_draw_text_line`, `surface_text_x_at_index`,
`surface_text_index_at_x`), shadows, paths, alpha blits. Override a default
when your platform does it better: every in-tree pixel backend overrides the
text-line verbs with its own shaper, because the default assumes one
monospace font.

`DrawsPrimitives::draw_dyn` and `draw_primitive` — the blanket entry points
every app and container calls — do the rest for you. They build the
[`LayoutCtx`] from your metrics, call the widget's `layout`, push a clip
around the widget's rect (widened by `Primitive::paint_bleed` for shadows),
select the widget's font role, call `paint` with a [`PaintEnv`] carrying
theme, style, live hover/press and the widget's persistent state, and record
the painted layout for hit-testing.

### Cell backends: `CellSurface`

A terminal cannot express `PaintSurface`'s verbs (a bordered box is literal
box-drawing glyphs chosen per corner; a double-width CJK glyph occupies two
cells), so a cell backend implements [`CellSurface`] instead: `size`,
`put_char`, `put_wide`, `put_str_clipped`, `fill`, `draw_border`, `get`,
`set_cursor`, `color_depth`, plus the defaulted `put_grapheme` and
`emit_graphics` (inline images; `false` when unsupported). Widgets paint to
it from `Primitive::paint_cells`. `quadraui::tui::BufferCellSurface` wraps a
bare buffer for tests and headless renders.

<!-- TODO(0.12 final pass): after #2980 (CSR) removes the ratatui escape hatch from
`CellSurface`, confirm the verb list above matches `src/cell_surface.rs` exactly. -->

### Frame scope

The native drawing handle (a `ratatui::Frame`, a `cairo::Context`, a
`CGContextRef`, an `ID2D1RenderTarget`) is only valid inside the toolkit's
draw callback. Each backend's `enter_frame_scope` stashes it for the
duration of a closure; `paint_surface()`/`cell_surface()` answer `Some` only
inside that scope. [`docs/BACKEND.md`](docs/BACKEND.md#1-frame-ownership)
shows the pattern.

## 4. The runner's paint sequence

Every runner paints a frame in the same order (see `paint_frame` in
`src/tui/run.rs`):

1. `begin_frame(viewport)`.
2. Enter the frame scope and call `app.render(backend, area)`. The app
   paints widgets with `backend.draw_dyn(rect, &widget, &())`, or pushes
   `Surface::Widget` entries onto a `ScreenLayout` and calls `draw`.
3. `crate::popup::paint_open_popups(backend)` — popups the frame's widgets
   opened ([`Interactive::popups`]) paint above everything.
4. The text-selection highlight, then `draw_focus_ring` around the focused
   widget.
5. `end_frame()`.

Between frames, each native event goes through `crate::runtime`'s shared
preprocessing before the app sees it: ambient hover/press tracking,
double-click folding, routing into the widgets of `AppLogic::frame_tree`,
accelerator rewrite, Ctrl-C copy of the active selection, Tab focus
cycling. Your runner calls it; you do not re-implement any of it.

### Write-back and repaint

Some widgets correct app-owned state during layout: [`TabBar`]'s layout
reports `corrected_scroll_offset` so the active tab stays visible, and the
app writes it back for the next frame (`examples/common/demo.rs` does this
right after `draw_dyn`). That needs a reliable "paint again" path. Whatever
your platform's "request another frame" mechanism is, make sure it fires
during continuous input:

- **TUI**: the loop redraws on its next iteration.
- **GTK**: do not defer a repaint through `glib::idle_add_local_once` —
  during a window drag-resize the idle queue is starved, and the user sees a
  stale frame until the drag stops.
- **Win-GUI**: `InvalidateRect` schedules a `WM_PAINT`, which Win32
  delivers before idling.
- **macOS**: `setNeedsDisplay` on the view.

## 5. Measurer-parameterised algorithms

**The most important pattern in quadraui**, and the source of the
hardest-to-debug class of bug.

When an algorithm needs a width (`fit X within Y`, `where does Z scroll to`,
`which slice fits in N units`), it lives in shared code *parameterised over
a measurement closure*. Widgets call these from their own `layout`,
measuring through `LayoutCtx::text_width`, which `draw_dyn` binds to your
backend's `Backend::measure_text`:

```rust,ignore
// Signature excerpts from src/primitives/{status_bar,tab_bar}.rs; bodies elided.
impl StatusBar {
    pub fn fit_right_start<F>(&self, bar_width: usize, min_gap: usize, measure: F) -> usize
    where
        F: Fn(&StatusBarSegment) -> usize;
}

impl TabBar {
    pub fn fit_active_scroll_offset<F>(
        active: usize,
        tab_count: usize,
        available_width: usize,
        measure: F,
    ) -> usize
    where
        F: Fn(usize) -> usize;
}
```

Your backend's only part is to measure in its own native unit:

| Backend | `measure_text` measures with | Unit |
|---|---|---|
| TUI | `quadraui::text_util::display_width` (cell count; a wide glyph is 2) | cells |
| GTK | Pango layout pixel size | pixels |
| Win-GUI | DirectWrite text-layout metrics | DIPs |
| macOS | Core Text typographic bounds | points |

The shared algorithm compares `measure(i) + measure(j) <= width` in
whatever unit the closure returns. **Never bake a unit into shared code.**
If you find yourself writing `let w = name.len() * 8` or `let cell_width = 1`
in a widget, stop: that constant silently breaks some backend. Take the
measurer. The symptom of getting it wrong is one backend's tabs, segments or
list items landing off-screen or clipped in ways that look like timing bugs
but aren't (vimcode#158 took five commits to trace to this).

## 6. Mapping paint verbs and input to your toolkit

The in-tree pixel backends implement the same verbs; this is the
translation table for a fifth.

| Verb | GTK (Cairo + Pango) | Win-GUI (Direct2D + DirectWrite) | macOS (Core Graphics + Core Text) |
|---|---|---|---|
| `surface_fill_rect` | `cr.set_source_rgb(..); cr.rectangle(x, y, w, h); cr.fill()` | `rt.FillRectangle(&rect, &brush)` | `CGContextSetRGBFillColor(..); CGContextFillRect(ctx, rect)` |
| `surface_draw_text_run` | `layout.set_text(t); cr.move_to(x, y); pangocairo::functions::show_layout(cr, &layout)` | `rt.DrawTextLayout(origin, &layout, &brush, ..)` | `CTLineCreateWithAttributedString(s); CGContextSetTextPosition(ctx, x, y); CTLineDraw(line, ctx)` |
| bold, in `surface_draw_text_run_styled` | a Pango weight attribute | an `IDWriteTextFormat` with `DWRITE_FONT_WEIGHT_BOLD` | a `CTFont` with `kCTFontBoldTrait` |
| `surface_push_clip` / `surface_pop_clip` | `cr.save(); cr.rectangle(..); cr.clip()` / `cr.restore()` | `rt.PushAxisAlignedClip(..)` / `rt.PopAxisAlignedClip()` | `CGContextSaveGState; CGContextClipToRect` / `CGContextRestoreGState` |
| `surface_measure_text` | `layout.pixel_size()` | `GetMetrics().widthIncludingTrailingWhitespace` | `CTLineGetTypographicBounds` |

On a cell backend the same jobs are `CellSurface::fill`,
`CellSurface::put_str_clipped`, a bold `CellStyle`, and writing only inside
the rect you were given.

| Native input | GTK | Win-GUI | macOS |
|---|---|---|---|
| key | `EventControllerKey::connect_key_pressed` | `WM_KEYDOWN` / `WM_CHAR` | `keyDown(_:)` |
| resize | the size passed to the draw func | `WM_SIZE` | the view's frame change |
| click | `GestureClick::connect_pressed` | `WM_LBUTTONDOWN` | `mouseDown(with:)` |

The TUI backend reads all three from crossterm.

### Use the shared `UiEvent` constructors

`crate::event` exports free-function constructors — `mouse_down`,
`mouse_up`, `mouse_moved`, `scroll`, `window_resized`, `files_dropped` —
that own each event's shape, including the mouse-wheel sign flip
(`scroll`'s `dy_native_down_positive` parameter negates once, there). A new
backend's translator needs exactly three backend-native pieces:

1. A keysym/keycode → [`Key`] table (`gdk_key_to_quadraui_key`,
   `ns_keycode_to_named_key`, `vk_to_named_key` are the existing shapes).
2. A button-number → [`MouseButton`] map (`gdk_button_to_quadraui`,
   `ns_button_number_to_quadraui`).
3. A modifier-bitmask → [`Modifiers`] map (`gdk_modifiers_to_quadraui`,
   `ns_modifier_flags_to_quadraui`).

…plus one-line wrappers that call the shared constructors with those
tables' output. No backend module constructs `UiEvent::MouseDown` or
`UiEvent::Scroll` literals of its own.

## 7. Hit-testing and z-order are not your job

The runtime decides which widget an event belongs to:

1. **An open modal suspends widget routing.** While `modal_stack_handle()`'s
   stack is non-empty, no frame widget sees the event; the modal's owner
   handles it (a compose controller, or `crate::dispatch::dispatch_mouse_down`
   against the stack). A click "below" a modal never reaches the button
   underneath.
2. **Open popups** — a dropdown, a submenu, a date grid — answer before any
   frame widget, whatever order their owners were painted in
   (`FrameTree::hit`).
3. **Frame widgets**, back to front, each tested against the sub-regions it
   declared (`Interactive::sub_regions`) and the layout it painted with.

What your backend must get right is the geometry underneath: `measure_text`
must agree with what `surface_draw_text_run` actually paints, or hit-tests
drift from the pixels; and `surface_push_clip` must clip, or a long label
bleeds past its popup's border.

## 8. Minimal walkthroughs

`examples/tui_demo.rs` and `examples/gtk_demo.rs` run the **same app**
(`examples/common/demo.rs`) and differ only in the runner they call. The
app paints a `TabBar` and a `StatusBar` with `draw_dyn`, writes the tab
bar's corrected scroll offset back, and handles keys. Nothing in the app is
backend-specific:

```bash
cargo run --example tui_demo --features tui
cargo run --example gtk_demo --features gtk
```

(GTK needs the GTK4 development libraries — `libgtk-4-dev` on Debian/Ubuntu,
`gtk4-devel` on Fedora, `gtk4` on Homebrew.)

To see a backend from the inside, read `paint_frame` in `src/tui/run.rs`
and then `src/tui/cell_surface.rs`; for pixels, `src/gtk/surface.rs`.

## 9. Backend-implementer checklist

### Paint

- [ ] **Every required `PaintSurface` verb** (or every `CellSurface` verb)
      is implemented, and `paint_surface()`/`cell_surface()` answer `Some`
      only inside the frame scope.
- [ ] **The clip stack nests** and actually clips — `draw_dyn` pushes one
      clip per widget, and a popup paints inside its own.
- [ ] **`measure_text` agrees with what you paint**, in your native unit,
      for every `FontRole`.
- [ ] **The text-line verbs use your platform shaper** if your UI font is
      proportional; the defaults assume one monospace font.
- [ ] **`draw_focus_ring`** strokes the focused widget's rect after the app
      and its popups have painted.

### Events

- [ ] **Native input goes through the shared `UiEvent` constructors** and
      the three translation tables of section 6.
- [ ] **The runner calls the shared preprocessing** for every event, so
      widget routing, accelerators, copy and Tab cycling work.
- [ ] **Owned `WidgetId` everywhere** — never `&'static str` in an event.
- [ ] **Repaint requests fire under continuous input** (section 4).
- [ ] **Every required `UiEvent` variant** in the
      [emission matrix](docs/BACKEND.md#uievent-emission-matrix) is emitted.

### Platform

- [ ] **`BackendCaps` is honest.** `begin_window_drag`,
      `toggle_window_maximize`, `begin_window_resize` and `set_cursor` all
      default to a `false`-returning no-op; declare `window_chrome` /
      `pointer_cursor` once you override them.
- [ ] **Every `PlatformServices` method** either does the real thing or
      returns `Unsupported`; it never fakes success.
- [ ] **Every new toolkit-crate import has its Cargo feature enabled.**
      Some bindings gate per class: `objc2-app-kit` puts every class, enum
      and constant behind a feature named after its originating header, so
      `use objc2_app_kit::NSCursor` without `features = ["NSCursor"]` is an
      `unresolved import` — and the feature is not reliably the symbol's
      own name (`NSApplicationActivationPolicy` is in `NSRunningApplication`,
      `NSBackingStoreType` in `NSGraphics`). `src/macos` only compiles on
      `target_os = "macos"`, so nothing on a Linux box catches this;
      `quadraui/tests/macos_appkit_features.rs` is the portable guard, and
      a new import is registered there too.
- [ ] **Type-check your backend's target from wherever you are, before
      pushing.** `cargo check --features <backend>` on the host triple
      compiles none of a `target_os`-gated module, so it exits 0 having
      proved nothing. `cargo check` never links, so it needs only the
      target's `rust-std`, not its SDK. From a Linux box:

      ```bash
      rustup target add aarch64-apple-darwin   # once
      RUSTFLAGS="-D warnings" cargo check -p quadraui --features macos \
          --target aarch64-apple-darwin --all-targets
      ```

      `--all-targets` compiles the `#[cfg(test)]` blocks too, which is
      where `-D warnings` `dead_code` failures show up. This still runs
      nothing — see the next item.
- [ ] **Don't call the native toolkit from a plain `#[test]`.** libtest runs
      each test on a worker thread, in a process that never created an
      `NSApplication` / GTK `Application` / message loop. Vending an AppKit
      object there fails only on the one runner that matters. Split the
      mapping decision out as a pure value (as `macos::backend`'s
      `MacCursorKind` does for cursors) or gate on `MainThreadMarker::new()`
      and accept that the test no-ops in CI. Assert *your* mapping, not the
      toolkit's own invariants.

### Testing

- [ ] **A headless driver** in `<platform>/testing.rs`, registered with the
      conformance suite (`tests/conformance/`), so every widget's paint and
      click tests run on your backend. [`docs/TESTING.md`](docs/TESTING.md)
      describes the tiers.
- [ ] **Manual resize test** — drag the window narrow and wide with many
      tabs and many status segments open. The active tab stays visible and
      the status bar drops segments cleanly.
- [ ] **Modal test** — open a palette or dialog and click "below" it on a
      button: the modal must eat the click.

## 10. When to extend quadraui itself

You'll want something that doesn't exist yet. In increasing scope:

1. **Compose existing widgets.** A notification list is a `ListView`; a log
   viewer is a `TextDisplay`; a settings screen is a `Form`. Try this first.
2. **Write a widget.** Implement [`Primitive`] (`layout`, `paint`,
   `paint_cells`) and [`Interactive`] (`focus`, `handle`, `State`, `Action`)
   for your type — in your own crate if you like, since `Primitive`,
   `Interactive`, `PaintSurface` and `CellSurface` are all public. It paints
   on every backend through `draw_dyn` and the runtime routes events to it,
   with no change to any backend.
   [`docs/PRIMITIVE_RULES.md`](docs/PRIMITIVE_RULES.md) has the rules for a
   widget that ships in this crate.
3. **Add a paint verb.** Only when no combination of existing verbs can
   express what a widget needs. Add it to `PaintSurface` (or `CellSurface`)
   with a portable default where one exists, and implement it on every
   backend in the same PR.
4. **Add a platform service.** A new `PlatformServices` method states its
   TUI story and returns an honest `Unsupported` where a platform lacks it
   — see the `PlatformServices` rule in `docs/PRIMITIVE_RULES.md`.
5. **Ask whether it's app-specific.** A markdown editor or a Kubernetes
   resource graph is probably app code composed from existing widgets,
   not a new quadraui widget.

[`docs/decisions/DECISIONS.md`](docs/decisions/DECISIONS.md) records which
widgets shipped, which were deferred, and why. Add to it when you add one.

## 11. Reference implementations

Read the backend closest to your target before writing a fifth:

- `src/tui/` — `TuiBackend`, terminals via ratatui + crossterm. Cell units,
  no font metrics; the easiest to read first.
- `src/gtk/` — `GtkBackend`, GTK4 (Cairo + Pango).
- `src/win/` — `WinBackend`, Windows (Direct2D + DirectWrite). Also renders
  headlessly for `WinDriver`.
- `src/macos/` — `MacBackend`, macOS (Core Graphics + Core Text). Built and
  tested on `macos-latest` CI.

A new backend adds `src/<platform>/` with its `backend`, `events`, `run`,
`shell_runner`, `services`, paint-surface and `testing` modules,
`impl Backend for <Platform>Backend`, and
`impl crate::backend::sealed::Sealed for <Platform>Backend` (required —
`Backend` is sealed). `tests/per_backend_file_ratchet.rs` keeps each backend
directory to platform modules.

<!-- TODO(0.12 final pass): after W9 (#2981) check the module list above against
the `PLATFORM_FILES` allow-lists in `tests/per_backend_file_ratchet.rs`. -->

## 12. Desktop plumbing you get for free (`crate::platform`)

Some interaction patterns are windowing-generic: the same state machine or
hazard shows up on every desktop toolkit. quadraui factors them into
`src/platform/` (`pub(crate)`, re-exported at `crate::desktop`), so a new
backend doesn't re-derive them from a GTK or AppKit source read. None of it
depends on `gtk4`, `objc2`, `windows` or any other toolkit crate.

Each item is `#[cfg]`-gated **on the features of the backends that call it,
nothing wider** — `src/platform/mod.rs` explains why. Adopting one means
**widening that item's gate with your backend's feature in the same commit
as the call.** Skipping the widen is a hard build failure, not a warning:
with CI's `RUSTFLAGS: "-D warnings"`, a `pub(crate)` item with no caller
under some feature set is `dead_code`, and the failure only appears on a
build with your feature, which the default Linux legs never run.

| What | Type | The hazard it solves |
|---|---|---|
| Client-side titlebar drag-to-move (and edge-resize) arm/threshold/commit | `WindowDragArm<P>` (`platform/window_drag.rs`) | Calling a native "begin move" synchronously on the raw press starts an interactive grab that swallows a following double-click. `WindowDragArm` owns the arm/discard/threshold/commit sequencing; your backend still makes the native call (`gdk4::Toplevel::begin_move`, `NSWindow::performWindowDragWithEvent:`, …) once `commit_if_past_threshold` hands the press back. If your platform's API already tells a drag from a click (AppKit's does), call `.take()` unconditionally and skip the threshold, as `MacBackend::begin_window_drag` does. |
| Nested-modal-loop re-entrancy guard | `ModalPumpDepth` + `ModalPumpGuard` (`platform/modal_pump.rs`) | A synchronous file dialog or message box that pumps the native event loop internally lets every other pending source run too — including your own input handlers, which may try to mutably borrow the backend the dialog call already holds. Unguarded, that second borrow panics inside a non-unwindable native callback and aborts the process. Clone a `ModalPumpDepth` into every runner closure that touches the backend; each checks `is_pumping()` and no-ops. Wrap the nested-loop call in a `ModalPumpGuard` (a depth counter, so a dialog opened from inside another dialog's pump stays guarded until the outermost one unwinds). |
| Headless smoke-mode config and pass/fail predicates | `SmokeConfig`, `smoke_size_ok`, `smoke_clipboard_round_trip_ok` (`platform/smoke.rs`) | "Does the live window look sane" bugs (a tiny or garbled window) only reproduce against a real running window. `SmokeConfig::from_env(ms_var, paste_var)` reads your backend's own env-var names; your runner's one-shot timer calls the predicates after presenting the window, then closes it, so an unattended `xvfb-run`-style invocation exits deterministically. `gtk::run` is the worked example. |
| `PointerShape`/`ResizeEdge` enum walk | `ALL_RESIZE_EDGES`, `all_pointer_shapes()` (`platform/pointer_shape.rs`) | Every backend owns its own `PointerShape` → native-cursor table. These list every variant, so your mapping test covers all of them, including any added later. |

`src/platform/` also holds trash, open-with, reveal-in-file-manager, URL
opening, notifications, clipboard error mapping, paste-keypress detection
and caught-panic reporting. Read its module doc before writing any of those
yourself.

## See also

- [`docs/BACKEND.md`](docs/BACKEND.md) — the trait's event-loop shape,
  frame ownership, error reporting and `UiEvent` emission matrix.
- [`docs/PRIMITIVE_RULES.md`](docs/PRIMITIVE_RULES.md) — rules for widgets
  that ship in this crate.
- [`docs/COMPOSE.md`](docs/COMPOSE.md) — the composite widgets and
  controllers built on the contract.
- [`docs/UI_CRATE_DESIGN.md`](docs/UI_CRATE_DESIGN.md) — design rationale
  and the plugin invariants (owned `WidgetId`s, plain-data events,
  serde-friendly descriptors).
- [`docs/NATIVE_GUI_LESSONS.md`](docs/NATIVE_GUI_LESSONS.md) — production
  lessons from shipping the native backends.

[`quadraui::Backend`]: https://docs.rs/quadraui/latest/quadraui/trait.Backend.html
[`Primitive`]: https://docs.rs/quadraui/latest/quadraui/primitive/trait.Primitive.html
[`Primitive::layout`]: https://docs.rs/quadraui/latest/quadraui/primitive/trait.Primitive.html#tymethod.layout
[`Primitive::paint`]: https://docs.rs/quadraui/latest/quadraui/primitive/trait.Primitive.html#tymethod.paint
[`Primitive::paint_cells`]: https://docs.rs/quadraui/latest/quadraui/primitive/trait.Primitive.html#method.paint_cells
[`Interactive`]: https://docs.rs/quadraui/latest/quadraui/interactive/trait.Interactive.html
[`Interactive::State`]: https://docs.rs/quadraui/latest/quadraui/interactive/trait.Interactive.html#associatedtype.State
[`Interactive::handle`]: https://docs.rs/quadraui/latest/quadraui/interactive/trait.Interactive.html#tymethod.handle
[`Interactive::popups`]: https://docs.rs/quadraui/latest/quadraui/interactive/trait.Interactive.html#method.popups
[`RoutedEvent`]: https://docs.rs/quadraui/latest/quadraui/interactive/enum.RoutedEvent.html
[`PaintSurface`]: https://docs.rs/quadraui/latest/quadraui/trait.PaintSurface.html
[`CellSurface`]: https://docs.rs/quadraui/latest/quadraui/trait.CellSurface.html
[`PaintEnv`]: https://docs.rs/quadraui/latest/quadraui/primitive/struct.PaintEnv.html
[`LayoutCtx`]: https://docs.rs/quadraui/latest/quadraui/primitive/struct.LayoutCtx.html
[`UiEvent`]: https://docs.rs/quadraui/latest/quadraui/enum.UiEvent.html
[`WidgetId`]: https://docs.rs/quadraui/latest/quadraui/struct.WidgetId.html
[`TabBar`]: https://docs.rs/quadraui/latest/quadraui/struct.TabBar.html
[`Key`]: https://docs.rs/quadraui/latest/quadraui/enum.Key.html
[`MouseButton`]: https://docs.rs/quadraui/latest/quadraui/enum.MouseButton.html
[`Modifiers`]: https://docs.rs/quadraui/latest/quadraui/struct.Modifiers.html
