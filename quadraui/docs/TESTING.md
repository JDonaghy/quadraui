# Testing

## Quality gate

**Required for any change touching a primitive or rasteriser:**

```bash
cargo build --features tui --features gtk
cargo test --features tui
cargo test --features gtk    # If GTK runtime is available
cargo clippy --features tui -- -D warnings
cargo clippy --features gtk -- -D warnings
cargo fmt --check
```

If GTK runtime libraries aren't available locally, the GTK feature won't
build — that's a CI concern. Run TUI checks at minimum.

**Mini-app validation:** if the change affects a primitive consumed by
the kubeui demos, verify they still build:

```bash
cd kubeui && cargo build
cd kubeui-gtk && cargo build  # if GTK is available
```

## Coverage taxonomy

Three bug classes, three test shapes. An agent picking up an issue
should map the work to the relevant rows and add tests accordingly —
no per-issue restatement needed.

| Bug class | Test shape | Lives in |
|---|---|---|
| **Coordinate drift** between paint and click | Paint/click round-trip — paint into the backend's headless surface, find a painted glyph, hit_test that exact coordinate, assert the hit identifies the painted element. | `tui/<name>.rs::tests` and `gtk/<name>.rs::tests`. |
| **Consumer-side click-routing drift** | Consumer-state round-trip — paint, simulate the consumer's click handler, assert the host's state mutation matches the painted UI. | Adjacent to the consumer pattern. Template: `tui::multi_section_view::tests`. |
| **State-derived paint geometry** | Painted-indicator test — set state to a known value, paint, find the indicator in the buffer/surface, assert it lands at the position the formula predicts. | Same module as the rasteriser. |
| **Example / app-wiring drift** | Example-driver round-trip — drive the *whole* `AppLogic` through the headless driver, script real `UiEvent`s, assert on the re-rendered screen. Catches mis-routed handlers, missing `Reaction::Redraw`, stale state — none of which (1)–(3) can see. | `tests/tui_example_driver.rs` (TUI, `TuiDriver`) and `tests/gtk_example_driver.rs` (GTK, `GtkDriver`). |

Every primitive needs (1). Primitives with consumer-pattern recipes
need (2). Primitives with state-derived indicators need (3). Every
runnable example should have at least one (4) covering its core
interaction.

**Each test must be empirically verified by mutation.** Break the
contract (zero out the offset, swap a +/-, paint at the wrong y),
observe at least one test fail, restore. A green test that doesn't
catch its bug class is theatre.

## Acceptance bar for new code

