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
| **Example / app-wiring drift** | Example-driver round-trip — drive the *whole* `AppLogic` through the headless driver, script real `UiEvent`s, assert on the re-rendered screen. Catches mis-routed handlers, missing `Reaction::Redraw`, stale state — none of which (1)–(3) can see. | `tests/tui_example_driver.rs`. |

Every primitive needs (1). Primitives with consumer-pattern recipes
need (2). Primitives with state-derived indicators need (3). Every
runnable example should have at least one (4) covering its core
interaction.

**Each test must be empirically verified by mutation.** Break the
contract (zero out the offset, swap a +/-, paint at the wrong y),
observe at least one test fail, restore. A green test that doesn't
catch its bug class is theatre.

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
- **Generalizes across backends.** Because `AppLogic` is
  backend-neutral, a future `GtkDriver` can feed identical scripted
  events to the identical app and snapshot the Cairo surface — true
  cross-backend parity from one event script.

**Limitation:** the driver renders into a `TestBackend` buffer, so it
does *not* exercise real ANSI/escape emission — terminal-protocol bugs
(raw-mode setup, escape parsing, SGR mouse decoding; e.g. #293) are out
of scope and need a pty-based smoke test instead.

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

## What unit tests don't cover

Animation cadence, font-rendering quirks across host platforms,
terminal-specific edge cases (kitty vs xterm vs urxvt), exact color
choices, accessibility heuristics, "does this feel right". These
remain manual smoke / human review. Goal: every story ratchets
harness coverage forward so the manual-residue surface shrinks
toward "things that genuinely need eyes".
