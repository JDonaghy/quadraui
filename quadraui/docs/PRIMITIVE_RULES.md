# Primitive Authoring Rules

Read this when adding or changing a widget.

## What a widget is

A widget is one Rust type with one implementation that every backend paints:

- a **descriptor** — the struct an app builds each frame (`Button`, `ListView`,
  `TreeView`). It holds what the app owns: content, selection, open/closed,
  scroll position, disabled.
- **`Primitive`** (`quadraui::primitive`, plus its `A11yPrimitive` supertrait)
  — the paint half. `layout()` resolves every geometry decision once into
  `Self::Layout`; `paint()` draws that layout through `&mut dyn PaintSurface`
  on GTK, macOS and Win; `paint_cells()` draws it through
  `&mut dyn CellSurface` on TUI.
- **`Interactive`** (`quadraui::interactive`) — the behaviour half.
  `handle()` turns a `RoutedEvent` into a semantic `Action` (which becomes a
  `UiEvent`), using the same `Self::Layout` the paint used. A widget with no
  behaviour uses `non_interactive!(Type)`.

Backends know no widget. A pixel backend implements `PaintSurface` (fill,
stroke, text runs and text lines, images, clip); TUI implements `CellSurface`
(cells, borders, cursor, inline graphics). `Backend` itself carries only
frame, font, metric, input and platform-service methods, and no widget
needs one of them added. A leaf widget lives in
`quadraui/src/primitives/<name>.rs`; a composite assembled from other widgets
(a dialog, a picker, an app shell) lives in `quadraui/src/compose/`.

## The rules

1. **Declarative description first.** The descriptor, its `Layout` type and
   any `hit_test` on that layout go in `primitives/<name>.rs`. Layout tests go
   at the bottom of that file and must not need a backend feature.
2. **One source of truth for layout.** `Primitive::layout` is the only place
   geometry is computed. `paint`, `paint_cells`, `handle`, `sub_regions`,
   `popups` and every test consume the `Layout` it returns. Backend metrics
   reach it through `LayoutCtx` (`units()`, `line_height`, `char_width`,
   `text_width()`, `style()`), never through a backend type. `layout()` is pure:
   the same `ctx` and `rect` give the same layout, which is what lets a host
   re-resolve it with `DrawsPrimitives::layout_primitive` to hit-test.
3. **Every backend, one painter each.** Implement `paint` for the pixel
   backends and `paint_cells` for TUI, both from the same `Layout`. Shared
   painters live beside the descriptor (the built-in widgets use a
   `native_surface_paint` module and a `cell_paint` module). Nothing is added
   under `src/{tui,gtk,macos,win}`: if a painter needs something a surface
   cannot do, add the verb to `PaintSurface` or `CellSurface` for every
   backend. Leaving `paint_cells` as its default no-op means the widget does
   not exist on TUI, which the *TUI is a first-class backend* rule in
   `CLAUDE.md` forbids for anything user-facing.
4. **A black-box paint/route test.** Every widget ships
   `tests/widget_contract_<name>.rs`: a small `AppLogic` that paints the widget
   as `Surface::Widget`, returns it from `frame_tree`, and drives it with
   `TuiDriver` — click what is painted (found with `find()`, never a
   hard-coded coordinate), press keys, and assert the `UiEvent` the app
   receives and the screen that results. Verify the test catches its bug class
   by breaking the painter or the `handle` arm it covers and watching it fail.
   A widget whose look changes with hover or press also joins the
   cross-backend `HoverBoard` (`tests/common/hover_board.rs`, driven by
   `tests/hover_channel_{tui,gtk,macos,win}.rs`).
