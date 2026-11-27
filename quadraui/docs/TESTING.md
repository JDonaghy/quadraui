# Testing

How quadraui is tested, and how to test what you add to it: a widget, an
example, a backend. The short version for a widget is *Testing a
contract widget* below. Every widget is one `Primitive` + `Interactive`
type, so it is tested the same way on every backend: unit tests on its
layout and `handle`, plus a black-box driver test that paints it,
clicks it and reads the result back.

## Quality gate

**Required for any change touching a widget, a paint surface or a
backend:**

```bash
cargo build --features tui --features gtk
cargo test --features tui
cargo test --features gtk,tui    # If GTK runtime is available
cargo clippy --features tui -- -D warnings
cargo clippy --features gtk,tui -- -D warnings
cargo fmt --check
```

If GTK runtime libraries aren't available locally, the GTK feature won't
build — that's a CI concern. Run TUI checks at minimum. A change under
`src/macos/` also needs the cross-target compile check (it compiles every
line of `src/macos/` on Linux, but runs nothing):

```bash
RUSTFLAGS="-D warnings" cargo check -p quadraui --features macos \
    --target aarch64-apple-darwin --all-targets
```

**Mini-app validation:** if the change affects a widget consumed by
the kubeui demos, verify they still build:

```bash
cd kubeui && cargo build
cd kubeui-gtk && cargo build  # if GTK is available
```

## CI matrix (`.github/workflows/`)

The commands above are the local minimum; CI runs a wider matrix on every
push/PR to `main` or `develop`. The workflow files are the source of
truth; this table is the map. If you add or remove a job, update this
table in the same PR.

| Job | Runner(s) | What it covers |
|---|---|---|
| `rustfmt` | ubuntu-latest | `cargo fmt --all --check` |
| `repo-lint` | ubuntu-latest | `cargo run -p quadraui-repo-lint` (`tools/lint`): maintainer-workflow guards (CLAUDE.md/ci.yml wording, `.githooks/` behaviour, example manifest hygiene) kept out of `quadraui/tests/` so an outside contributor's `cargo test` never trips on them |
| `Example-test coverage` | ubuntu-latest, PR events only | `tools/example_coverage.py --fail-on-gap`: a `tui_*.rs` example added by the PR must have a matching `TuiDriver` test |
| `Comment-history lint`, `Backend line-count ratchet`, `Wire types`, `CHANGELOG [Unreleased] entry check`, `cargo-semver-checks`, `cargo doc`, `cargo package`, `msrv`, `musl static build` | ubuntu-latest | Repository hygiene and publishability gates; each job's own comments say what it checks |
| `tui + win (build, test, clippy)` | ubuntu-latest **and** windows-latest, both blocking | `cargo build`/`test`/`clippy --features tui`; benches (verify-only); the `win`-feature compile check and arithmetic-only unit tests on Linux; the macOS cross-target compile check; on windows-latest, a real `--features win` build, test and clippy against Direct2D (`src/win/testing.rs`'s `HeadlessSurface`); the Tier-3 pty smoke suite (`--features tui,terminal --test tui_pty_smoke`); the `terminal_engine` unit tests; the `layout`, `a11y`, `tui-images` and `control` feature legs; the conformance matrix, uploaded as an artifact per OS (plus the `win` C0 and Tier-1 tables on windows-latest) |
| `gtk (build, test, clippy)` | ubuntu-latest | `cargo build`/`test`/`clippy --features gtk,tui` (both backends, for `cross_backend_parity.rs` and every two-backend widget test); quadraui-py `check` under `gtk`/`macos`/`win`; the gtk+tui conformance matrix, uploaded as an artifact |
| `quadraui-py (maturin develop + pytest)` | ubuntu-latest | The Python binding's test suite |
| `downstream consumers (compile truth)` | ubuntu-latest | `cargo check --all-targets` against coord-tui and vimcode, both redirected onto this PR's quadraui checkout |
| `gallery capture (tui/gtk/windows)` | ubuntu-latest, windows-latest | Renders the gallery pages to PNGs, uploaded as artifacts for visual review |
| `macOS` (separate workflow, `macos.yml`) | macos-latest; push to `develop` and PRs to `main` touching `quadraui/src/**`, plus weekly and manual dispatch | `cargo build`/`test`/`clippy --features macos` — the only CI place `src/macos`'s tests (including `MacDriver`'s) run. A develop PR is tested on a Mac by the coordinator's Test stage instead |
| `fuzz` (separate workflow, `fuzz.yml`) | ubuntu-latest, `schedule` (daily) + `workflow_dispatch` only, never `push`/`pull_request` | `cargo +nightly fuzz run <target>` for each target in `quadraui/fuzz/fuzz_targets/`, 5 minutes each; see "Property tests and fuzzing" below |

## Property tests and fuzzing (quadraui#1130)

The paint/click round-trips and example-driver tests below prove
*behavioural* correctness against hand-picked scenarios. They say nothing
about the crate's small set of parsers that take **untrusted or foreign
input** — a VS Code theme file downloaded from a marketplace
(`Theme::from_vscode_json` / `text_util::strip_json_comments`), a
markdown document from a chat transcript or README
(`compose::markdown::render_markdown_to_styled`), or whatever bytes a
child process writes into an embedded terminal's PTY (the `vt100` parser
`terminal_engine::TerminalSession` wraps). A hand-picked fixture can only
cover the malformed shapes someone thought to write; these need the
arbitrary-input space instead.

**Two layers, same split as the rest of this doc's CI-vs-local posture:**

