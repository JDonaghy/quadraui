# Guide: your first quadraui app

This walks through building a small two-pane app — a file list on the
left, a text pane on the right — from an empty `cargo new` to something
you can run and click on. If you just want to see *something* on screen
first, run the one-file example before reading further:

```sh
cargo run --example hello --features tui
```

That's `quadraui/examples/hello.rs` — under 100 lines including its doc
comments, no shared helper module. It's a `ShellApp` (see Step 0 below)
rather than the `AppLogic` this guide's Step 1–3 walks through, and it
paints with `ScreenLayout`/`Surface` — pushing declarative
`Surface::Widget` entries and calling `ScreenLayout::draw`. That is the
canonical way to paint a screen: one call site paints every widget, and
the same widgets returned from `AppLogic::frame_tree` receive their
input. Step 3 below also paints some widgets by hand at a `Rect` it
computes itself. See "How a host paints, routes and tests a widget" in
[`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md) for the whole host side.

The Rust blocks in Steps 2 to 5 are consecutive pieces of one `src/main.rs` (Step
5's test goes in a `#[cfg(test)]` module); put together in order they build and
the test passes against this release. Each is marked `ignore` for rustdoc because
no single piece compiles on its own.

Every code sample here opens with `use quadraui::prelude::*;`. The
prelude is a deliberately small subset of quadraui's ~500 crate-root
exports — the two runner traits, the event/geometry types, and a
handful of primitives (`Split`, `StatusBar`, `TextDisplay`,
`TextInput`, `TreeView`) representative enough to build this app. When
you reach for a primitive the prelude doesn't carry, import it from
`quadraui::` directly — nothing is hidden, the prelude just narrows the
front door.

## Step 0: which runner do I want?

quadraui has two ways to plug your app into a backend, and picking the
wrong one wastes the most time of anything in this guide. Decide before
you write any code:

| You want | Implement | Call | Feels like |
|---|---|---|---|
| Full control over every pixel/cell — you lay out everything yourself | [`AppLogic`] | `quadraui::tui::run(app)` / `quadraui::gtk::run(app)` | A blank canvas: one `render`, one `handle`, you own all of it. |
| VS-Code-style chrome for free — activity bar, sidebar, title bar, status bar, bottom panel — and you fill in the *content* of each panel | [`ShellApp`] | `quadraui::tui::shell_runner::run_with_shell(app, config)` / the GTK equivalent | A window manager you configure, not one you build. |

**This guide uses `AppLogic`.** Our two-pane app is simple enough that
building the split ourselves is less code than configuring a
`ShellView`. Reach for `ShellApp` when you find yourself manually
reimplementing an activity bar, a collapsible sidebar, or a bottom
panel with tabs — `ShellView` (what `ShellApp` runs inside) already has
all of that, and the `ShellView` section of [`COMPOSE.md`](COMPOSE.md)
shows how to fill its panels.

Both traits' methods take a `&mut dyn Backend` — the same trait object
either way, so switching from `AppLogic` to `ShellApp` later doesn't
mean relearning the primitive/paint API, only how your `render`/`handle`
plug into the runner.

## Step 1: project setup

```sh
cargo new two-pane && cd two-pane
```

```toml
# Cargo.toml
[dependencies]
quadraui = { version = "0.11", features = ["tui"] }
```

[`INSTALL.md`](INSTALL.md) lists every backend feature and the system
packages each one needs (GTK4 development libraries for `gtk`, for
example). For a sibling checkout use
`quadraui = { path = "/path/to/quadraui/quadraui", features = ["tui"] }`
instead.

## Step 2: app state

```rust,ignore
use std::cell::Cell;

use quadraui::prelude::*;
use quadraui::{DrawsPrimitives, FrameTree, TreeEvent}; // draw_primitive, draw_dyn

struct TwoPane {
    split: Split,
    files: Vec<&'static str>,
    selected: usize,
    body: String,
    /// Where `render` last put the file tree, so `frame_tree` can route
    /// input to the same rectangle.
    tree_rect: Cell<Rect>,
}

impl TwoPane {
    fn new() -> Self {
        Self {
            split: Split {
                id: WidgetId::new("main:split"),
                direction: SplitDirection::Horizontal,
                ratio: 0.3,
                first_min: 10.0,
                second_min: 20.0,
            },
            files: vec!["README.md", "src/main.rs", "Cargo.toml"],
            selected: 0,
            body: "Select a file on the left.".into(),
            tree_rect: Cell::new(Rect::default()),
        }
    }

    fn tree_view(&self) -> TreeView {
        TreeView {
            id: WidgetId::new("files:tree"),
            rows: self
                .files
                .iter()
                .enumerate()
                .map(|(i, name)| TreeRow {
                    path: vec![i as u16],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain(*name),
                    badge: None,
                    is_expanded: None, // every row is a leaf
                    decoration: Default::default(),
                    edit: None,
                })
                .collect(),
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: Some(vec![self.selected as u16]),
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            style: Default::default(),
            has_focus: true,
        }
    }

    fn body_view(&self) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("body:text"),
            lines: self
                .body
                .lines()
                .map(|l| TextDisplayLine {
                    spans: vec![StyledSpan::plain(l)],
                    decoration: Default::default(),
                    timestamp: None,
                })
                .collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        }
    }
}
```

The app owns its state; `tree_view` and `body_view` build fresh widget
descriptions from it every frame. A widget is plain data: the `TreeView`
holds the rows and which one is selected, and the app changes those
fields when the tree reports a click.

`Split` only describes geometry, a ratio and two minimum sizes; it holds
neither pane's content. The app decides what goes in each rectangle the
split hands back.

## Step 3: rendering and input

```rust,ignore
impl AppLogic for TwoPane {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let bounds = Rect::new(0.0, 0.0, viewport.width, viewport.height);

        // Painting the split draws its divider and returns its layout:
        // two pane rectangles, and we decide what goes in each one.
        let panes = backend.draw_primitive(bounds, &self.split);
        self.tree_rect.set(panes.first_bounds);

        backend.draw_dyn(panes.first_bounds, &self.tree_view(), &());
        backend.draw_dyn(panes.second_bounds, &self.body_view(), &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        // The widgets that take input this frame, at the rects they were
        // painted at. The runtime hit-tests these, gives the clicked one
        // keyboard focus and calls its `handle`.
        let mut tree = FrameTree::new();
        let files = self.tree_view();
        tree.push(files.id, self.tree_rect.get(), Box::new(files));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // The tree reports what happened to it as a `TreeEvent`: a
            // click on a row, or the arrow keys moving the selection once
            // the tree has focus.
            UiEvent::Tree(_, TreeEvent::RowClicked { path, .. })
            | UiEvent::Tree(_, TreeEvent::SelectionChanged { path }) => {
                self.selected = usize::from(path[0]);
                self.body = format!("You picked: {}", self.files[self.selected]);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
```

Three things worth calling out:

- **The app never hit-tests the tree.** `TreeView` implements
  `Interactive`: the runtime finds which row is under the pointer from the
  same layout the tree painted with, so a click cannot land on a row other
  than the one on screen. The app only applies the `TreeEvent` it gets
  back.
- **The tree takes focus when clicked.** After that the arrow keys, Home,
  End and typeahead move its selection, and each move arrives as
  `TreeEvent::SelectionChanged`. Keys the tree does not use, such as
  Escape, still reach `handle` as `UiEvent::KeyPressed`. To make the tree
  reachable with Tab, return its id and rect from
  `AppLogic::extra_tab_stops`.
- **`render` takes `&self`, `handle` takes `&mut self`.** State changes
  happen in `handle`; `render` only reads, which is why the tree's rect is
  kept in a `Cell`.

## Step 4: running it

```rust,ignore
fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(TwoPane::new())
}
```

`cargo run --features tui` and you should see a two-pane terminal app:
a file list on the left, a message pane on the right. Click a filename,
then use the arrow keys; Esc quits.

## Step 5: testing it

Every shipping `tui_*` example in this repo has a matching
[`quadraui::tui::testing::TuiDriver`] end-to-end test — the same pattern
applies to your app:

```rust,ignore
use quadraui::tui::testing::TuiDriver;

#[test]
fn clicking_a_file_updates_the_body() {
    let mut driver = TuiDriver::new(TwoPane::new(), 80, 24);
    assert!(driver.screen_contains("README.md"));

    let (x, y) = driver.find("src/main.rs").unwrap();
    driver.click(x, y);

    assert!(driver.screen_contains("You picked: src/main.rs"));
}
```

`TuiDriver` runs your real `AppLogic` against a headless terminal buffer
— no PTY, no visible window — and `find`/`screen_contains` locate
painted text instead of hardcoding cell coordinates, so the test
survives you tweaking the layout later. See
[`quadraui/docs/TESTING.md`](TESTING.md) for the rest of the coverage
taxonomy (primitive-level paint↔click round-trips, cross-backend
conformance) once your app grows past one screen.

## Where to go next

- **More widgets**: the root README's *What's in the box* introduces the
  widgets in `quadraui::primitives`, and each module's doc comment
  describes the widget, its layout and the events it emits.
  [`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md) covers writing your own
  widget and how a host paints, routes and tests one.
- **`ShellView` / `ShellApp`**: if your app grows an activity bar, a
  resizable sidebar, or tabs, revisit Step 0. [`COMPOSE.md`](COMPOSE.md)
  documents `ShellView` and the other compose-layer widgets that
  coordinate several widgets.
- **A second backend**: everything above is TUI-specific only in
  `main()`. Swap `quadraui::tui::run` for `quadraui::gtk::run` (behind
  the `gtk` feature) and the exact same `AppLogic` impl draws with
  Cairo/Pango instead of ratatui — that's the cross-backend portability
  commitment `CLAUDE.md` describes.
- **Testing your own `Backend`-consuming code without a real backend**:
  [`quadraui::testing::RecordingBackend`] is a headless `Backend` for
  unit tests that need *some* backend to hand a function, not a specific
  one's real rendering — see its doc for what it records and which
  platform services it leaves `unimplemented!()`.

[`AppLogic`]: ../src/runner.rs
[`ShellApp`]: ../src/shell.rs
[`quadraui::tui::testing::TuiDriver`]: ../src/tui/testing.rs
[`quadraui::testing::RecordingBackend`]: ../src/testing/mod.rs