5. **Behaviour lives in `Interactive`.** `handle` receives events already
   resolved against the widget: `RoutedEvent::Pointer`/`Wheel` carry `local`
   coordinates (origin at the widget's rect), and `Pointer` also carries the
   `sub` part under the pointer. Declare tab order with `focus()` (`FocusSpec::None`, `Stop`, or
   `Children` for per-row or per-cell stops). Name addressable parts with
   `sub_regions()` (local rects, most specific first) and, where a driver or
   structural test must see a part as a zone, `sub_region_name()`. Reach the
   runtime only through `EventCx` (`request_focus`, `capture_pointer`,
   `set_cursor`, `request_redraw`, `emit`). Report assistive-technology
   actions with `a11y_actions()` and accept them as `RoutedEvent::Action`.
6. **Test state-derived paint geometry.** Whenever a painter computes a
   position from state — a scrollbar thumb from `(scroll_offset, content_rows,
   viewport_rows)`, a focus ring from focus, a badge offset from text length,
   an animation phase from a tick — write a test that paints at a known state
   and asserts the indicator lands where the formula predicts. Rule 4 catches
   paint and routing disagreeing; this catches the formula itself being wrong.
   Verify by mutating the formula and watching the test fail.
7. **Never a `Backend` method, never a per-backend file.** `Backend` is sealed
   and in-tree only, and holds platform services alone.
   `tests/backend_method_ratchet.rs` pins the trait to its exact list of
   platform methods and fails on any `*_layout` method or `draw_*` method
   other than the frame-level `draw_focus_ring`;
   `tests/per_backend_file_ratchet.rs` pins each backend directory to its
   exact list of platform modules, lets its line count only shrink, and pins
   `frame::Surface` to `Widget` + `Opaque`. A diff that adds a
   `draw_<name>`/`<name>_layout` method, a widget file under a backend
   directory, or a per-widget `Surface` variant fails those tests and is a
   review reject. Implement `Primitive` + `Interactive` instead; see
   "Writing a widget" below.
8. **Public API changes are versioned by deprecation, not by announcement.**
   See rule 8 in full below — it is the one rule whose failure lands in *other
   repos*.
9. **A new `PlatformServices` capability states its TUI story and ships a
   driver test proving it.** See rule 9 in full below.

## Paint state: hover, press and persistent state

A widget reads everything that is not layout from the `PaintEnv` its paint
receives:

- `env.theme()` and `env.style()` — what to paint with.
- `env.row_height` — this frame's resolved row pitch.
- **Hover and press** — `env.is_hovered(&id)`, `env.is_pressed(&id)`,
  `env.hovered_sub(&id)`, `env.pressed_sub(&id)`. The runtime tracks the
  pointer against what was painted, so a widget adds no `hovered`/`pressed`
  descriptor field and keeps no hover tracker. A composite that paints a child
  under its own id hands the child `env.child_interaction(&parent, &child)`.
- **Persistent state** — `env.widget_state_as::<MyState>()`: the widget's own
  `Interactive::State`, which the runtime keeps by id in `WidgetStateStore`
  between events. `None` until the widget has received an event; paint it as
  `State::default()`.
- `env.state` — the caller's per-call `PaintState`. Pass `&()` to paint with
  the live pointer state; pass an `InteractionState` to pin hover/press (a
  screenshot, a test).

**Controlled mode.** State the app may read or set — the selected row, whether
a dropdown is open, a scroll offset the app restores — is a descriptor field,
and the widget reports every change to it as an `Action`. The widget also keeps
that value in its own `Interactive::State` and paints from it while the
descriptor field is unchanged: a field left at its default, or resent
unchanged, leaves the widget uncontrolled, and a changed value moves it.
`behaviour::row_window::HostLatch` is the shared rule (`ListView`, `TreeView`,
`DataTable`, `TextDisplay`, `TabBar`, `CellGrid` use it; `AppMenu` keeps its
open menu the same way). Test such a widget both with a host that echoes its
events back and with one that never does. `Interactive::State` also holds what
the app never needs to see: a drag in progress, a press origin, a typeahead
buffer.

**Content loaded on demand.** A widget whose content is too large to send
whole carries a slice plus its place in the whole (`items_window:
Option<ItemsWindow>` on the four row widgets), paints placeholders for what it
does not have, and asks for more by returning an event from
`Interactive::data_requested` (`UiEvent::RowsRequested`, a `CellGrid`'s
`CellsRequested`). The runtime and binding-core ask it after every input the
widget handled; it is never asked at paint time. Use `behaviour::row_window`
for the band arithmetic and `RowsRequestLatch` so a range is asked for once.

## Popups

A widget that opens a dropdown, calendar or submenu sets
`const OPENS_POPUPS: bool = true` and answers `popups(&state, &layout)` with
one `quadraui::Popup` per open popup, anchored in widget-local coordinates.
Open is whatever that answer says: in controlled mode it reads a descriptor
field. The runtime paints popups above the whole frame, hit-tests them before
any widget, and delivers their events as `RoutedEvent::PopupEvent { popup,
event }`; an outside press or Escape arrives as `RoutedEvent::PopupDismissed`.
`ComboBox` and `DatePicker` are the references; `tests/widget_contract_popups.rs`
is the black-box test.

## Writing a widget

A complete widget, as a crate outside quadraui writes it (needs the `tui`
feature). Built-in widgets have the same shape with `crate::` paths:

```rust
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    A11yInfo, A11yPrimitive, AppLogic, Backend, CellSurface, CellSurfaceStyle, EventCx, FocusSpec,
    FrameTree, Handled, Interactive, Key, KeyEvent, LayoutCtx, MouseButton, NamedKey, PaintEnv,
    PaintSurface, PointerKind, Primitive, Reaction, Rect, RoutedEvent, ScreenLayout, Surface,
    UiEvent, WidgetAction, WidgetId,
};

/// A two-state chip. Controlled: the app owns `on` and flips it when the
/// chip reports `UiEvent::SwitchToggled`.
#[derive(Clone)]
pub struct Chip {
    pub id: WidgetId,
    pub label: String,
    pub on: bool,
}

/// Everything `paint`, `paint_cells` and `handle` need, resolved once.
pub struct ChipLayout {
    pub bounds: Rect,
}

pub enum ChipAction {
    Toggled(bool),
}

impl WidgetAction for ChipAction {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        match self {
            ChipAction::Toggled(value) => UiEvent::SwitchToggled { widget: id, value },
        }
    }
}

impl A11yPrimitive for Chip {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Chip {
    type Layout = ChipLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> ChipLayout {
        ChipLayout { bounds: rect }
    }

    // GTK, macOS, Win: one painter against the PaintSurface verbs.
    fn paint(&self, layout: &ChipLayout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        let theme = env.theme();
        let bg = if self.on {
            theme.selected_bg
        } else if env.is_hovered(&self.id) {
            theme.header_bg
        } else {
            theme.surface_bg
        };
        surface.surface_fill_rect(layout.bounds, bg);
        surface.surface_draw_text_run(layout.bounds, &self.label, theme.foreground);
    }

    // TUI: the cell-grid painter.
    fn paint_cells(&self, layout: &ChipLayout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        let theme = env.theme();
        let style = CellSurfaceStyle {
            fg: Some(theme.foreground),
            bg: Some(theme.surface_bg),
            reverse: env.is_hovered(&self.id),
            ..Default::default()
        };
        let b = layout.bounds;
        let mark = if self.on { "[x]" } else { "[ ]" };
        cells.fill(b, style);
        let text = format!("{mark} {}", self.label);
        cells.put_str_clipped(b.x as u16, b.y as u16, &text, b.width as u16, style);
    }
}

impl Interactive for Chip {
    type State = ();
    type Action = ChipAction;

    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        _layout: &ChipLayout,
        cx: &mut EventCx,
    ) -> Handled<ChipAction> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                Handled::Event(ChipAction::Toggled(!self.on))
            }
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter) | Key::Char(' '),
                ..
            }) => Handled::Event(ChipAction::Toggled(!self.on)),
            _ => Handled::Ignored,
        }
    }
}

// ── The host side: paint it, route to it, test it ──────────────────────

const AREA: Rect = Rect::new(0.0, 0.0, 12.0, 1.0);

struct App {
    chip: Chip,
}

impl AppLogic for App {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.chip.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.chip.id, AREA, Box::new(self.chip.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::SwitchToggled { widget, value } = event {
            if widget == self.chip.id {
                self.chip.on = value;
            }
        }
        Reaction::Continue
    }
}

#[test]
fn clicking_the_chip_toggles_it() {
    let chip = Chip {
        id: WidgetId::new("wrap"),
        label: "Wrap".into(),
        on: false,
    };
    let mut driver = TuiDriver::new(App { chip }, 20, 2);
    assert!(driver.screen_contains("[ ] Wrap"), "{}", driver.screen());

    let (x, y) = driver.find("Wrap").unwrap();
    driver.click(x, y);
    assert!(driver.screen_contains("[x] Wrap"), "{}", driver.screen());
}
```

`tests/primitive_public_api.rs` holds the same proof for a paint-only
`Primitive` drawn with `DrawsPrimitives::draw_primitive`, and
`tests/widget_contract_button.rs` is the shortest built-in example.

## How a host paints, routes and tests a widget

- **Paint.** Push `Surface::Widget { rect, widget: Box::new(w) }` onto a
  `ScreenLayout` and call `draw(backend)`, or call
  `backend.draw_dyn(rect, &w, &())` directly. Both resolve the layout with
  this backend's metrics, clip to `rect` (widened by `Primitive::paint_bleed`
  for a shadow or edge stroke), and call `paint` or `paint_cells`. A widget
  the platform presents itself (the macOS menu bar) answers
  `Primitive::native_presentation` and is not painted. `draw_primitive` paints
  a bare `Primitive` with no `Interactive` impl; it records nothing for
  routing.
