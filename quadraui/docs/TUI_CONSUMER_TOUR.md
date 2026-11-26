# How a TUI app consumes quadraui — a code tour

A reading guide for someone who wants to see how a quadraui widget gets
from a struct in app code to cells on a terminal and back to an event in
the app, before writing a widget or an app of their own. Every step
points at real code in this repository: one widget, one example app, and
the test that drives it.

The terminal is the easiest place to read the pattern, but nothing in it
is TUI-specific except the final paint call. On GTK, macOS and Windows
the same widget, the same app code and the same routing run unchanged;
only the surface the widget paints onto differs.

---

## The contract in one paragraph

A **widget** is one Rust type with three parts:

1. A **declarative struct** (`StatusBar { id, left_segments, … }`,
   `TabBar { id, tabs, … }`) that says *what* to show. The app rebuilds
   it from its own state every frame.
2. A **`Primitive` impl**: `layout(ctx, rect)` resolves every coordinate
   once into a `Layout`, and `paint(layout, surface, env)` (pixel
   backends) or `paint_cells(layout, cells, env)` (the terminal) draws
   from that `Layout` and nothing else.
3. An **`Interactive` impl**: which focus stops the widget has, which
   named sub-parts it has (`sub_regions`), how it reacts to each
   `RoutedEvent` (`handle`), what it remembers between events (`State`),
   and which popups it has open (`popups`).

Backends hold no widget code. The TUI backend hands a widget a
`CellSurface` (a cell grid with glyph, style, border and cursor verbs);
the pixel backends hand it a `PaintSurface` (fill, stroke, text and image
verbs). Paint and hit-testing both read the same `Layout`, so a click
cannot land somewhere other than where the widget painted.

The trait definitions, with their full contracts, are in
[`src/primitive.rs`](../src/primitive.rs) and
[`src/interactive.rs`](../src/interactive.rs).

---

## 1. One widget: `StatusBar`

[`src/primitives/status_bar.rs`](../src/primitives/status_bar.rs). Read
it in this order:

- **The struct.** `StatusBar` and `StatusBarSegment`. A segment with an
  `action_id` is clickable; one without is a label.
- **`impl Primitive for StatusBar`.** `layout` branches once on
  `ctx.units()`: on a cell grid each character is one cell, on a pixel
  surface segment widths come from `ctx.text_width`, which measures
  through the running backend's own font. Both branches return the same
  `StatusBarPrimitiveLayout`. `paint_cells` and `paint` then draw from
  it. Neither re-measures anything.
- **Hover and press tint.** The painters read hover and press from the
  `PaintEnv` (`env.hovered_sub(&self.id)`, `env.pressed_sub(&self.id)`),
  not from a field on the struct. The runtime fills those in from the
  pointer, so the app never threads hover state through its own model.
- **`impl Interactive for StatusBar`.**
  - `focus` returns `FocusSpec::None`: a status bar is reached by
    pointer only.
  - `sub_regions` names every visible segment, in bar-local coordinates,
    as a `SubTarget`. This is what lets the runtime say *which segment*
    the pointer is on, and what lets the painters above tint just that
    one.
  - `handle` is a small state machine over `StatusBarState`: a press arms
    a segment and captures the pointer, a release over the same segment
    returns `Handled::Event(StatusBarSegmentAction::SegmentClicked(id))`,
    and a release anywhere else activates nothing.
- **`impl WidgetAction for StatusBarSegmentAction`.** Turns the widget's
  own action into the `UiEvent` the app receives:
  `UiEvent::StatusBar(bar_id, StatusBarEvent::SegmentClicked { id })`.

That is the whole widget. No backend module (`src/tui/`, `src/gtk/`,
`src/macos/`, `src/win/`) has status-bar code of its own.

---

## 2. Painting it: `Surface::Widget` and `draw_dyn`

An app paints widgets by pushing them into a `ScreenLayout` and drawing
the frame. [`examples/hello.rs`](../examples/hello.rs) is the smallest
complete case: its `render_content` builds a `StatusBar` from
`self.keys_pressed`, pushes it as a `Surface::Widget` at the rect the
shell computed, and calls `frame.draw(backend)`.

`Surface::Widget` paints through `DrawsPrimitives::draw_dyn`
([`src/backend.rs`](../src/backend.rs)), which for each widget:

1. resolves `layout` with the backend's own metrics (row height, cell
   width, text measurement);
2. paints it, through `paint_cells` when the backend has a cell surface
   (the TUI backend) or through `paint` inside a clip when it has a
   pixel surface;
3. for a widget with an id, records its rect and sub-regions for this
   frame, publishes its `paint_zone` and its named sub-regions as
   frame-inventory zones, hands it its persistent `State` from the
   runtime's store, and queues any popups it has open.

An app that wants the resolved layout back (to hit-test something itself)
can call `backend.draw_dyn(rect, &widget, &())` directly; it returns the
layout it painted. The third argument is a `PaintState`: `&()` paints
with the live pointer state, and an explicit `InteractionState` pins
hover and press, which is how a test paints a hovered widget without a
mouse.

---

## 3. Routing input: `AppLogic::frame_tree`

Painting a widget does not make it clickable. An app opts a widget into
input routing by returning it from `AppLogic::frame_tree`.
[`examples/common/wide_tab_bar_demo.rs`](../examples/common/wide_tab_bar_demo.rs)
is the reference: it paints a `TabBar` and a `StatusBar` in `render`, and
returns the same `TabBar` from `frame_tree`:

