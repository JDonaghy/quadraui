# Widgets

A widget is **plain data**: a struct (Rust) or a node (Python, Node) that
says what to show. You build a fresh one every frame from your state; it
does not hold a pointer back into your app.

- **Python:** `Button("+", id="inc", on_click=self.inc)`
- **Node:** `<button id="inc" label="+" onClick={...} />`
- **Rust:** `button("+").id("inc").on_click(Msg::Inc)` in a `View`, or the
  descriptor itself, `Button::new("inc".into(), "+")`, below the app layer

Every widget has a **`WidgetId`**, a string you choose (`"inc"`). Events
name the widget they came from by that id, and the runtime keeps a widget's
private state under it between frames. Keep ids stable. In all three
languages an app-layer id is optional; a node without one gets an id from
its position in the tree.

quadraui ships more than 50 widgets: lists, trees, tables, forms, an editor, a
terminal, charts, tabs, menus, dialogs and more. See them all, on every
backend, in the [Widget gallery](../gallery/index.md).

## If you know...

- **React:** a node is a host element (`<button>`), not a component. Your
  own reusable pieces are `Component` subclasses (Python, Node) that bundle
  a view with local state.
- **iced / egui:** a widget is a value you return from `view`, like
  iced's `Element`. Unlike egui, it does not draw immediately: the runtime
  lays out, paints and hit-tests the whole tree.
- **Textual:** there is no widget object living across frames. What
  Textual keeps on the widget instance (scroll position, cursor), quadraui
  keeps in its own per-id store; see [State](state.md).

## Writing your own (Rust)

A widget is one type implementing two traits:

- **`Primitive`** lays out once into a `Layout` value, then paints from it:
  `paint` for the pixel backends, `paint_cells` for the terminal.
- **`Interactive`** handles input against that same `Layout`: focus stops,
  clicks, keys, popups. Paint and hit-testing cannot disagree, because both
  read one layout.

This one paints a block of colour and has no behaviour (from the README;
CI compiles and paints it):

<!-- sample: quadraui/tests/readme_own_widget.rs -->
```rust,noplayground
impl Primitive for Swatch {
    // The resolved geometry that paint, and any hit-test, read.
    type Layout = Rect;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    // GTK, macOS and Windows.
    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, _env: &PaintEnv) {
        surface.surface_fill_rect(*layout, self.color);
    }

    // The terminal (needs the `tui` feature).
    fn paint_cells(&self, layout: &Rect, cells: &mut dyn CellSurface, _env: &PaintEnv) {
        let style = CellSurfaceStyle {
            bg: Some(self.color),
            ..Default::default()
        };
        cells.fill(*layout, style);
    }
}

quadraui::non_interactive!(Swatch);
```

Backends contain no widget code, so a widget written this way works on all
four. The rules every widget follows are in
[`PRIMITIVE_RULES.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/PRIMITIVE_RULES.md).

Python has the same idea in `quadraui.app.Widget` (override `measure`,
`paint`, `on_event`); `quadraui-py/examples/rating.py` is a worked
example. Node has `defineWidget`.