- **Route.** Return the same widgets from `AppLogic::frame_tree` (or derive
  the tree with `ScreenLayout::into_frame_tree`). The runtime hit-tests the
  tree, moves focus, captures the pointer, and calls `handle`; the resulting
  `UiEvent` reaches `AppLogic::handle`. A click focuses a widget that has an
  `Interactive::focus` stop. Tab and Shift+Tab cycle only through the
  widgets whose ids and rects `AppLogic::extra_tab_stops` returns; while it
  returns nothing, Tab reaches `AppLogic::handle` unclaimed.
- **Observe.** Every identified widget painted through `draw_dyn` is recorded
  for the frame: `primitive::painted_hit(x, y)` answers which widget and
  sub-part is under a point, and `primitive::painted_sub_center` finds a
  sub-part to click. A widget's `paint_zone` and named `sub_region_name`
  parts appear as zones in `testing::FrameInventory`, which structural tests
  assert on.
- **Test.** `TuiDriver` runs on any host; `GtkDriver`, `MacDriver` and
  `WinDriver` drive the same `AppLogic` on their backends. See rule 4.

## Adding a widget — the short checklist

Rule 7 is the hard constraint (no `Backend` method, no backend file). This is
what ships in the PR:

1. **Descriptor + `Primitive` impl** in `primitives/<name>.rs` (or
   `compose/` for a composite): `id()`, `layout()`, `paint()`,
   `paint_cells()`, plus `measure()` if a container must size it to content
   and `paint_zone()` if structural tests must see it as a zone.