**Every PR that adds or changes a primitive (`<name>.rs`) or an example (`examples/tui_*.rs` / `examples/gtk_*.rs`) must include the matching test from the coverage taxonomy above** — the paint/click round-trip for a primitive, the example-driver round-trip for an example (TUI via `tests/tui_example_driver.rs` / `TuiDriver`, GTK via `tests/gtk_example_driver.rs` / `GtkDriver` — both landed, #301 GD-1..GD-3). **A PR missing its test is rejected at review.** This is enforced by the adversarial reviewer, which reads the project rules in [`CLAUDE.md`](../../CLAUDE.md) (see *"Demos are mandatory for visual features"*).

Tests must use the **high-level driver API** — `find("text")` to locate a painted target, then `click(x, y)` with the coords it returns, plus `screen_contains()`, `press()`, `type_char()`. **Hardcoded coordinates are brittle and out of policy** — locate, don't guess. A coordinate that's correct today silently rots the first time padding, a label, or a layout metric changes.

## Example-driver tests (end-to-end, in-process)

`quadraui::tui::testing::TuiDriver` drives a whole `AppLogic` impl — the
same type the `tui_*` examples instantiate — through the real
event → `handle` → `render` path, against ratatui's in-memory
`TestBackend`. No TTY, no pty: deterministic and `cargo test`-native.

```rust
let mut d = TuiDriver::new(PipelineApp::new(), 100, 30);
let (x, y) = d.find("Go").unwrap();   // locate the painted action button
d.click(x, y);                         // MouseDown in cell coords — no escape math
assert!(d.screen_contains("stage 3")); // click round-tripped paint→hit_test→handle→render
```

- **Why it's distinct from the round-trips above.** Those test one
  `draw_*` fn on a hand-built struct; this tests the example's wiring.
- **No drift from production.** `render`/`dispatch` call the same
  `tui::run::render_frame` / `dispatch_event` the live runner uses, so
  the test path renders + pre-processes events (text selection, Ctrl-C)
  identically to `tui::run`.
- **Mouse is a `UiEvent::MouseDown` in backend coordinates** — no SGR
  escape-sequence math, the ergonomic win over a pty runner for
  click-heavy primitives.
- **Drags work too.** `mouse_down`/`mouse_move`/`mouse_up`/`drag` route
  through `TuiBackend::translate_injected` — the same `apply_dispatch` +
  `DragState` layer `wait_events` uses — so a scripted drag exercises the
  real selection/drag machinery. E.g. drag over a registered text region
  → `TextSelectionChanged`, then `ctrl_char('c')` → `TextCopied` (see
  `panel_drag_selects_text_and_ctrl_c_copies_it`). (Scrollbar thumb-drag
  isn't wired into any TUI example yet — `apply_dispatch` passes no
  scroll surfaces — so there's nothing to drive there until a consumer
  adopts it; the offset math is unit-tested in `dispatch.rs`.)
- **Generalizes across backends.** Because `AppLogic` is
  backend-neutral, `GtkDriver` (below) feeds identical scripted events to
  the identical app and snapshots the Cairo surface — true cross-backend
  parity from one event script.

**Limitation:** the driver renders into a `TestBackend` buffer, so it
does *not* exercise real ANSI/escape emission — terminal-protocol bugs
(raw-mode setup, escape parsing, SGR mouse decoding; e.g. #293) are out
of scope and need a pty-based smoke test instead.

## GtkDriver example-driver tests (end-to-end, in-process)

`quadraui::gtk::testing::GtkDriver` is the GTK twin of `TuiDriver` (#301,
GD-1..GD-3, #446-448): it drives a whole `AppLogic` impl — the same type
the `gtk_*` examples instantiate — through the real
event → `handle` → `render` path, against a headless
`cairo::ImageSurface`. No `gtk::init`, no `Application`, no
`GdkDisplay`, no Xvfb: deterministic and `cargo test --features
gtk`-native, runs anywhere the `gtk4`/`cairo`/`pangocairo` crates link
(incl. dellserver, no display server needed).

```rust
let mut d = GtkDriver::new(PipelineApp::new(), 800, 480);
let (x, y) = d.find("Go").unwrap();   // locate the painted action button
d.click(x, y);                         // MouseDown in pixel coords
assert!(d.screen_contains("stage 3")); // click round-tripped paint→hit_test→handle→render
```

- **No drift from production.** `render`/`dispatch` call the same
  `gtk::run::render_frame` / `dispatch_event` the live `quadraui::gtk::run`
  runner uses, and `click`/`drag` route through the same
  `dispatch_click` / `dispatch_mouse_drag` / `dispatch_mouse_up` the live
  handlers use, so the test path behaves identically to production.
- **Assertion API mirrors `TuiDriver`'s shape** — `find("text")` /
  `screen_contains()` / `press_named()` / `type_char()` — backed by the
  `(text, bounds)` map recorded at paint time (GD-2, #447) plus pixel
  readback via `pixel(x, y)` for colour/geometry assertions Pango text
  search can't express.
- **Core smoke set + cross-backend parity** live in
  `tests/gtk_example_driver.rs` (GD-3, #448) and
  `tests/cross_backend_parity.rs` — the latter runs one scripted event
  sequence through both `TuiDriver` and `GtkDriver` and asserts they
  reach the same logical state.

### Out of scope / limitations

The offscreen driver rasterises the `draw_*` primitives through
`GtkBackend::enter_frame_scope`; it does **not** instantiate the real
`Application`/`ApplicationWindow` or talk to a compositor. Therefore:

- **Toplevel-window-sizing bugs** (e.g. #437 "opens with tiny/broken
  window") are out of its reach — window geometry is negotiated with the
  real WM/compositor, which this driver never touches.
- **Real clipboard/paste** is out of its reach — quadraui's GTK backend
  uses `arboard`, which hits the X11/Wayland selection; the headless
  surface has no selection owner to talk to.
- **Raw GDK signal delivery** is out of its reach — raw keycode
  translation (`gdk_key_to_uievent`), IME composition, and actual
  `EventController` wiring are bypassed; `GtkDriver` injects `UiEvent`s
  directly, the same unified boundary `TuiDriver` uses.
- **Terminal-protocol / real-GL rendering paths** likewise stay with the
  live app.

These all belong to **GD-5 (#450)** — live-app headless smoke that runs
the *real* window under Xvfb/Broadway — or a real-display oracle
(precision/elitebook) for cases GD-5 still can't reach.

### Cross-backend example tests: shared bodies, per-backend adapters

Now that `GtkDriver` has landed (#301, GD-1..GD-3), example-driver tests
should **not** be duplicated per backend. The split is hybrid — ~80%
shares, ~20% is irreducibly backend-specific:

| Layer | Shared? | Why |
|---|---|---|
| The `AppLogic` under test | yes — identical | backend-neutral by design |
| The event script (`press`/`click`/`drag`) | yes — identical | `UiEvent` is the unified boundary |
| Logical / state assertions | yes — identical | same app → same state on every backend |
| Reading "the screen" | **no** | TUI = character-cell grid (string search); GTK = Cairo pixel surface (Pango text map) |
| Coordinate units | **no** | TUI = cells (`line_height 1`); GTK = pixels (`line_height ~16`) |

Abstract the two backend-specific rows behind a small trait, write each
test body **once** as a generic fn, and run it against both drivers:

```rust
trait ExampleDriver {
    fn press_named(&mut self, k: NamedKey);
    fn type_char(&mut self, c: char);
    fn click_text(&mut self, needle: &str);   // locate text → click center (native coords)
    fn drag_text(&mut self, from: &str, to: &str);
    fn screen_has(&self, needle: &str) -> bool;
    fn exited(&self) -> bool;
}

fn check_pipeline_click<D: ExampleDriver>(mut d: D) {   // written ONCE
    d.click_text("Go");
    assert!(d.screen_has("stage 3"));
}

#[test] fn tui() { check_pipeline_click(TuiDriver::new(PipelineApp::new(), 100, 30)); }
#[test] fn gtk() { check_pipeline_click(GtkDriver::new(PipelineApp::new(), 800, 480)); }
```

**Rules for shared bodies:**

1. **Locate by semantics, never literal coordinates.** Use
   `click_text("Go")` / `drag_text(a, b)`, not `click(12.0, 3.0)` — each
   driver resolves text to its own native position (TUI scans the cell
   grid; GTK queries the `(text, bounds)` map it records at paint time),
   so bodies stay unit-agnostic. A test that hard-codes cell/pixel
   numbers will not port.
2. **Assert on logic/text, not pixels, in shared bodies.** `screen_has`
   works on both (cell-grid search vs Pango text map).

**Parity test (free bonus):** run the same script on both drivers and
assert they reach the same logical state — the strongest cross-backend
guarantee:

```rust
fn pipeline_parity<D: ExampleDriver>(mut d: D) -> Vec<bool> {
    d.click_text("Go");
    vec![d.screen_has("stage 3"), d.exited()]
}
assert_eq!(pipeline_parity(tui), pipeline_parity(gtk));
```

**Irreducible per-backend residue (keep separate, by design):** exact
pixel colours, 1px borders, font rendering, double-width glyph handling.
These genuinely differ — TUI keeps cell-style assertions; GTK gets
pixel/Pango checks. Don't try to share these.

**Realized in `tests/cross_backend_parity.rs`:** the `ExampleDriver`
trait sketched above is implemented for both `TuiDriver` and
`GtkDriver`, and `pipeline_parity_tui_and_gtk_agree_on_logical_state`
runs one shared script body against both. Per-example TUI/GTK suites
(`tests/tui_example_driver.rs`, `tests/gtk_example_driver.rs`) stay
separate for now — only the cross-backend parity test currently uses
the shared-body pattern; migrating the full per-example suites onto it
is follow-up work, not required by #301.

## Backend testability requirement

Every backend MUST support headless paint-to-memory so tests don't
need a real display, terminal, window manager, or font server.

- TUI: `ratatui::Buffer` (in-memory char + style cells).
- GTK: `cairo::ImageSurface::create(Format::ARGB32, w, h)` + Pango
  layout queries.
- macOS: `quadraui::macos::headless::BitmapSurface` (CGBitmapContext +
  pixel readback, top-left origin matching `QuadraView`). Integrates
  with `MacBackend::enter_frame_scope`; the full in-window rasteriser
  surface (chrome / content / MSV / containers / overlays) drives the
  same code paths as the live runner.
- Windows (when implemented): `ID2D1Bitmap` as offscreen render target.

New backends ship with their harness on day one.

## Live-app headless smoke (GD-5, quadraui#450)

The offscreen `GtkDriver` above is deliberately display-free — no
`gtk::init`, no `Application`, no window, no `GdkDisplay` — which means
it structurally cannot catch bugs that only exist in a *real* window:
raw GDK signal delivery, widget realization/allocation, IME, or the real
OS clipboard. That's exactly the bug class that motivated this: #437
(`gtk_terminal` opening with a tiny/garbled window, paste not working at
all) only reproduced against a live window.

`quadraui::gtk::run` (every `gtk_*` example goes through it — no
per-example code needed) honours `QUADRAUI_GTK_SMOKE_MS` /
`QUADRAUI_GTK_SMOKE_PASTE` to run a scripted check against the real
window and exit 0/non-zero — see the "Headless smoke mode" section of
`quadraui/src/gtk/run.rs`'s module doc for exactly what's checked
(window/`DrawingArea` size floor, OS clipboard round-trip + a synthetic
Ctrl-V through the real interception path). `quadraui/scripts/gtk_smoke.sh`
wraps the `xvfb-run -a env GSK_RENDERER=cairo ...` invocation.

**This is an operator-run tier, not a CI gate.** The `gtk` CI job is
deliberately Xvfb-free (see the comment in `.github/workflows/ci.yml`) —
running it requires a box with `xvfb` installed (the quadraui#450
`gtk-headless` capability routing). Only the size/clipboard *assertion
logic* is unit-tested in-repo (`gtk::run::smoke_tests`, no display
required); the live launch itself needs a human or an operator-run box
to actually invoke `scripts/gtk_smoke.sh`.

## What unit tests don't cover

Animation cadence, font-rendering quirks across host platforms,
terminal-specific edge cases (kitty vs xterm vs urxvt), exact color
choices, accessibility heuristics, "does this feel right". These
remain manual smoke / human review. Goal: every story ratchets
harness coverage forward so the manual-residue surface shrinks
toward "things that genuinely need eyes".