```rust,ignore
// Excerpt from examples/common/wide_tab_bar_demo.rs, which builds with
// the rest of the examples; not compilable on its own.
fn frame_tree(&self, _area: ()) -> FrameTree {
    let mut tree = FrameTree::new();
    tree.push(Self::id(), self.strip.get(), Box::new(self.bar()));
    tree
}

fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
    match event {
        UiEvent::KeyPressed {
            key: quadraui::Key::Char('q'),
            ..
        } => Reaction::Exit,
        UiEvent::TabBar(_, TabBarEvent::TabActivated { index }) if index < 2 => {
            self.active = index;
            Reaction::Redraw
        }
        _ => Reaction::Continue,
    }
}
```

`frame_tree` gets no backend, so the demo stores the rect it painted the
bar at in a `Cell` during `render` and reads it back here, rather than
re-deriving a rect that could drift from the painted one.

The runner calls `frame_tree` once per incoming event, and the routing
engine (`route_widget_event` in [`src/runtime.rs`](../src/runtime.rs))
does the rest:

- **Mouse down:** hit-tests the tree, moves keyboard focus to the hit
  widget if it has a focus stop (click to focus), and delivers a
  `RoutedEvent::Pointer` in widget-local coordinates, with the
  `SubTarget` under the pointer.
- **Mouse move:** delivers `Enter`/`Leave` to whichever widgets the
  pointer entered or left, or every move to the widget holding a pointer
  capture.
- **Mouse up:** goes to the widget holding the capture.
- **Keys:** go to the focused widget.
- **Open modal:** while one is open, nothing is routed to the frame's
  widgets.

Each widget's `State` lives in the runtime's `WidgetStateStore`, keyed by
`WidgetId`, so it survives the descriptor being rebuilt every frame. A
widget whose id drops out of the tree loses its state; if it comes back
it starts from `State::default()`. Whatever `handle` returns as
`Handled::Event` reaches the app's own `handle` as the `UiEvent` its
`WidgetAction` produces. The app applies it to its model, and the next
frame's descriptor shows the result.

Tab and Shift+Tab cycle a `frame_tree` widget's focus stops only once the
widget's id also appears in `AppLogic::extra_tab_stops`; see that
method's documentation in [`src/runner.rs`](../src/runner.rs).

---

## 4. Popups

A widget that opens something above the rest of the frame (a dropdown, a
calendar) sets `Interactive::OPENS_POPUPS` and answers
`Interactive::popups` from what it already knows, usually a descriptor
field the app sets, such as `ComboBox::open`. The runtime keeps no popup
state of its own:

- `draw_dyn` queues each open popup, and the runner paints the queue
  above the whole frame;
- an event over a popup reaches its owner as `RoutedEvent::PopupEvent`,
  resolved against the popup's own content;
- a press outside the popup and its owner, or Escape, reaches the owner
  as `RoutedEvent::PopupDismissed`, and the owner emits the event that
  makes the app clear the field.

Read [`src/popup.rs`](../src/popup.rs)'s module documentation, then
`ComboBox`'s `popups` and `handle` in
[`src/primitives/combo_box.rs`](../src/primitives/combo_box.rs).

---

## 5. Testing it

`TuiDriver` ([`src/tui/testing.rs`](../src/tui/testing.rs)) runs a real
`AppLogic` through the same `event → routing → handle → render` path the
terminal runner uses, against an in-memory terminal.
[`tests/tui_example_driver.rs`](../tests/tui_example_driver.rs) drives
the demo above; search it for `wide_tab_bar_demo_`:

- `wide_tab_bar_demo_clicking_a_tab_activates_it_through_the_routing_engine`
  finds a tab's label on screen with `driver.find`, clicks it, and asserts
  the status readout changed. No coordinate is hardcoded.
- `wide_tab_bar_demo_right_then_enter_activates_the_next_tab` clicks to
  focus, then presses Right and Enter. That exercises focus routing and
  `TabBar`'s keyboard handling through the real engine.

When a label alone cannot identify a target (every tab paints the same
close glyph), `TuiDriver::tab_center` and `tab_close_center` look the
part up by widget id in the frame's record of painted widgets.
[`tests/widget_contract_combo_box.rs`](../tests/widget_contract_combo_box.rs)
is the same kind of test for a widget with a popup.

A widget's `handle` can also be unit-tested on its own with a small
`EventHost` stand-in that records focus requests, redraws and emitted
events; see the `MockHost` in the tests at the bottom of
[`src/interactive.rs`](../src/interactive.rs). [`TESTING.md`](TESTING.md)
covers the other tiers.

---

## 6. Writing your own widget

A widget defined outside this crate works exactly like `StatusBar`:
implement `Primitive` and `Interactive` (or `non_interactive!` for one
with no behaviour), push it as a `Surface::Widget`, and return it from
`frame_tree` if it takes input. It needs no change to any backend. The
smallest complete example is the `Swatch` in the root `README.md`
("Your own widget"); `src/primitive.rs`'s module documentation paints
the same kind of widget onto a stand-in surface. The rules every widget
follows, including the public-API lifecycle, are in
[`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md).

---

## Suggested reading order

About an hour:

1. [`examples/hello.rs`](../examples/hello.rs): a widget painted from app
   state.
2. [`src/primitives/status_bar.rs`](../src/primitives/status_bar.rs):
   the `Primitive`, `Interactive` and `WidgetAction` impls (§1).
3. `draw_dyn` in [`src/backend.rs`](../src/backend.rs) (§2).
4. [`examples/common/wide_tab_bar_demo.rs`](../examples/common/wide_tab_bar_demo.rs)
   and its tests in
   [`tests/tui_example_driver.rs`](../tests/tui_example_driver.rs)
   (§3, §5).
5. [`src/popup.rs`](../src/popup.rs) and
   [`src/primitives/combo_box.rs`](../src/primitives/combo_box.rs) (§4).
6. [`decisions/DECISIONS.md`](decisions/DECISIONS.md) for why the
   contract is shaped this way.