2. **`A11yPrimitive` impl** — report the widget through the same `a11y`
   machinery every built-in widget uses.
3. **`Interactive` impl**, or `non_interactive!(Name)` for a widget with no
   behaviour.
4. **No `Surface`/`FrameZone` variant.** The widget reaches the frame as
   `Surface::Widget`.
5. **Binding exposure**, if Python or Node apps build the widget. The
   descriptor derives `Serialize` + `Deserialize` and, under the `schema`
   feature, `schemars::JsonSchema`. Add a `widget_kinds!` entry in
   `quadraui/src/widget_kinds.rs` and the kind string to `WIDGET_KINDS` in
   `quadraui-binding-core/src/widget_kinds.rs`; the binding then decodes it
   as a generic widget leaf, paints and routes it through its own
   `Primitive`/`Interactive`, and keeps its `Interactive::State` across
   frames. Regenerate the typed wire files with
   `python3 scripts/gen_wire_types.py` and run `scripts/check_wire_types.sh`.
   No binding-side painter or controller is written. A widget defined
   outside quadraui registers its kind at runtime with
   `quadraui::register_widget_kind_with_schema` instead.
6. **Gallery entry** in `quadraui-gallery` showing the widget on every
   backend.
7. **Tests:** layout tests on the descriptor (rule 1), the black-box contract
   test (rule 4), and state-derived geometry tests (rule 6) where the widget
   has any.