1. **`proptest` — in-tree, every PR, bounded.** Each of the files above
   has a `#[cfg(test)] mod proptests` block (`src/text_util.rs`,
   `src/theme.rs`, `src/compose/markdown.rs`, `src/terminal_engine.rs`,
   plus `src/tui/caps.rs`'s `parse_decrqm_reply` and
   `src/tui/backend.rs`'s SGR-mouse-report decode) asserting "never
   panics" and, where the function has one, a real round-trip/invariant
   property — not just the absence of a crash. These run as part of the
   ordinary `cargo test` in the quality gate above: hundreds of
   generated cases per property, sub-second, no extra tooling.
2. **`cargo-fuzz` — scheduled, unbounded, coverage-guided.** `quadraui/fuzz/`
   is a standalone `cargo-fuzz` crate (its own `Cargo.toml`/`Cargo.lock`,
   opted out of the root workspace — see that `Cargo.toml`'s comments)
   with one libFuzzer target per parser that has a reachable `pub` entry
   point: `strip_json_comments`, `theme_from_vscode_json` (round-tripped
   through a real temp file, since `Theme::from_vscode_json` only takes a
   `Path`), `markdown`, and `vt100_parser` (drives the `vt100` crate
   directly — see that target's doc for why). Run via
   `.github/workflows/fuzz.yml`, `schedule`/`workflow_dispatch` only,
   **never** `push`/`pull_request` — a fuzz run's value is continuous
   background exploration on a timescale no per-PR budget affords, and
   bolting it onto `ci.yml` would only slow down every PR for coverage
   the scheduled run already gives more thoroughly. Run locally with:
   ```bash
   cd quadraui/fuzz
   cargo +nightly fuzz run strip_json_comments  # or any other target
   ```

**Why no fuzz target for DECRQM/kitty-keyboard decoding.** `parse_decrqm_reply`
(`src/tui/caps.rs`) is `pub(crate)`, not `pub` — a separate fuzz crate
outside `quadraui`'s own crate boundary cannot reach it without a new
public export, and CLAUDE.md's downstream-impact discipline (see the root
`CLAUDE.md`'s "Downstream consumers" section) argues against adding `pub`
surface whose only consumer is a fuzz harness. Its `proptest` coverage in
`src/tui/caps.rs::proptests` is the whole story for that parser — an
in-crate property test has no such visibility restriction.

**A property test already found a real bug this way.** The initial,
unbounded `vt100_parser_never_panics*` properties (`rows`/`cols` with no
floor) found two upstream `vt100` 0.16.2 panics — `rows < 2` and,
separately, `cols == 1` with certain multi-byte input — that a real host
resizing an embedded-terminal pane down to a single row or column could
have hit in production. `terminal_engine.rs`'s `MIN_VT100_ROWS`/
`MIN_VT100_COLS` consts now floor every caller-supplied dimension before
it reaches `vt100::Parser::new`/`Screen::set_size`, pinned by
`spawn_with_a_single_row_does_not_panic` and its three siblings in that
file's `mod tests`. This is the point of property testing a parser you
don't own: it doesn't have to be *your* bug for it to be worth defending
against.

## Coverage taxonomy

Five bug classes, five test shapes. An agent picking up an issue should
map the work to the relevant rows and add tests accordingly — no
per-issue restatement needed.

| Bug class | Test shape | Lives in |
|---|---|---|
| **Coordinate drift** between paint and click | Paint/click round-trip: paint through a driver, find the painted glyph or sub-part, click that exact coordinate, assert the widget reports the element that painted there. | `tests/widget_contract_<widget>.rs` (black-box), plus the widget module's own `contract_tests` for `layout`/`paint_cells`. |
| **Behaviour drift** between input paths | Event parity: the same gesture through the runtime (a driver click), a direct `Interactive::handle` call, and `a11y::route_widget_action` produces the same `UiEvent`. | The widget module's `contract_tests` (direct and a11y legs) and `tests/widget_contract_<widget>.rs` (runtime leg). |
| **Consumer-side click-routing drift** | Consumer-state round-trip: drive the consumer's `AppLogic`, click what painted, assert the host's state matches the painted UI. | Next to the consumer pattern, or `tests/<backend>_example_driver.rs` for an example. |
| **State-derived paint geometry** (hover, press, scroll, selection) | Painted-indicator test: set the state, paint, find the indicator in the rendered frame, assert it lands where the formula predicts. | The widget's driver test; hover and press on every backend through `tests/hover_channel_<backend>.rs`. |
| **Example / app-wiring drift** | Example-driver round-trip: drive the *whole* `AppLogic` through a headless driver, script real `UiEvent`s, assert on the re-rendered screen. Catches mis-routed handlers, a missing `Reaction::Redraw`, stale state. | `tests/tui_example_driver.rs` (`TuiDriver`), `tests/gtk_example_driver.rs` (`GtkDriver`), `tests/macos_example_driver.rs` (`MacDriver`), `tests/win_example_driver.rs` (`WinDriver`). |

Every widget needs rows 1 and 2. Widgets with consumer-pattern recipes
need row 3. Widgets with state-derived indicators need row 4. Every
runnable example should have at least one row-5 test covering its core
interaction.

**Each test must be empirically verified by mutation.** Break the
contract (zero out the offset, swap a +/-, paint at the wrong y),
observe at least one test fail, restore. A green test that doesn't
catch its bug class is theatre.

## Acceptance bar for new code

**Every PR that adds or changes a widget, or an example
(`examples/tui_*.rs` / `gtk_*.rs` / `macos_*.rs` / `win_*.rs`), must
include the matching test from the coverage taxonomy above**: the
round-trip and event-parity tests for a widget, the example-driver
round-trip for an example. **A PR missing its test is rejected at
review.** This is enforced by the adversarial reviewer, which reads the
project rules in [`CLAUDE.md`](../../CLAUDE.md) (see *"Demos are
mandatory for visual features"*).

Tests must use the **high-level driver API** — `find("text")` to locate
a painted target, then `click(x, y)` with the coords it returns, plus
`screen_contains()`, `press()`, `type_char()`. For a sub-part with no
unique text (a tab's close button, a divider, an activity row), locate
it with `quadraui::primitive::painted_sub_center(&id, sub)`. **Hardcoded
coordinates are brittle and out of policy** — locate, don't guess. A
coordinate that's correct today silently rots the first time padding, a
label, or a layout metric changes. A test may click inside a rect it
declared itself (the area it painted a widget at); that is a located
coordinate, not a guessed one.

**Checking the rule is measurable, not just stated:** `tools/example_coverage.py`
(repo root) prints a matrix of every `examples/tui_*.rs` /
`examples/gtk_*.rs` against whether its `AppLogic` struct is wired into
the matching example-driver test file, and exits non-zero if any example
is missing coverage.

```bash
tools/example_coverage.py
```

**This is also a CI gate:** the `Example-test coverage` job runs the same
tool with `--fail-on-gap` on every PR. It fails only when a `tui_*.rs`
example *added by that PR* lacks coverage; pre-existing gaps print in the
matrix but don't fail the run, and `gtk_*.rs` examples are reported, not
gated. CI diffs against the PR's base commit; to approximate that
locally, diff against `origin/develop` (feature PRs merge into
`develop`; `main` only moves via release merges):

```bash
tools/example_coverage.py --fail-on-gap --base origin/develop
```

## Testing a contract widget

A widget is one type implementing `Primitive` (layout, pixel paint, cell
paint) and `Interactive` (focus, `handle`, sub-regions, popups). There is
no per-backend code to test separately: the same `paint` runs on GTK,
macOS and Win, and the same `paint_cells` on TUI. Test it in four layers.

The examples below use one small widget, `Chip`: a one-row label that a
click or Enter activates — a smaller cousin of the toggle chip in
[`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md#writing-a-widget)'s worked
example. It is written the way a widget outside the crate would be (requires
quadraui's `tui` feature for `paint_cells`):

```rust
use quadraui::{
    A11yInfo, A11yPrimitive, CellSurface, CellSurfaceStyle, EventCx, FocusSpec, Handled,
    Interactive, Key, KeyEvent, LayoutCtx, MouseButton, NamedKey, PaintEnv, PaintSurface,
    PointerKind, Primitive, Rect, RoutedEvent, UiEvent, WidgetAction, WidgetId,
};

#[derive(Debug, Clone)]
pub struct Chip {
    pub id: WidgetId,
    pub label: String,
}

pub struct ChipActivated;

impl WidgetAction for ChipActivated {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::ButtonActivated { widget: id }
    }
}

impl A11yPrimitive for Chip {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Chip {
    type Layout = Rect;
    type Event = ChipActivated;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        let theme = env.theme();
        let bg = if env.is_hovered(&self.id) { theme.hover_bg } else { theme.background };
        surface.surface_fill_rect(*layout, bg);
        surface.surface_draw_text_run(*layout, &self.label, theme.foreground);
    }

    fn paint_cells(&self, layout: &Rect, cells: &mut dyn CellSurface, env: &PaintEnv) {
        let theme = env.theme();
        let bg = if env.is_hovered(&self.id) { theme.hover_bg } else { theme.background };
        let style = CellSurfaceStyle {
            fg: Some(theme.foreground),
            bg: Some(bg),
            ..Default::default()
        };
        cells.fill(*layout, style);
        let (x, y, w) = (layout.x as u16, layout.y as u16, layout.width as u16);
        cells.put_str_clipped(x, y, &self.label, w, style);
    }
}

impl Interactive for Chip {
    type State = ();
    type Action = ChipActivated;

    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        _layout: &Rect,
        cx: &mut EventCx,
    ) -> Handled<ChipActivated> {
        match event {
            RoutedEvent::Pointer { kind: PointerKind::Down(MouseButton::Left), .. } => {
                cx.request_focus(None);
                Handled::Event(ChipActivated)
            }
            RoutedEvent::Key(KeyEvent { key: Key::Named(NamedKey::Enter), .. }) => {
                Handled::Event(ChipActivated)
            }
            _ => Handled::Ignored,
        }
    }
}
```

### 1. Unit tests: `layout` and `handle`, no backend

`layout` is a pure function of a `LayoutCtx` and a rect, so test it
directly: build a `LayoutCtx::new(line_height, char_width, nerd_fonts)`
(`1.0, 1.0` is a cell grid; a pixel metric like `16.0, 8.0` is a pixel
backend), lay out at a **non-zero origin**, and assert on the geometry.

`handle` takes an `EventCx` over any `EventHost`. A recording host lets a
test assert on what the widget asked for (focus, pointer capture, a
redraw) as well as on what it returned:

```rust
use quadraui::backend::PointerShape;
use quadraui::interactive::SubTarget;
use quadraui::primitive::Units;
use quadraui::EventHost;

#[derive(Default)]
struct RecordingHost {
    focus_requests: Vec<Option<SubTarget>>,
}

impl EventHost for RecordingHost {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        false
    }
    fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
        self.focus_requests.push(sub);
    }
    fn capture_pointer(&mut self, _id: &WidgetId) {}
    fn release_pointer(&mut self) {}
    fn set_cursor(&mut self, _cursor: PointerShape) {}
    fn request_redraw(&mut self) {}
    fn emit(&mut self, _event: UiEvent) {}
    fn units(&self) -> Units {
        Units::Cells
    }
}

#[test]
fn a_press_focuses_and_activates_the_chip() {
    let chip = Chip { id: WidgetId::new("chip"), label: "OK".into() };
    let ctx = LayoutCtx::new(1.0, 1.0, false);
    let layout = chip.layout(&ctx, Rect::new(3.0, 2.0, 4.0, 1.0));

    let mut host = RecordingHost::default();
    let press = RoutedEvent::Pointer {
        kind: PointerKind::Down(MouseButton::Left),
        sub: None,
        local: quadraui::Point::new(1.0, 0.0),
        mods: Default::default(),
    };
    let outcome = chip.handle(&mut (), &press, &layout, &mut EventCx::new(chip.id, &mut host));

    assert!(matches!(outcome, Handled::Event(ChipActivated)));
    assert_eq!(host.focus_requests, vec![None]);
}
```

A `RoutedEvent::Pointer`'s `local` is relative to the widget's own rect,
so a unit test never needs to know where the widget will be painted.

For a widget in this crate, these tests live in the widget module's own
`#[cfg(test)] mod contract_tests` (see `src/primitives/button.rs`).
That module also holds two legs of the event-parity test: the same
`RoutedEvent::Action(A11yAction::Click)` through a direct `handle` call
and through `a11y::route_widget_action` over a one-node `FrameTree` must
produce the same `UiEvent`.

### 2. Black-box driver test: paint, locate, click

The acceptance-bar test. An `AppLogic` paints the widget as a
`Surface::Widget` and routes it through a `FrameTree`; a driver clicks
what painted and asserts on what the app received. Paint and routing
must use the same rect (see `LESSONS.md`, *Paint and hit-test must be
handed the same rect*), so `render` records the rect it painted at and
`frame_tree` reads it back:

```rust
use std::cell::Cell;

use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, FrameTree, Reaction, ScreenLayout, Surface};

struct ChipApp {
    chip: Chip,
    /// The rect `render` painted the chip at; `frame_tree` routes against it.
    painted: Cell<Rect>,
    activations: Vec<WidgetId>,
}

impl ChipApp {
    fn new() -> Self {
        Self {
            chip: Chip { id: WidgetId::new("chip"), label: "OK".into() },
            painted: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            activations: Vec::new(),
        }
    }

    fn screen(&self, rect: Rect) -> ScreenLayout {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget { rect, widget: Box::new(self.chip.clone()) });
        frame
    }
}

impl AppLogic for ChipApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        // Sized from the backend's own metrics, so the same app runs on
        // cells and pixels.
        let rect = Rect::new(0.0, 0.0, backend.char_width() * 6.0, backend.line_height());
        self.painted.set(rect);
        self.screen(rect).draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        self.screen(self.painted.get()).into_frame_tree()
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        vec![(self.chip.id, self.painted.get())]
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::ButtonActivated { widget } = event {
            self.activations.push(widget);
        }
        Reaction::Continue
    }
}

#[test]
fn clicking_the_painted_label_activates_the_chip() {
    let mut d = TuiDriver::new(ChipApp::new(), 20, 3);
    let (x, y) = d.find("OK").expect("label painted");
    d.click(x, y);
    assert_eq!(d.app().activations, vec![WidgetId::new("chip")]);
}

#[test]
fn tab_then_enter_activates_the_chip() {
    let mut d = TuiDriver::new(ChipApp::new(), 20, 3);
    d.press_named(NamedKey::Tab);
    d.press_named(NamedKey::Enter);
    assert_eq!(d.app().activations.len(), 1);
}
```

`AppLogic::frame_tree` is what opts the app into the routing engine: hit
test, click-to-focus, pointer capture and `Interactive::handle` all run
against it. A widget's focus stops join Tab cycling once its id appears
in `extra_tab_stops`. For a widget in this crate the file is
`tests/widget_contract_<widget>.rs`; `tests/widget_contract_button.rs`
is the minimal model.

### 3. Sub-parts, hover and press

**Sub-parts.** A composite widget names its parts through
`Interactive::sub_regions`. `draw_dyn` records them, translated to
absolute coordinates, in the frame's `PaintedWidgets`, so a test reaches
a part with no unique text through it:
`quadraui::primitive::painted_sub_center(&id, sub)` returns the point to
click, and `painted_hit(x, y)` answers which widget and sub-part sit
under a point. `tests/widget_contract_shell_view.rs` clicks activity
rows and drags the sidebar divider this way.

**Hover and press.** A widget reads hover and press from
`PaintEnv::is_hovered` / `hovered_sub` / `is_pressed` / `pressed_sub`.
`draw_dyn(.., &())` fills them from the runtime's ambient pointer state,
which the drivers' `mouse_move` / `mouse_down` update. Assert on the
rendered output, not on widget state:

```rust
#[test]
fn hovering_the_chip_tints_it() {
    quadraui::interaction::reset_ambient_interaction();
    let mut d = TuiDriver::new(ChipApp::new(), 20, 3);
    let (x, y) = d.find("OK").expect("label painted");
    let at_rest = d.style_at(x as u16, y as u16).expect("cell in grid").bg;

    d.mouse_move(x, y);

    let hovered = d.style_at(x as u16, y as u16).expect("cell in grid").bg;
    assert_ne!(at_rest, hovered, "hover repaints the chip");
}
```

The ambient state is per-thread and outlives a test's frames, so reset
it first (`LESSONS.md`, *Ambient hover is per-thread state*). To paint
one specific hover without a pointer, pass an explicit
`InteractionState` as `draw_dyn`'s `state` argument, or build a
`PaintEnv` with `with_interaction` for a direct `paint` call. Every
widget that tints is on the shared `HoverBoard` fixture
(`tests/common/hover_board.rs`), which `tests/hover_channel_tui.rs`,
`_gtk.rs`, `_macos.rs` and `_win.rs` drive on each backend and read back
as cell styles or pixels.

**Popups.** A widget that opens a dropdown, a calendar or a submenu sets
`Interactive::OPENS_POPUPS` and answers `Interactive::popups`. The
runtime paints popups above the frame and hit-tests them first, so test
them black-box: open the popup, assert it paints over a widget painted
after the owner, click an item, then press outside or Escape and assert
the dismissal. `tests/widget_contract_popups.rs` does all four for
`ComboBox`.

### 4. Every backend, one test body

A widget test written against `ConformanceDriver` runs unchanged on every
driver the build enables. `ConformanceDriver::new_fixture` takes a
`LogicalViewport` in cells and sizes each backend's surface from it, and
`click_text` resolves text to each backend's own coordinates:

```rust
use quadraui::testing::{ConformanceDriver, LogicalViewport};

fn click_then_enter<D: ConformanceDriver<App = ChipApp>>(app: impl Fn(&D) -> &ChipApp) {
    quadraui::interaction::reset_ambient_interaction();
    let mut d = D::new_fixture(ChipApp::new(), LogicalViewport::new(20, 3));
    d.click_text("OK");
    d.press_named(NamedKey::Enter);
    assert_eq!(app(&d).activations.len(), 2);
}

#[test]
fn click_then_enter_tui() {
    click_then_enter::<TuiDriver<ChipApp>>(|d| d.app());
}

#[cfg(feature = "gtk")]
#[test]
fn click_then_enter_gtk() {
    click_then_enter::<quadraui::gtk::testing::GtkDriver<ChipApp>>(|d| d.app());
}
```

`ConformanceDriver` has no app accessor, so a shared body that inspects
app state takes one as a closure, as above. Add a `MacDriver` leg under
`#[cfg(all(feature = "macos", target_os = "macos"))]` and a `WinDriver`
leg under `#[cfg(all(feature = "win", target_os = "windows"))]`.

## The example drivers (end-to-end, in-process)

Each backend ships a headless driver that runs a whole `AppLogic` — the
same type the examples instantiate — through the real
event → `handle` → `render` path, painting into an in-memory surface. No
TTY, no window, no display server: deterministic and `cargo test`-native.

| Driver | Feature | Paints into | Units | Reads back |
|---|---|---|---|---|
| `quadraui::tui::testing::TuiDriver` | `tui` | ratatui `TestBackend` buffer | cells | `screen()`, `screen_contains`, `find`/`find_bounds` (cell-grid scan), `style_at`/`styled_row` |
| `quadraui::gtk::testing::GtkDriver` | `gtk` | `cairo::ImageSurface` | pixels | `screen_contains`, `find`/`find_bounds` (text map recorded at paint time), `pixel(x, y)` |
| `quadraui::macos::testing::MacDriver` | `macos`, on macOS | `macos::headless::BitmapSurface` (CGBitmapContext) | pixels | same as GTK |
| `quadraui::win::testing::WinDriver` | `win`, on Windows | Direct2D `HeadlessSurface` | pixels | same as GTK |

The shape of a driver test (a fragment: `PipelineApp` stands for any
`AppLogic`):

```rust,ignore
let mut d = TuiDriver::new(PipelineApp::new(), 100, 30);
let (x, y) = d.find("Go").unwrap();   // locate the painted action button
d.click(x, y);                         // MouseDown in cell coords
assert!(d.screen_contains("stage 3")); // click round-tripped paint→hit-test→handle→render
```

`GtkDriver::new(app, 800, 480)`, `MacDriver::new(app, w, h)` and
`WinDriver::new(app, w, h)` take the surface size in pixels;
`ConformanceDriver::new_fixture(app, LogicalViewport::new(cols, rows))`
sizes any of them from a cell viewport.

- **No drift from production.** Each driver's `render`/`dispatch` call
  the same `render_frame` / `dispatch_event` its live runner uses, so
  the test path renders and pre-processes events (text selection,
  Ctrl-C, widget routing through `AppLogic::frame_tree`) identically to
  `<backend>::run`.
- **Mouse is a `UiEvent` in backend coordinates** — no escape-sequence
  math. `press`, `type_char`, `press_named`, `ctrl_char`, `click`,
  `mouse_down`, `mouse_move`, `mouse_up` and `drag` are shared across
  drivers through `testing::DriverInput`. `click` is a bare press with no
  release on every driver except `WinDriver`, whose `click` also
  releases; a test that needs the release calls `mouse_down` then
  `mouse_up`. `TuiDriver` adds `double_click`, `middle_click`,
  `right_click` and `click_with` (modifiers).
- **Drags work too.** On TUI, `mouse_down`/`mouse_move`/`mouse_up`/`drag`
  route through `TuiBackend::translate_injected` — the same
  `apply_dispatch` + `DragState` layer `wait_events` uses — so a scripted
  drag exercises the real selection/drag machinery. E.g. drag over a
  registered text region → `TextSelectionChanged`, then `ctrl_char('c')`
  → `TextCopied` (see `panel_drag_selects_text_and_ctrl_c_copies_it`).
- **Locating contract widgets.** Text-bearing targets: `find`. Sub-parts
  with no unique text: `primitive::painted_sub_center`. `TuiDriver` also
  offers `tab_center` / `tab_close_center` for a tab bar.

**Limitation (`TuiDriver`):** it renders into a `TestBackend` buffer, so
it does *not* exercise real ANSI/escape emission — terminal-protocol bugs
(raw-mode setup, escape parsing, SGR mouse decoding; e.g. #293) are out
of scope and need a pty-based smoke test instead — see Tier-3 below, and
the `tui-vt100` observer under *Conformance tiers*.

### Out of scope for the pixel drivers

The pixel drivers paint offscreen; they never instantiate a real
application window or talk to a compositor. Therefore:

- **Toplevel-window-sizing bugs** (e.g. #437 "opens with tiny/broken
  window") are out of reach — window geometry is negotiated with the
  real window manager, which these drivers never touch.
- **Real clipboard/paste** is out of reach — the headless surface has no
  selection owner to talk to.
- **Raw native event delivery** is out of reach — raw keycode
  translation (`gdk_key_to_uievent` and its macOS/Win counterparts), IME
  composition, and the native event-controller wiring are bypassed; the
  drivers inject `UiEvent`s directly, the same unified boundary
  `TuiDriver` uses.
- **Terminal-protocol / real-GL rendering paths** likewise stay with the
  live app.

These belong to the live-app smoke tier (*Live-app headless smoke*,
below) or a real-display check on the operator's hardware.

### Cross-backend tests: shared bodies, per-backend adapters

Tests should **not** be duplicated per backend. The split is hybrid —
most of a test shares, a little is irreducibly backend-specific:

| Layer | Shared? | Why |
|---|---|---|
| The `AppLogic` under test | yes — identical | backend-neutral by design |
| The event script (`press`/`click`/`drag`) | yes — identical | `UiEvent` is the unified boundary |
| Logical / state assertions | yes — identical | same app → same state on every backend |
| Reading "the screen" | **no** | TUI = character-cell grid (string search); pixel backends = a text map recorded at paint time |
| Coordinate units | **no** | TUI = cells (`line_height 1`); pixel backends = pixels (`line_height ~16`) |

`quadraui::testing::ConformanceDriver` abstracts the two backend-specific
rows. It is implemented by `TuiDriver`, `GtkDriver`, `MacDriver` and
`WinDriver`, and has no feature gate of its own, so a test body written
once as a generic fn runs against each (see *Testing a contract widget*,
layer 4, for a worked example). Its surface: `new_fixture`, `press_named`,
`type_char`, `type_text`, `ctrl_char`, `click_text`, `click_text_at`,
`drag_text`, `scroll_at`, `screen_has`, `inventory`, `backend_caps`,
`exited`.

**Rules for shared bodies:**

1. **Locate by semantics, never literal coordinates.** Use
   `click_text("Go")` / `drag_text(a, b)` / `painted_sub_center`, not
   `click(12.0, 3.0)` — each driver resolves text to its own native
   position, so bodies stay unit-agnostic. A test that hard-codes
   cell/pixel numbers will not port.
2. **Assert on logic/text, not pixels, in shared bodies.** `screen_has`
   and the `FrameInventory` relations work on every backend.

**Parity test:** run the same script on two drivers and assert they
reach the same logical state — the strongest cross-backend guarantee.
`tests/cross_backend_parity.rs` does this for `TuiDriver` and
`GtkDriver` (and `MacDriver` on the macOS CI leg), and
`tests/widget_contract_shell_view.rs` asserts that every driver
publishes the same zone set.

**Irreducible per-backend residue (keep separate, by design):** exact
pixel colours, 1px borders, font rendering, double-width glyph handling.
These genuinely differ — TUI keeps cell-style assertions; the pixel
backends get pixel checks. Don't try to share these.

### `FrameInventory`: the portable paint-inventory contract (quadraui#490)

`quadraui::testing::FrameInventory` (`quadraui/src/testing/mod.rs`, no
backend feature gate) is the backend-neutral answer to "what did this
frame paint, and where":

```rust,ignore
pub struct TextRun { pub text: String, pub bounds: Rect }
pub struct ZoneRec { pub id: WidgetId, pub bounds: Rect }
pub struct FrameInventory { /* text_runs(), zones() */ }
```

`ConformanceDriver::inventory()` returns one per frame. `text_runs` is
recovered from each backend's own paint output with no per-widget
opt-in (TUI's wide-char-aware cell-grid scan; the pixel backends' text
runs recorded as each `PaintSurface` text verb paints). `zones` holds
every `Backend::register_zone` call the frame made. A contract widget
gets zones without calling `register_zone` itself — `draw_dyn` and
`draw_primitive` register them after painting:

- an identified widget whose `Primitive::paint_zone` answers `Some` is
  registered under its own id;
- each sub-part a widget names with `Interactive::sub_region_name` is
  registered under `"<widget id>:<name>"` (`ShellView`'s chrome becomes
  `app-shell:status-bar`, `app-shell:sidebar-content`, …).

`FrameInventory` carries a small relational assertion vocabulary — the
structural fix for CLAUDE.md's no-hardcoded-coordinates rule, since
every one of these is computed on the `Rect`s the inventory already
reports, in the backend's own units, so a shared test body never writes
a coordinate itself (a fragment; `driver` is any `ConformanceDriver`):

```rust,ignore
let inv = driver.inventory();
inv.screen_has("EXPLORER");             // any text run contains this substring
inv.absent("SOURCE CONTROL");           // no text run does
inv.count("Settings");                  // how many runs do
inv.left_of("E", "EXPLORER");           // a's right edge <= b's left edge
inv.above("EXPLORER", "content");       // a's bottom edge <= b's top edge
inv.same_row("E", "S");                 // vertical ranges overlap
inv.inside("content", &WidgetId::new("app-shell:sidebar-content"));
```

**Backend checklist:** every backend driver that implements
`ConformanceDriver` MUST implement `inventory()` and populate
`text_runs` from its real paint output (not a stub), and forward every
zone the frame registered.

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
- **Deliberately thin — a handful of representative examples**
  (`tui_pipeline`, `tui_chat`, and `tui_split`/`tui_no_mouse` for the
  `no_mouse` module below), not broad coverage. This is wiring/
  integration confidence for the terminal-protocol layer; `TuiDriver`
  (Tier "example / app-wiring drift" above) remains the primary,
  deterministic tool for everything else — coordinate drift, click
  routing, state-derived paint, and per-example behavior all stay
  covered there.
- **Runs as a real CI gate**, not an operator-only tier like the GTK
  live-app smoke below. A pty is a kernel device, not a display server —
  no Xvfb, no compositor, works headlessly anywhere `openpty` does.
- **Byte-exact escape-sequence assertions are `#[cfg(unix)]`** — the
  `sgr_color_depth` module (quadraui#826's SGR colour-depth fixtures),
  which reads the raw master-side bytes via `raw_contains`/`wait_for_raw`
  rather than the parsed screen. On Unix, `openpty(3)` is a kernel byte
  pipe: what the child writes is what the master reads, so those bytes
  really are "what a terminal would receive from our app". On Windows,
  `portable-pty` is backed by ConPTY (`CreatePseudoConsole`), which puts
  a whole conhost terminal emulator in between — the master reads
  conhost's re-serialised repaint of its own text buffer, with its own
  SGR encoding and batching, not the child's bytes. Content-level
  assertions (`wait_for`/`screen_text`) survive that round trip and stay
  cross-platform; byte-exact ones cannot. **If you add a raw-byte
  assertion, put it in that module.** The platform-independent half of
  the same claim — that a painted cell is actually quantised to the
  detected depth — lives in `src/tui/color.rs`'s
  `pipeline_theme_pair_quantises_through_a_real_draw_at_every_depth`,
  which does run on Windows.
- **Fixtures that control input *byte boundaries* are `#[cfg(unix)]` too**
  — same ConPTY reason, mirrored onto the input direction, and the gate
  belongs on the test rather than on a module here.
  `tui_chat_escape_glued_to_sgr_motion_in_one_write_does_not_leak`
  (quadraui#293) works only because it can glue a lone `ESC` and an SGR
  mouse report into a single `write()`, which is the input crossterm's
  Unix event source then has to disambiguate
  (`parse_event(buffer, input_available)` — #293's actual mechanism). On
  Windows crossterm never parses bytes at all: `event::source::windows`
  reads `INPUT_RECORD`s through the console API, and ConPTY has already
  re-parsed the master-side bytes with conhost's own VT input state
  machine before the child sees anything. The byte boundaries the test
  constructs therefore do not survive to the code under test, and a
  failure there reports on conhost, not on quadraui. **Rule of thumb: if
  a pty fixture's premise is "these exact bytes, in this exact grouping"
  — in either direction — gate it `#[cfg(unix)]` and cover the logic
  itself with an in-crate unit test that runs everywhere** (here:
  `src/tui/backend.rs`'s `recover_leaked_sgr_mouse_fragments` tests).
  Content-level pty assertions ("typing works", "the report didn't leak
  into the transcript") stay cross-platform and still run on
  windows-latest.

## Conformance tiers (C0–C4) and the scenario suite (quadraui#491)

The conformance suite is **declarative** scenarios, run against **every**
registered backend, producing a scenario × backend matrix. It
complements — does not replace — the widget and example-driver tests
above.

**A new conformance fixture does not waive the per-example obligation.**
When a scenario's fixture is a brand-new `tui_*` example (CLAUDE.md's
"Demos are mandatory for visual features" / "Every TUI example also
ships an automated black-box test"), that example still needs its own
`TuiDriver` cluster in `tests/tui_example_driver.rs`. The two check
different things: the driver test is a hand-written, backend-specific
regression net over that one example's behaviour; the scenario is the
same interaction expressed once and proven identical across every
registered backend. Neither substitutes for the other.
(`tui_modal_occlusion` / `ModalOcclusionDemo` — the fixture behind
`dialog.blocks_click_through` — ships both: see the `modal_occlusion_*`
tests in `tests/tui_example_driver.rs`.)

### The tiers (a new backend's burn-down checklist)

- **C0 — Boot (day one).** Construct the backend headless;
  `begin_frame`/`end_frame`; sane viewport; draw every case in
  `tests/conformance/c0.rs`'s `CASES` once with a canned descriptor — no
  panic **and** an observable frame (non-empty `text_runs()` for a
  text-bearing case, or a registered zone). This turns silent no-op
  paint paths into visible red/green immediately.
  <!-- TODO(0.12 final pass): after W9, re-read tests/conformance/c0.rs. Today CASES holds one entry per legacy Backend draw_* method and `cases_cover_every_draw_method_on_the_trait` enforces that; once the per-widget methods are gone the cases are `draw_dyn(<Type>)` and the coverage rule changes. Describe what CASES must cover then, and name the test that holds it. -->
- **C1 — Interaction core (mandatory for "complete").** The Tier-1
  scenarios: click routing, keyboard focus, modal occlusion,
  scroll-under-cursor, split relayout, tab close, menu
  open/navigate/activate, palette type/pick, toast dismiss, status-bar
  updates, viewport reflow — the scenario files under
  `tests/conformance/scenarios/`.
- **C2 — Event parity.** For each required `UiEvent` variant, a
  per-backend native-injection recipe proving it is emitted. Required
  (windowed backends — GTK/Win/macOS):
  Key/Char/MouseDown/Up/Moved/Scroll/DoubleClick/WindowResized/
  WindowClose/Accelerator/ClipboardPaste/TextCopied. TUI has no OS
  window, so `WindowClose` doesn't apply there — see
  `quadraui/docs/decisions/DECISIONS.md` D-010. Optional (declare, don't
  fake): FilesDropped, MouseEntered/Left, DpiChanged, native menu events.
  The full required/optional-per-backend table lives in
  `quadraui/docs/BACKEND.md`'s "UiEvent emission matrix".
  `tests/conformance/c2.rs` is the mouse/key/scroll/resize slice of this
  tier's harness on TUI+GTK. The widget-level half of event parity
  (runtime vs direct `handle` vs a11y) is per widget — see the coverage
  taxonomy.
- **C3 — Platform services.** Clipboard round-trip (headless where
  possible), capability-honest dialogs and notifications.
- **C4 — Native residue (never shared).** Exact colours, font rendering,
  wide-glyph pixels, live-window smoke.

### Running it

```sh
cargo test -p quadraui --features tui      --test conformance -- --nocapture
cargo test -p quadraui --features gtk,tui  --test conformance -- --nocapture
```

The Tier-1 matrix prints on every run and is also written to
`$CARGO_TARGET_DIR/conformance-matrix.txt`; the Tier-0 grid
(`c0_paint_smoke`) goes to `$CARGO_TARGET_DIR/conformance-matrix-c0.txt`
— two files, because both tests can run concurrently in the same
`cargo test` invocation and a shared path would let whichever finished
last clobber the other's artifact. CI uploads both from the `tui` and
`gtk` jobs, plus the `win` tables from the windows-latest leg. For a
backend under construction, that artifact is the implementation
checklist.

```text
Conformance matrix (scenario × backend)
scenario                         tier  tui   gtk
-------------------------------------------------
pipeline.click_advances_stage       1  pass  pass
dialog.blocks_click_through         1  pass  pass
...
```

### Burn-down columns: registered, reported, not gating (quadraui#708)

A backend whose paint paths are still landing needs a column *before* it
can pass one — otherwise "the artifact is the implementation checklist"
is impossible to satisfy, because adding the column would red every PR on
that platform's CI leg. `runner::Gating` separates the two decisions:

- `BackendReg::register::<F>(name)` — **blocking.** A `FAIL` cell fails
  `conformance_matrix`. Every scenario-suite column (`tui`, `tui-vt100`,
  `gtk`, `macos`, `win`) is registered this way.
- `BackendReg::register_burn_down::<F>(name)` — **reported, not gating.**
  Cells render identically in the matrix and the detail block, the
  artifact still carries them, and a legend under the table names every
  non-gating column so a reader can never mistake one for a regression.
  A newly added backend starts here.

Burn-down is self-expiring, not a permanent exemption: if a burn-down
column runs at least one scenario and fails *none* of them,
`conformance_matrix` fails with "promote it to `BackendReg::register`".
An all-`skip` column doesn't count as having run — a backend that
declares no capabilities can't earn promotion by never attempting
anything.

**`c0_paint_smoke` shares the same `runner::Gating` enum**, applied to
its per-case grid: each column in that test's `Column` list carries a
`gating` field. `win` is `Gating::BurnDown` there today; its
`PANIC`/`FAIL` cells print on a "non-gating C0 gap(s)" line separate from
the blocking gap list and are excluded from the assertion. The same
legend and the same self-expiring promotion check apply: a burn-down
column with zero failing cases fails `c0_paint_smoke` until it is moved
to `Gating::Blocking`. There is no capability-skip concept at this tier
— every case always runs — so only "ever failed" counts.

### TUI: two observers, one row each (quadraui#555)

Every tier above — including the conformance suite's own `"tui"`
column — observes the TUI backend at **draw time**: `TuiDriver` reads
`TestBackend`'s retained `ratatui::buffer::Buffer` straight back after
`app.render` returns. That answers "what did the rasteriser ask for?" It
cannot answer "what would a real terminal actually show?", and for a
cell-grid backend those can diverge — a double-width (CJK/emoji) glyph's
reserved continuation cell, a malformed buffer, or an ANSI-encoding bug
can each cause the two to disagree, and #302's own module doc names the
general form: `TestBackend` "never touches a real TTY, so
terminal-protocol bugs are invisible to it."

```sh
cargo test -p quadraui --features tui,terminal --test conformance -- --nocapture
```

adds a **second** TUI column, `"tui-vt100"`
(`quadraui::tui::vt_testing::TuiVtDriver`): the exact same `AppLogic`
fixture, replayed through the exact same production render path
(`tui::run::paint_frame`), but painted through
`ratatui::backend::CrosstermBackend` into a real ANSI byte stream and
read back with [`vt100`](https://docs.rs/vt100) — the same crate
Tier-3's `tui_pty_smoke.rs` uses, minus the OS pty: `TuiVtDriver` drives
its fixture with scripted `UiEvent`s exactly like `TuiDriver`, never real
keystrokes, so there is no terminal *input* to decode and therefore
nothing that needs a live pty to answer a real terminal's queries (see
`quadraui::tui::vt_testing`'s module doc for exactly which query that
would be and how `TuiVtDriver` avoids it — `CrosstermBackend::size()`
reads `/dev/tty`, unrelated to whatever `Write` sink the backend wraps).
The result is deterministic and needs no subprocess, matching what makes
`TestBackend`-based `TuiDriver` fast, just with a real ANSI byte stream —
not a same-process buffer read — in the loop. `tui_pty_smoke.rs`'s real
pty remains the tier that also covers terminal-*input* protocol (raw-mode
setup, SGR mouse decoding); `TuiVtDriver` is scoped to output fidelity.

**Not every scenario pays for it.** A `.scn.json` file only runs against
`"tui-vt100"` if it declares `"text_fidelity": true` (`schema.rs`) — the
matrix has no cell at all (`-`, not `skip`) for a scenario that didn't
opt in, so the bulk of the suite stays `TestBackend`-only. Reach for the
opt-in when a scenario's assertions are actually about text fidelity,
wide (double-width) characters, or cursor placement — most scenarios
aren't and shouldn't pay the extra render.

```text
scenario                         tier  tui   tui-vt100
--------------------------------------------------------
tabbar.wide_label_text_fidelity     1  pass  pass
tabgroup.close_tab                  1  pass  -
```

`tests/conformance/scenarios/tabs/tabbar.wide_label_text_fidelity.scn.json`
is that worked example — a `TabBar` whose active tab's label contains a
CJK filename mid-string (`" 1: 日本語.rs "`, quadraui#554's original
report), asserted with the ordinary `assert_screen_has`/`assert_left_of`
vocabulary against both observers unmodified. Building it against
`"tui-vt100"` is what caught quadraui#555's own finding: `TuiDriver`'s
own `screen()`/`screen_has` — the direct backer of `assert_screen_has` —
walked the buffer one column at a time and so inserted a phantom space
after every double-width glyph (the glyph's reserved continuation cell,
which is always blank by construction and never actually reaches
`TestBackend`'s retained buffer, since `Buffer::diff` skips emitting it).
That made the exact same painted content read as *absent* through
`screen_has` while `find_bounds`/`inventory` — already wide-char aware
since quadraui#488 — found it painted as one contiguous run. `TuiDriver`
now strides `screen()` the same way, so both observers agree; see
`tui::testing::screen_is_wide_char_aware_for_adjacent_cjk_glyphs` for the
regression guard. The tab-bar *measurement* bug quadraui#554 itself was
filed against was already fixed at the source (`display_width`-based tab
budgeting in `tui/backend.rs`) before this tier landed, so both observers
already agreed there — this file's value ended up being the `screen()`
gap it caught along the way, which is the general principle #555 argues
for: a second, more-realistic observer earns its keep exactly by
occasionally disagreeing with the first.

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
| `click_text`, `click_text_at`, `click_text_any` | `assert_left_of`, `assert_above`, `assert_inside` | |
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

`tests/conformance.rs` (a fragment; `my::testing::MyDriver` stands for the
new backend's driver):

```rust,ignore
struct MyFactory;
impl runner::DriverFactory for MyFactory {
    fn make<A: AppLogic + 'static>(app: A, vp: LogicalViewport) -> Box<dyn runner::DynDriver> {
        Box::new(my::testing::MyDriver::new_fixture(app, vp))
    }
}
// … and, in `backends()`:
regs.push(BackendReg::register_burn_down::<MyFactory>("my"));
```

Everything else — the fixture registry, the step interpreter, the
matrix — is backend-agnostic. `DynDriver` is the object-safe view of
`ConformanceDriver` (which is `Sized`, so it can't be a trait object);
one blanket impl covers every present and future driver. The new
backend's C0 column is one more `columns.push` in `c0_paint_smoke`.

### Capabilities: a skip must name what's missing

A scenario's `requires` lists capabilities by `BackendCaps` field name
(`mouse`, `scroll`, `drag`, `text_selection`, …). A scenario that
requires a capability the backend doesn't declare is **skipped, with the
missing capability printed in the matrix detail block**. A backend
therefore cannot quietly not-run a scenario: either it declares the gap,
or the scenario runs and fails.

The capability list is not written in the test file. `DriverFactory::caps`
builds a throwaway driver and reads `Backend::backend_caps()` off the real
backend, so a registration can't claim a capability the backend doesn't
declare. `every_requires_names_a_known_capability` keeps scenario files
on the `BackendCaps` vocabulary.

### Zone-backed assertions: what is registered today

`assert_inside` is the one step that does **not** work off painted text
— it needs a `WidgetId`-keyed rect, which the frame must register (see
*`FrameInventory`* above for how a contract widget gets one). The set of
assertable zone ids is therefore explicit:

| Zone id | Registered by | When |
|---|---|---|
| `<id>` | any identified widget whose `Primitive::paint_zone` answers `Some` | every frame it paints |
| `<id>:<name>` | any widget that names a sub-part with `Interactive::sub_region_name` | every frame that sub-part exists |
| `app-shell:window`, `app-shell:activity-bar`, `app-shell:main-content` | `ShellView` (id `app-shell`) | always |
| `app-shell:title-bar`, `app-shell:sidebar-header`, `app-shell:sidebar-content`, `app-shell:divider`, `app-shell:bottom-panel`, `app-shell:command-line`, `app-shell:status-bar` | `ShellView` | when that region exists this frame |
| the panel's own id, e.g. `panel:explorer` | the app shell, one per activity-bar item | every frame |

<!-- TODO(0.12 final pass): after W7 (#2958 deletes AppShell), confirm what registers the per-panel activity-row zones (`panel:explorer`, …) — today `AppShell::render` registers them by hand — and update the last row. -->

Every driver forwards whatever the frame registered, so a zone available
on one backend is available on all; `tests/widget_contract_shell_view.rs`
asserts that the `app-shell:*` set agrees across drivers.

**Naming a zone nothing registers makes the step unsatisfiable, not
flaky.** `FrameInventory::inside` returns `false` for an unregistered
zone exactly as it does for a run that landed outside a registered one,
so the matrix alone can't tell the two apart. Two things close that gap:
`every_asserted_zone_is_registered_by_every_backend` (in
`tests/conformance.rs`) fails with "backend never registers this id" and
a list of what *is* registered, and the `assert_inside` failure text
itself distinguishes never-registered / never-painted / outside-the-rect.
Before writing an `assert_inside` against a new widget, give it a
`paint_zone` or a `sub_region_name` first.

### Known coordinate-free gaps

- **Split-divider drag.** A split divider has no text on the pixel
  backends — it's a filled rect — and publishes no zone, so there is
  nothing for `drag_text` to name. `split.direction_toggle_relayout` and
  `split.resize_via_keyboard` cover split relayout relationally instead.
  Unblocked by a divider zone plus a zone-anchored drag step.
- **Editor click-to-caret** needs a caret-position read-back on
  `ConformanceDriver`.

<!-- TODO(0.12 final pass): after W5 (#2968 ports Split/SplitTree onto the contract), check whether the divider is now a named sub-region published as a zone; if so, rewrite the split-divider gap. -->

## Backend testability requirement

Every backend MUST support headless paint-to-memory so tests don't
need a real display, terminal, window manager, or font server.

- TUI: `ratatui` `TestBackend` buffer (in-memory char + style cells),
  reached by widgets through `CellSurface`.
- GTK: `cairo::ImageSurface::create(Format::ARGB32, w, h)` + Pango
  layout queries.
- macOS: `quadraui::macos::headless::BitmapSurface` (CGBitmapContext +
  pixel readback, top-left origin matching `QuadraView`).
- Windows: `quadraui::win::testing::HeadlessSurface`, a GPU-free Direct2D
  render target with pixel readback.

A new backend ships with its harness on day one: a driver implementing
`DriverInput` and `ConformanceDriver`, including `inventory()` (see the
`FrameInventory` backend checklist above), and a column in both
conformance tests.

## Backend `todo!()` gate (quadraui#923)

`tests/backend_todo_gate.rs` guards against a specific latent-abort
shape: an un-cfg-gated `todo!()`/`unimplemented!()` inside one of the
four real per-platform `Backend` impls (`TuiBackend`, `GtkBackend`,
`MacBackend`, `WinBackend`) that is reachable when that backend is
compiled for its own native target. That shipped once — three macOS
methods had a bare `todo!()` and aborted vimcode's macOS GUI on first
paint (vimcode#896, fixed in `dbb3023`). A raw `todo!()` count can't tell
that case apart from Win-GUI's legitimate `cfg`-gated stubs (there so
`cargo check --features win` can type-check `WinBackend` on Linux), so
the test classifies each call site instead.

It parses each backend's source with `syn` (a `#[cfg(...)]` binds to
exactly one following statement or item — see the test's module doc for
the sibling-statement shape a proximity check gets wrong) and counts the
**dangerous** sites: un-gated and reachable on the backend's native
target. The test prints the per-backend count on every run. Known Win-GUI
debt is frozen in `KNOWN_DANGEROUS_TODOS`; any dangerous site not in that
baseline fails the test.

**Recognised `cfg` spellings** (anything else is refused as
`Unclassifiable` — a hard test failure naming the exact attribute, never a
silent pass in either direction):

- `cfg(test)` — exempt everywhere (never compiled outside a test build).
- `cfg(not(test))` — not exempt (this is the shipped arm).
- `cfg(target_os = "X")` — exempt iff `X` differs from the file's own
  backend's native `target_os` (`"macos"` for `src/macos/`, `"windows"`
  for `src/win/`).
- `cfg(not(target_os = "X"))` — exempt iff `X` matches that native
  `target_os`.

**Refused, not classified:** `cfg_attr(...)` in any form, `cfg(any(...))`,
`cfg(all(...))`, `cfg(not(any(...)))`/`cfg(not(all(...)))`, bare
`cfg(unix)`/`cfg(windows)`, a `target_os` comparison inside `src/gtk/` or
`src/tui/` (no native `target_os` is defined for those — they ship
cross-platform), or a `cfg(...)` whose inner tokens don't parse as a
`syn::Meta` at all.

**Scope:** only `src/tui/`, `src/gtk/`, `src/macos/`, `src/win/` — the
`impl Backend` blocks reachable from `quadraui::{tui,gtk,macos,win}::run`.
`src/testing/mod.rs`'s `RecordingBackend` (a `pub` `Backend` mock for unit
tests) and private `#[cfg(test)]` mocks are out of scope.

**Statement-position macros are covered too.** `syn` parses a
semicolon-terminated macro used as a mid-block statement —
`todo!("x"); more_code();` — as a `Stmt::Macro` node, not the
tail-expression shape. The walker's `visit_stmt_macro` override runs the
same classification for it; see `backend_todo_gate.rs`'s `self_test`
module for the pinned fixtures. (A per-match-arm `#[cfg(...)]` is the
same class of gap in principle — there is no `visit_arm` override — but
no backend uses that pattern, so it is undetected-but-inert.)

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

## Win-GUI C4 playbook (native residue, operator-run, quadraui#746)

C4 ("Native residue — never shared" in the [tiers table above](#the-tiers-a-new-backends-burn-down-checklist))
is exact colours, font rendering, wide-glyph pixels and live-window
smoke — none of it assertable from shared code by design. This section
is the manual pass an operator runs on `dell64` (see CLAUDE.md's
"Win-GUI: building and testing for real"), plus the one-time question
that motivated writing it down.

### Step 1 (done, 2026-09-04): the Wine question, settled

The concern going in: `dell64`'s automated `cargo xwin test ...`
path runs the cross-compiled `.exe` through *something* other than a
human logging into Windows, so a green run there might be an artifact
of emulation rather than proof the rasteriser works — Direct2D/
DirectWrite support under a generic Linux binfmt+Wine setup is
partial, and a red *or* green result under Wine wouldn't say much
about real Windows either way.

**That premise doesn't hold on this box, and it's worth recording why
rather than re-asking the question per issue.** `dell64` has no Wine
installed at all:

```console
$ which wine wine64
wine not found
wine64 not found
```

The binfmt interpreter CLAUDE.md's trap #2 refers to
(`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env`, letting
`binfmt_misc` "do the exec directly") is **WSL2's own interop
handler**, not a userspace PE loader:

```console
$ cat /proc/sys/fs/binfmt_misc/WSLInterop
enabled
interpreter /init
flags: P
offset 0
magic 4d5a
```

`/init` hands the process to the real Windows host over the WSL
interop pipe — confirmed by shelling out to the actual Windows side
(`/mnt/c/Windows/System32/cmd.exe`, not a wrapper) and reading back
its own idea of itself:

```console
$ /mnt/c/Windows/System32/cmd.exe /c "ver & hostname"
Microsoft Windows [Version 10.0.26200.9168]
dell64
```

That's a real, installed Windows 11 build on real hardware named
`dell64` — the same host CLAUDE.md already names as "physically to
hand." **So the `cargo xwin test --target x86_64-pc-windows-msvc
-p quadraui --features win` / `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env`
path is not an emulator run to be distrusted — it already executes
the compiled binary against the genuine NT kernel and the genuine
Direct2D/DirectWrite stack.** A pass there (re-run today as this
issue's step 1, same command CLAUDE.md documents):

```console
$ RUSTFLAGS="-C target-feature=+crt-static" \
  CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env \
  cargo xwin test --target x86_64-pc-windows-msvc -p quadraui --features win
...
test result: ok. 1760 passed; 0 failed; ...           # lib unit tests
     Running tests/conformance.rs
test result: ok. 39 passed; 0 failed; ...              # C0 + C1, win burn-down column
     Running tests/win_example_driver.rs
test result: ok. 5 passed; 0 failed; ...
     Running tests/win_sxs_manifest.rs
test result: ok. 3 passed; 0 failed; ...
```

**Consequence: C0–C3 numbers gathered via the documented `dell64`
runner are real-Windows evidence already — they do not need a second,
separate "was that actually Windows" re-check.** What that run
*cannot* prove is anything requiring a visible window or an
interactive desktop: `HeadlessSurface` never creates an `HWND`, and a
`cargo xwin test`-launched process may run outside the interactive
window station even though it's genuine native code. That residual
gap — not "is it Wine", but "is there a window and a human looking at
it" — is exactly C4, and is what the checklist below covers.

**Doctests need `RUSTDOCFLAGS` too.** `cargo xwin test`'s doctest
binaries fail with exit 53 — the "missing vcruntime" signature of
CLAUDE.md's trap #1 — when only `RUSTFLAGS` carries `crt-static`:
`rustdoc` compiles each doctest itself and reads **`RUSTDOCFLAGS`**,
not `RUSTFLAGS`. The full command, all three variables required:

```bash
RUSTFLAGS="-C target-feature=+crt-static" \
RUSTDOCFLAGS="-C target-feature=+crt-static" \
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env \
  cargo xwin test --target x86_64-pc-windows-msvc -p quadraui --features win
```

`tools/win-test.sh` wraps exactly that line so none of the three has to be
retyped from memory:

```console
$ tools/win-test.sh
...
   Doc-tests quadraui
test result: ok. 11 passed; 0 failed; 22 ignored; ...
```

**Run the doctest leg through `tools/win-test.sh`, not by retyping the
env-var prefix.** The Windows run needs three environment variables and
the doctest leg is the part that notices when one is missing — while
all 20 integration-test targets sail past. Worse, *two different*
missing variables produce the *same* headline number, `9 failed` out of
11 (the two survivors are the `compile` / `compile fail` doctests,
which are never executed), so the count alone cannot tell you which:

| Missing variable | What each failing doctest prints |
|---|---|
| `RUSTDOCFLAGS="-C target-feature=+crt-static"` | `Test executable failed (exit status: 53)` |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env` | `Couldn't run the test: No such file or directory (os error 2)` — rustdoc got cargo-xwin's hard-coded `wine` as its `--runtool`, and there is no wine on this host |

Both were hit for real: the first by #832's own smoke run, which
reported "9 of 11 doctests failed" and sent a worker looking in the
diff. The wrapper exists so neither can happen again;
`tools/lint` (`quadraui-repo-lint`, #1110's meta-test-relocation half —
moved out of `quadraui/tests/` so an outside contributor's `cargo test`
never fails on it) asserts
`tools/win-test.sh` keeps setting all three variables and keeps running the
same cargo line spelled out above.
Note that a `.cargo/config.toml` would *not* have prevented the second
one — cargo-xwin injects the runner into cargo's environment, and env
beats config.

**A fifth trap, unrelated to the above: `<binary>.exe: Invalid argument`
is the Windows host's code-integrity policy, not a broken build.** A
freshly-linked, unsigned `.exe` is sometimes refused by Device Guard /
Smart App Control while its reputation is still being resolved;
`binfmt_misc` surfaces that refusal as `EINVAL`, so the shell prints
`…exe: Invalid argument` and cargo reports `test failed` for a test
binary that never ran a single test. Confirm it with
`/mnt/c/Windows/System32/cmd.exe /c "<C:\ path to a copy> --list"`, which
prints the real reason (`was blocked by your organization's Device Guard
policy`); diffing the PE headers against a binary that *does* run will
show nothing, because nothing is wrong with it. **Remedy: re-run the
command.** The block clears on its own — the identical bytes exec fine a
minute later. It is intermittent and picks different victims each run
(doctest binaries especially, since each is freshly linked into a new
`/tmp` dir), so treat a lone `Invalid argument` as a flake and only
investigate if the *same* target fails a re-run.

### Manual pass checklist

Run after any change touching `src/win/backend.rs`'s rasterisers, and
otherwise periodically as a spot-check. Needs an interactive `dell64`
desktop session (RDP or console) — `cargo xwin run`, not `xwin test`,
so a real `HWND` opens:

```sh
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo xwin run --target x86_64-pc-windows-msvc --example win_appshell_demo --features win
```

1. **Theme colours actually applied.** Every contract widget paints
   with the theme `draw_dyn` reads from `Backend::theme()`, so after
   `WinBackend::set_theme` the whole window should repaint in the new
   colours. Switch to a non-default theme and confirm the tab bar,
   status bar, activity bar, a text input's caret and selection, and a
   scrollbar thumb all follow it. Anything still painting default
   colours is a bug, not a known gap.
2. **Font rendering and hinting.** Compare label and editor text at
   100% and 150% Windows display scaling (Settings → System → Display)
   against the same text on the `tui`/`gtk` backends run side by side.
   Look for: correct weight (no accidental bold/thin substitution),
   no clipped descenders/ascenders, hinting that doesn't blur at
   small sizes.
3. **Wide-glyph / CJK / emoji advance** (pairs with #703's shared
   scale logic, `src/win/text.rs`). Use `win_appshell_demo`'s editor
   pane: type or paste a line mixing
   ASCII with CJK (`" 1: 日本語.rs "`, the same fixture
   `tabbar.wide_label_text_fidelity` uses) and an emoji. Confirm each
   wide glyph occupies two cell-widths' worth of advance with no
   overlap into the next character and no stray gap — the exact
   failure mode #703 fixed for `gtk`/`macos`/`tui`.
4. **Live-window resize / DPI-change behaviour.** Drag-resize the
   window across a wide range (including down to something absurdly
   small) and confirm chrome relayouts without artifacts or a crash.
   Drag the window between two monitors at different scale factors (or
   change scaling live via Settings while the window is focused) and
   confirm text/chrome re-rasterises crisply at the new DPI rather than
   staying blurry-scaled from the old one.
5. **Cursor shapes.** Hover each interactive zone (resize edges, text
   inputs, buttons, splitters if the example under test has any) and
   confirm the OS pointer glyph changes appropriately (I-beam over
   text, resize arrows over splits/edges, hand/pointer over buttons)
   and reverts on leave.

### Where to record results

Log a dated entry **in this section** (not a separate file — a later
reader should not have to hunt) each time the manual pass above is
run, so "verified natively" is distinguishable from "green in CI" at a
glance:

| Date | Rev | Run by | Result |
|---|---|---|---|
| 2026-09-04 | `9452d50` (branch tip before this doc commit — the code state actually exercised by the `cargo xwin test` run below) | operator (dell64) | Step 1 (Wine question) done — see above. Manual checklist (1–5) not yet run; no interactive `dell64` session available for this pass. |

CI's `windows-latest` leg and `cargo xwin test` on `dell64` both prove
"compiles and runs headless" — neither is a substitute for a row in
this table. Only a row here means a human actually watched the window.

## What unit tests don't cover

Animation cadence, font-rendering quirks across host platforms,
terminal-specific edge cases (kitty vs xterm vs urxvt), exact color
choices, accessibility heuristics, "does this feel right". These
remain manual smoke / human review. Goal: every story ratchets
harness coverage forward so the manual-residue surface shrinks
toward "things that genuinely need eyes".

- **Native message-dialog visibility** (`PlatformServices::show_message_dialog`,
  quadraui#666). `GtkDriver` paints `cairo::ImageSurface` offscreen — it
  never opens a real GTK `Application`/window, so it structurally cannot
  see a `gtk4::AlertDialog`, which is a *separate* native window GTK owns
  outside the `DrawingArea` this crate rasterises into. `scripts/gtk_smoke.sh`'s
  generic live-window check (window/`DrawingArea` size floor, optional
  clipboard round-trip — see "Live-app headless smoke" above) doesn't open
  a dialog either, so it proves the window itself is alive, not that the
  alert appears or that its buttons behave. Same structural gap as
  `PlatformServices::show_file_open_dialog`/`show_file_save_dialog`/
  `show_folder_open_dialog` (#427, #935), which has never had automated
  coverage of the native picker actually appearing. Manual smoke, until a
  `GtkDriver` successor can see native windows:
  1. `cargo run --example gtk_message_dialog --features gtk`
  2. Press `m`. Confirm a real OS dialog titled "Discard unsaved
     changes?" appears, parented to (centered on) the demo window, with
     "Keep Editing" right of "Discard" (GNOME HIG: cancel leftmost,
     default rightmost).
  3. Click each button in turn (re-running step 1 between clicks) and
     confirm the status bar reports "Kept editing" / "Discarded"
     matching the click. Press `m` again and dismiss via Escape or the
     window's close box; confirm the status bar reports "Cancelled".
  4. TUI is **not** part of this manual-only gap:
     `show_message_dialog` drives a real in-canvas
     `compose::MessageDialogController` through a nested draw-and-read
     loop (see `tui::services`'s module doc), and the `TuiDriver` tests
     in `tests/tui_example_driver.rs` cover the full round trip
     headlessly — a real button resolves, Escape resolves the cancel
     button, no hang. `cargo run --example tui_message_dialog --features
     tui` and press `m` is still worth a manual sanity check (arrow/Tab
     between buttons, Enter, Escape, confirm the status bar reports
     "Kept editing"/"Discarded" matching the choice) but is no longer
     the *only* coverage the way GTK's native-window gap above still is.
  5. **Windows** (quadraui#744 — `show_message_dialog` is real here,
     `native_dialogs: true`): `cargo run --example win_platform_services
     --features win` on `dell64` (see this doc's "Win-GUI: building and
     testing for real" note in the repo's `CLAUDE.md`) and press `m`.
     Confirm a real `TaskDialogIndirect` alert titled "Save changes?"
     appears with Save/Don't Save/Cancel buttons (Save bold/default).
     Click Save and Don't Save in turn (re-pressing `m` between clicks)
     and confirm stderr reports `chose WidgetId("save")` /
     `chose WidgetId("dont_save")` matching the click. Press `m` again
     and dismiss via Escape or the dialog's close box; confirm stderr
     reports `chose WidgetId("cancel")` both times — that button is
     marked `is_cancel`, so the dismissal gesture round-trips to it
     rather than reporting "dismissed with no choice".

- **macOS tests still depend on the host having Menlo installed**
  (quadraui#1110, second half — deliberately **not** done by the
  `tools/lint` extraction above). `MacDriver::new` (`src/macos/testing.rs`)
  installs `make_font("Menlo", 14.0)` as the default font before every
  headless macOS test runs, mirroring what `macos::run` installs for a
  real window, and roughly a dozen individual tests additionally call
  `make_font("Menlo", …).expect("Menlo installed on every macOS host")`
  directly (`macos/text.rs`, `macos/status_bar.rs`, `macos/minimap.rs`,
  `macos/toolbar.rs`) — 82 `"Menlo"` string literals across 37 files in
  `src/macos/` in total (`rg '"Menlo"' quadraui/src/macos | wc -l`), most
  of them reached transitively through `MacDriver::new`. `#929`'s
  `Backend::register_font_from_memory` (`macos::text::register_font_from_memory`)
  exists and is exercised by its own unit tests, but nothing wires an
  embedded test-font's bytes into `MacDriver::new` or the direct
  `make_font("Menlo", …)` call sites yet — a fork/CI runner without
  Menlo (any non-macOS-with-full-font-set host, or a locked-down macOS
  image) still fails the whole `--features macos` test leg on font
  lookup, not on anything the PR under test actually changed. Doing this
  right means picking (or generating) an embeddable monospace font,
  re-deriving every pixel-metric assertion tuned to Menlo's actual glyph
  advances against the embedded font's metrics instead, and re-verifying
  `cargo test --features macos` on a real macOS host — large enough that
  it needs its own tracked issue and its own PR rather than riding along
  with the `tools/lint` extraction (which only moves *maintainer-workflow*
  meta-tests, and touches nothing under `src/macos/`). Today this is
  mitigated, not solved: `.github/workflows/macos.yml` only runs on
  `macos-latest`, which ships Menlo, so CI itself doesn't hit this gap —
  it is a real gap for anyone running `cargo test --features macos` on a
  macOS host that lacks it.
