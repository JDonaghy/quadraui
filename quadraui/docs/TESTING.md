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

**Checking the rule is measurable, not just stated:** `tools/example_coverage.py` (repo root) prints a matrix of every `examples/tui_*.rs` / `examples/gtk_*.rs` against whether its `AppLogic` struct is wired into the matching example-driver test file, and exits non-zero if any example is missing coverage (#310).

```bash
tools/example_coverage.py
```

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
of scope and need a pty-based smoke test instead — see Tier-3 below.

## Tier-3 pty smoke (real terminal-protocol black box, quadraui#302)

`tests/tui_pty_smoke.rs` closes the exact gap the Limitation above
calls out. It spawns the *actual* example binary
(`cargo run --example <name> --features tui`) inside a real
pseudo-terminal via [`portable-pty`](https://docs.rs/portable-pty)
(the same crate `terminal_engine.rs` uses for the embedded-terminal
primitive) and parses the emitted byte stream with
[`vt100`](https://docs.rs/vt100) into a screen model — no `TestBackend`,
no injected `UiEvent`s. Keystrokes and raw SGR mouse-report bytes are
written to the pty's stdin exactly as a real terminal would deliver
them; assertions read back the emulated screen (`vt100::Screen::contents`).

```bash
cargo test -p quadraui --features tui,terminal --test tui_pty_smoke
```

- **What it catches that `TuiDriver` can't.** Raw-mode / alternate-screen
  setup actually working on a real pty, the ANSI escape stream `tui::run`
  emits parsing back to the expected screen, and SGR mouse-report
  round-trips (the #293 class — motion events leaking their raw escape
  bytes into a focused text input instead of decoding to a mouse event).
- **The PTY master must behave like a terminal, not a dumb pipe.**
  `ratatui`'s crossterm backend queries the cursor position
  (`ESC [ 6 n`) once during `Terminal::new()` and treats a missing reply
  as fatal — a real terminal always answers this. `PtyExample`'s
  background reader thread plays that minimal terminal-emulator role,
  replying `ESC [ row ; col R` from the `vt100` parser's tracked cursor
  position. Skipping this makes every example fail before it ever
  renders (confirmed empirically — see the harness's module doc).
- **Deliberately thin — 2 representative examples** (`tui_pipeline`,
  `tui_chat`), not broad coverage. This is wiring/integration
  confidence for the terminal-protocol layer; `TuiDriver` (Tier "example
  / app-wiring drift" above) remains the primary, deterministic tool for
  everything else — coordinate drift, click routing, state-derived
  paint, and per-example behavior all stay covered there.
- **Runs as a real CI gate**, not an operator-only tier like the GTK
  live-app smoke below. A pty is a kernel device, not a display server —
  no Xvfb, no compositor, works headlessly anywhere `openpty` does.

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

### `FrameInventory`: the portable paint-inventory contract (quadraui#490)

Before #490, "what did you paint and where" was backend-specific and
leaked platform types into test bodies: TUI drivers scanned the cell
grid with `find`/`find_bounds`, GTK read a `(text, bounds)` map, macOS
had nothing, and a test that wanted to relate two painted things
("is X left of Y?", "is X inside the sidebar zone?") had no
backend-neutral way to ask.

[`quadraui::testing::FrameInventory`] (`quadraui/src/testing.rs`, no
`tui`/`gtk`/`macos`/`win` feature gate) is that contract — the same
shape `#322` proposes for quadraweb's id→rect map:

```rust
pub struct TextRun { pub text: String, pub bounds: Rect }
pub struct ZoneRec { pub id: WidgetId, pub bounds: Rect }
pub struct FrameInventory { pub text_runs: Vec<TextRun>, pub zones: Vec<ZoneRec> }
```

`ConformanceDriver::inventory()` returns one per frame. `text_runs` is
recovered from each backend's existing paint output with no per-widget
opt-in (TUI's wide-char-aware cell-grid scan; GTK's `painted_text` map,
fed by every Pango draw via `gtk/painted_text.rs`'s choke point).
`zones` is populated from [`Backend::register_zone`] calls made during
render — currently `AppShell::render` registers one zone per
activity-bar item (keyed by that panel's real `WidgetId`) plus one
`app-shell:`-prefixed zone per shell-chrome region (`activity-bar`,
`sidebar-header`, `sidebar-content`, `main-content`, `status-bar`,
`command-line`, `divider`, `bottom-panel`, `title-bar`, `window`); other
primitives haven't been wired to call `register_zone` yet and simply
contribute no zone, never a wrong one.

`FrameInventory` carries a small relational assertion vocabulary — the
structural fix for CLAUDE.md's no-hardcoded-coordinates rule, since
every one of these is computed on the `Rect`s the inventory already
reports, in the backend's own units, so a shared test body never writes
a coordinate itself:

```rust
let inv = driver.inventory();
inv.screen_has("EXPLORER");             // any text run contains this substring
inv.absent("SOURCE CONTROL");           // no text run does
inv.count("Settings");                  // how many runs do
inv.left_of("E", "EXPLORER");           // a's right edge <= b's left edge
inv.above("EXPLORER", "content");       // a's bottom edge <= b's top edge
inv.same_row("E", "S");                 // vertical ranges overlap
inv.inside("content", &WidgetId::new("app-shell:sidebar-content"));
```

`left_of`/`above`/`inside` are proven to agree across `TuiDriver` and
`GtkDriver` for the `AppShellDemo` shell_app fixture in
`tests/cross_backend_parity.rs` — see
`frame_inventory_relations_agree_tui_and_gtk`.

**Backend checklist:** every backend driver that implements
`ConformanceDriver` MUST implement `inventory()` and populate
`text_runs` from its real paint output (not a stub). Populating `zones`
is incremental (call `Backend::register_zone` from whichever composers/
primitives you want zone-testable) but the field must exist on the wire
from day one — declare it empty, never omit it, so callers never need a
breaking change when a new zone source lands.

## Conformance tiers (C0–C4) and the scenario suite (quadraui#491)

The conformance suite is Tier-4 generalized: **declarative** scenarios,
run against **every** registered backend, producing a scenario × backend
matrix. It complements — does not replace — the tiers above:
`docs/SMELL_AUDIT_2026-07.md` §6.7 spells out the relationship (Tiers 1–3
unchanged; the pty smoke #302 unchanged; `GtkDriver`/GD-5 division of
labour unchanged).

**A new conformance fixture does not waive the per-example Tier-1
obligation.** When a Tier-1 scenario's fixture is a brand-new `tui_*`
example (CLAUDE.md's "Demos are mandatory for visual features" /
"Every TUI example also ships an automated black-box test"), that example
still needs its own `TuiDriver` cluster in `tests/tui_example_driver.rs` —
same as any other new `tui_*` example, conformance scenario or not. The
two mechanisms check different things: the Tier-1 driver test is a
hand-written, backend-specific regression net over that one example's
behaviour; the Tier-4 scenario is the same interaction expressed once and
proven identical across every registered backend. Neither substitutes for
the other. (`tui_modal_occlusion` / `ModalOcclusionDemo` — the fixture
behind `dialog.blocks_click_through` — ships both: see the
`modal_occlusion_*` tests in `tests/tui_example_driver.rs`.)

### The tiers (a new backend's burn-down checklist)

- **C0 — Boot (day one).** Construct the backend headless;
  `begin_frame`/`end_frame`; sane viewport; draw *every* primitive once
  with a canned descriptor — no panic **and** non-empty `text_runs()` for
  text-bearing primitives. This turns the silent no-op `Backend` defaults
  into visible red/green immediately. *(Auto-generated per primitive —
  quadraui#492.)*
- **C1 — Interaction core (mandatory for "complete").** The Tier-1
  scenarios: click routing, keyboard focus, modal occlusion,
  scroll-under-cursor, split relayout, text-selection drag + copy, tab
  close, menu open/navigate/activate, palette type/pick, toast dismiss,
  editor click-to-caret. **This is what ships today** — see the ten
  scenario files under `tests/conformance/scenarios/`.
- **C2 — Event parity.** For each required `UiEvent` variant, a
  per-backend native-injection recipe proving it is emitted. Required:
  Key/Char/MouseDown/Up/Moved/Scroll/DoubleClick/WindowResized/
  Accelerator/ClipboardPaste/TextCopied. Optional (declare, don't fake):
  FilesDropped, MouseEntered/Left, DpiChanged, native menu events.
- **C3 — Platform services.** Clipboard round-trip (headless where
  possible), capability-honest dialogs and notifications.
- **C4 — Native residue (never shared).** Exact colours, font rendering,
  wide-glyph pixels, live-window smoke. GD-5 stays exactly as it is.

### Running it

```sh
cargo test -p quadraui --features tui      --test conformance -- --nocapture
cargo test -p quadraui --features gtk,tui  --test conformance -- --nocapture
```

The matrix prints on every run and is also written to
`$CARGO_TARGET_DIR/conformance-matrix.txt`; CI uploads it as an artifact
from both the `tui` and `gtk` jobs. For a backend that doesn't exist yet
(Windows, macOS) **that artifact is the implementation checklist**.

```text
Conformance matrix (scenario × backend)
scenario                         tier  tui   gtk
-------------------------------------------------
pipeline.click_advances_stage       1  pass  pass
dialog.blocks_click_through         1  pass  pass
...
```

### Adding a scenario = one JSON file, no Rust

Drop a `*.scn.json` under
`quadraui/tests/conformance/scenarios/<area>/`. The runner discovers
files from disk, so nothing registers it. The file stem must equal the
scenario's `id` (the suite asserts this, so a matrix row is greppable
back to its file).

```json
{
  "id": "pipeline.click_advances_stage",
  "fixture": "pipeline_app",
  "tier": 1,
  "viewport": { "cols": 100, "rows": 30 },
  "requires": [],
  "steps": [
    { "assert_absent": "stage 3" },
    { "press": "Right" },
    { "click_text": "Go" },
    { "assert_screen_has": "stage 3" },
    { "type_char": "q" },
    { "assert_exited": true }
  ]
}
```

Steps, one key each (`quadraui/tests/conformance/schema.rs`):

| Act | Assert | Document |
|---|---|---|
| `press`, `type_char`, `type_text`, `ctrl_char` | `assert_screen_has`, `assert_absent`, `assert_count` | `note` |
| `click_text`, `click_text_at` | `assert_left_of`, `assert_above`, `assert_inside` | |
| `drag_text`, `scroll_at` | `assert_exited` | |

**There is no numeric coordinate field anywhere in the schema**, and
serde rejects unknown keys, so `{"click_at": {"x": 12, "y": 3}}` fails to
deserialise. Hardcoded coordinates aren't discouraged — they're
unrepresentable (`schema.rs::numeric_coordinate_steps_are_unrepresentable`
is the executable form of that claim). The only numbers in the schema are
unit-free counts: `tier`, `viewport.cols`/`rows`, `scroll_at.lines`,
`assert_count.count`.

Two gotchas when writing needles:

- `assert_screen_has` / `assert_absent` match a whole painted **row** on
  TUI, but the relational assertions (`assert_left_of`, `assert_above`,
  `assert_inside`) match a single **text run**, which TUI splits at
  spaces. So `assert_screen_has: "OPEN EDITORS"` works while
  `assert_left_of: {a: "OPEN EDITORS", …}` does not — use a
  single-token needle (`"EDITORS"`) for relational assertions.
- How *many* rows fit is a backend detail (TUI counts cells, GTK divides
  pixels by line height). Assertions should name items that are on-screen
  on every backend, not rely on a specific row count.

### Adding a backend = one driver impl + one registration line

`tests/conformance.rs`:

```rust
struct MyFactory;
impl runner::DriverFactory for MyFactory {
    fn make<A: AppLogic + 'static>(app: A, vp: LogicalViewport) -> Box<dyn runner::DynDriver> {
        Box::new(my::testing::MyDriver::new_fixture(app, vp))
    }
}
const MY_CAPS: &[&str] = &["mouse", "scroll", "drag", "text_selection"];
// … and, in `backends()`:
regs.push(BackendReg::new("my", MY_CAPS, fixtures::build::<MyFactory>));
```

Everything else — the fixture registry, the step interpreter, the
matrix — is backend-agnostic. `DynDriver` is the object-safe view of
`ConformanceDriver` (which is `Sized`, so it can't be a trait object);
one blanket impl covers every present and future driver.

### Capabilities: a skip must name what's missing

Each backend declares a capability list. A scenario whose `requires`
mentions a capability the backend hasn't declared is **skipped, with the
missing capability printed in the matrix detail block**. A backend
therefore cannot quietly not-run a scenario: either it declares the gap
up front, or the scenario runs and fails. Silence is impossible.

### Zone-backed assertions: what is registered today

`assert_inside` is the one step that does **not** work off painted text —
it needs a `WidgetId`-keyed rect, which a paint site must volunteer by
calling `Backend::register_zone` during the frame. That is opt-in per
paint site, so the set of assertable zone ids is small and explicit.

Registered today, by `AppShell::render` (via `register_chrome_zones`) and
nothing else:

| Zone id | Registered when |
|---|---|
| `app-shell:window`, `app-shell:activity-bar`, `app-shell:main-content` | always |
| `app-shell:title-bar`, `app-shell:sidebar-header`, `app-shell:sidebar-content`, `app-shell:divider`, `app-shell:bottom-panel`, `app-shell:command-line`, `app-shell:status-bar` | when that region exists this frame |
| the panel's own id, e.g. `panel:explorer` | one per activity-bar item |

Both `TuiDriver::inventory` and `GtkDriver::inventory` forward whatever
the frame registered, so a zone available on one backend is available on
both; `tests/cross_backend_parity.rs` asserts that for
`app-shell:sidebar-content` specifically.

**Naming a zone no paint site registers makes the step unsatisfiable, not
flaky.** `FrameInventory::inside` returns `false` for an unregistered
zone exactly as it does for a run that landed outside a registered one,
so the matrix alone can't tell the two apart. Two things close that gap:
`every_asserted_zone_is_registered_by_every_backend` (in
`tests/conformance.rs`) fails with "backend never registers this id" and
a list of what *is* registered, and the `assert_inside` failure text
itself distinguishes never-registered / never-painted / outside-the-rect.
Before writing an `assert_inside` against a new primitive, wire its
`register_zone` call first.

### Known coordinate-free gaps

Two Tier-1 behaviours from the audit's C1 list can't be expressed
coordinate-free yet, and are *not* faked with a literal:

- **Split-divider drag.** The `SplitTree` divider (not the shell's
  sidebar divider, which *is* registered as `app-shell:divider`) has no
  text on GTK — it's a filled rect — and no `split.rs` paint site calls
  `register_zone`, so there is nothing for `drag_text` to name.
  `split.direction_toggle_relayout` covers split *relayout* relationally
  instead. Unblocked by registering split-divider zones and adding a
  zone-anchored drag step.
- **Editor click-to-caret** (`editor_col_at_x`) — needs a caret position
  read-back on `ConformanceDriver`.

Two related paint/hit-test rounding warts surfaced while writing these
scenarios and are worth knowing about when a scenario mysteriously
"clicks nothing" on TUI: `tui/split.rs` paints the divider at
`round(divider_bounds.x)` while `SplitLayout::hit_test` uses the
unrounded rect (the fix `tui/split_tree.rs` already got for #452), and
a dialog whose total height is an odd number of lines centres on a
half-line in an even-height viewport, so its painted button row and its
hit region land one cell apart.

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

New backends ship with their harness on day one. Once a backend has a
`ConformanceDriver` impl, it MUST also implement
[`FrameInventory`](#frameinventory-the-portable-paint-inventory-contract-quadraui490) —
see the backend checklist above.

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