8. **Changelog fragment** — `changelog.d/<issue>.added.md` per
   `changelog.d/README.md`.

## Rule 8 — public-API lifecycle

`quadraui` is published on crates.io; D-019 in `docs/decisions/DECISIONS.md`
states what a `0.x` release may change. Consumers and how each declares its
dependency are listed in `CLAUDE.md`'s *Downstream consumers* section, along
with the `downstream` CI job that compiles both against every PR.

**What is breaking here vs. what isn't.** Consumers implement `ShellApp`,
`AppLogic`, `Primitive` and `Interactive` — never `Backend`. So:

| Change | Breaks consumers? |
|---|---|
| New `Backend` method | **No** (in-tree only) — but never for a widget (rule 7). A platform capability only. |
| New required method on `ShellApp` / `AppLogic` / `Primitive` / `Interactive` | **Yes** — give it a default impl. |
| New field on a public struct a consumer constructs | **Yes** unless the struct is `#[non_exhaustive]` + built via `Default`/builder. |
| New variant on a public enum a consumer matches | **Yes** unless the enum is `#[non_exhaustive]`. |
| Rename / removal of any `pub` item with a consumer hit | **Yes.** |

**`Backend` is `pub` but sealed.** `Backend: sealed::Sealed` in
`src/backend.rs`, and `sealed` is a `pub(crate)` module, so `Backend` cannot be
implemented outside this crate — `Backend`'s rustdoc carries a `compile_fail`
doctest proving it. `BACKEND.md` describes contributing a new in-tree backend
as the supported path for a target the four do not cover. Every in-tree backend
and test double (`RecordingBackend`, `MockBackend`) implements `sealed::Sealed`
alongside `Backend`; do the same for a new one.

**Measure before you cut.** Both consumers sit beside this checkout:

```bash
grep -rn '<symbol>' ~/src/coord-tui/src ~/src/vimcode/src
```

Zero hits in both plus no in-tree use ⇒ remove it outright. **Paste the grep
output into the PR body.** Grep both: forgetting `vimcode` is the usual way a
"safe" removal breaks a consumer.

**Deprecation shims, in ascending cost.** All of these keep a consumer
compiling, with a warning that names the fix. (Shapes only — the `/* … */`
bodies are elided.)

```rust,ignore
// Rename a type — the whole cost of a non-breaking rename.
pub struct CardBadge { /* … */ }
#[deprecated(since = "0.0.2", note = "renamed to `CardBadge`")]
pub type Stage = CardBadge;

// Rename an enum variant — keep the old spelling as an associated const.
// (`allow` is required: the quality gate runs clippy with `-D warnings`,
// and a PascalCase const trips `non_upper_case_globals`.)
impl BadgeStatus {
    #[allow(non_upper_case_globals)]
    #[deprecated(since = "0.0.2", note = "renamed to `BadgeStatus::Warning`")]
    pub const RequestChanges: BadgeStatus = BadgeStatus::Warning;
}

// Change a shape — forward from the old one.
#[deprecated(since = "0.0.2", note = "use `CardBadge { label, status }`")]
impl From<(Stage, BadgeStatus)> for CardBadge { /* … */ }

// Drop a field consumers still set — accept and ignore, don't delete.
#[deprecated(since = "0.0.2", note = "no backend ever painted this; drop the call")]
pub fn with_machine(self, _machine: impl Into<String>) -> Self { self }
```

**The two-PR protocol.** A breaking change is never one PR:

- **PR 1** — add the new shape, keep the old one compiling behind
  `#[deprecated]`, update in-tree examples and tests to the new shape.
  Open the migration issue in each affected consumer repo and link it
  here. Consumers are now on a warning, not an error, and can migrate on
  their own schedule.
- **PR 2** — delete the shims, once those migrations have merged.
  Reference them by number.

**Both PRs get a `changelog.d/<issue>.<kind>.md` fragment** (repo root — see
`changelog.d/README.md`): PR 1 a `deprecated` fragment naming the old item, its
replacement and the PR-2 tracking issue; PR 2 a `removed` fragment with the same
wording. `scripts/release.py prepare` assembles them into `CHANGELOG.md` at
release time, so a reader can follow a deprecation from introduction to
removal without opening either PR.

**One breaking change per PR.** A consumer migrating a batch of unrelated
removals has no partially-compiling state to bisect from. Split them.

**Every PR touching a `pub` item carries a `## Downstream impact`
section** naming each consumer file that must move, or stating "no
consumer hits" with the grep. A public-API PR without one is sent back at
review.

## Rule 9 — `PlatformServices` honesty

A platform capability that silently does nothing on some backend is a bug the
app cannot detect. `tests/conformance/caps.rs` makes every capability state how
absence is reported. `Backend`'s defaulted methods are policed by
`CAP_CONTRACTS` / `backends_declare_only_what_they_override` (declare a
`BackendCaps` field and the matching override must exist, in both
directions). `PlatformServices` methods are policed by
`PLATFORM_SERVICE_CONTRACTS`, parsed from `src/backend.rs`; every method needs
a `ServiceHonesty`:

| Honesty | When to use it | Example |
|---|---|---|
| `CapBacked(cap)` | The method's return type is already ambiguous (`Option<T>` — `None` on cancel vs. `None` on no-facility-at-all) and a `BackendCaps` field disambiguates it. | `show_message_dialog` / `native_dialogs` |
| `ResultTwin(name)` | The method returns a bare `()` — nothing on the method itself could report absence. Add a `<name>_result` twin returning `ServiceResult<...>` (the same shape as `Clipboard::write_text`/`write_text_result`) and keep the `()` method for source compatibility. | `open_url` / `open_url_result` |
| `Delegates(trait)` | The method vends another trait object whose own methods carry their own honesty story. | `clipboard()` → `Clipboard` |
| `AlwaysReal` | The method can never silently do nothing — a plain identifier, or a fully typed, unambiguous return with no cap needed. | `platform_name`, a `_result` twin itself |

Two tests make this a gate:

- `platform_service_contracts_cover_every_method` — a new `PlatformServices`
  method with no `PLATFORM_SERVICE_CONTRACTS` entry fails the build.
- `every_bare_unit_service_method_has_a_typed_honesty_story` — a method that
  returns bare `()` **must** be `CapBacked` or `ResultTwin`.

**What this means when you add a capability.** Every new `PlatformServices`
method (or new capability *family*) needs, in the same PR:

1. **A `PLATFORM_SERVICE_CONTRACTS` entry** stating its honesty story —
   `ResultTwin` for a fire-and-forget action, `CapBacked` for something whose
   success or failure is otherwise ambiguous, or, for a whole new family (tray,
   window control, …), vend `Option<&mut dyn SubService>` so absence is
   structural and there is no cap flag to keep in sync.
2. **An explicit TUI story.** Most platform capabilities have no terminal
   equivalent, and that degrade must be a typed answer
   (`Err(BackendError::Unsupported)` from a `_result` method, or `None` gated
   by a `false` `BackendCaps` field), never a silent no-op.
3. **A `tui_*` driver test asserting the degrade** — build the example's
   `AppLogic`, drive it through `TuiDriver`, and assert the typed
   "unsupported" answer reaches the app. `examples/tui_platform_seams.rs` and
   its `platform_seams_demo_*_reports_unsupported_on_tui` tests in
   `tests/tui_example_driver.rs` are the template.

## Coordinate frames

The contract uses two frames, each fixed by the trait:

- **`Self::Layout` is absolute.** `layout()` receives the widget's rect in the
  surface's own coordinates and bakes that origin in, because `paint()` gets
  no rect of its own. A `Layout`'s `hit_test`, if it has one, takes absolute
  points.
- **Everything `Interactive` exchanges with the runtime is widget-local**
  (origin at the widget's rect): `RoutedEvent::Pointer`/`Wheel`'s `local`,
  `sub_regions()` rects, and `Popup` anchors. The runtime translates; a widget
  converts between the two only by adding or subtracting its rect's origin.

A cell-grid layout quantises its rect to whole cells before baking it in, so
hit-testing compares against the cells actually painted, not a fractional rect
a container handed down. Write a non-zero-origin test (`rect.x != 0` or
`rect.y != 0`) for any widget that converts between the frames: at the origin
a missing translation is invisible.

## One primitive, one canonical paint path

A widget has exactly one paint path: its own `paint`/`paint_cells`, reached
through `draw_dyn` (directly or via `Surface::Widget`). Do not add a second
route to the same pixels — a helper that paints a widget "the way it looks"
without going through its `Primitive` impl, or a backend-specific shortcut.
Two paths drift, and the compiler cannot see across them.

`ScreenLayout` + `Surface::Widget` is the path for a screen assembled from
several widgets: it paints them in z-order and `into_frame_tree` routes to
exactly what was painted. A direct `draw_dyn` call is right for a composite
painting a child it owns, or a test painting one widget.

## Layout math lives in `primitives/`

Everything that decides *what* to paint is shared code beside the descriptor:
geometry constants, `status → glyph` and `status → colour` tables, which theme
colour a hover/pressed/focused state resolves to, scroll-offset resolution
(including keeping a selection visible), and reservations that hit-testing and
paint must agree on (`needs_hscrollbar`-style). Pixel-layout helpers that need
no native drawing handle go in `quadraui/src/primitives/layout_metrics.rs`;
text widths come from `LayoutCtx::text_width` (or, in a shared helper that
takes no `LayoutCtx`, `layout_metrics::TextMeasure`). A painter only issues
`PaintSurface` or `CellSurface` calls against an already-resolved layout.

**A fence is a debt, and it needs an owner.** If a file-overlap conflict blocks
moving one piece of shared logic and you leave a duplicate in place:

- name the issue that **unblocks** it, not just the issue that tracks it;
- open a follow-up issue *at fence time* to remove the fence and link it in
  the comment — a comment is not a work item;
- state what the duplicate currently does differently, so the next reader can
  tell a deliberate difference from drift.

A fence whose blocker has closed is a bug waiting to be found.

## Native-vs-painted menu convention

Apps pick a menu's display path by trigger, not by backend:

| Trigger | Path | Why |
|---|---|---|
| `MouseDown { button: Right }` on a canvas / row / cell | `compose::ContextMenuController::open` | Resolves the menu style: on macOS it shows a native `NSMenu` (`Backend::show_context_menu`), getting the platform's fonts, accent colour, Dark Mode, VoiceOver and ⌘-shortcuts; elsewhere it paints a `ContextMenu` and routes it. Activation arrives as `UiEvent::ContextMenuItemActivated(WidgetId)`, dismissal as `UiEvent::ContextMenuDismissed`. |
| Left-click on an affordance that opens a dropdown (a combo box, a menu bar entry, a palette sub-menu) | The owning widget's `popups()` | App-controlled, pixel-consistent across backends, positioned relative to the widget rather than the cursor. The application menu bar is `compose::AppMenu`, which the macOS backend presents natively through `Primitive::native_presentation`. |

## When a widget is shipped

A widget is shipped when it has the descriptor with layout tests, `Primitive`
with both `paint` and `paint_cells`, `Interactive` (or `non_interactive!`),
and its `tests/widget_contract_<name>.rs`. A descriptor without painters is a
**descriptor** — its shape is real and tested, but apps cannot adopt it yet.
Don't delete one; give it painters when an app needs it.
