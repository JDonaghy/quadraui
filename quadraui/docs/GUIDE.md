# Guide: your first quadraui app

This walks through building a small two-pane app — a file list on the
left, a text pane on the right — from an empty `cargo new` to something
you can run and click on. If you just want to see *something* on screen
first, run the one-file example before reading further:

```sh
cargo run --example hello --features tui
```

That's `quadraui/examples/hello.rs` — under 60 lines, no shared helper
module, nothing to understand beyond this guide's Step 1–3. Everything
below builds on the same shape.

Every code sample here opens with `use quadraui::prelude::*;`. The
prelude is a deliberately small subset of quadraui's ~500 crate-root
exports — the two runner traits, the event/geometry types, and a
handful of primitives (`Panel`, `Split`, `StatusBar`, `TextDisplay`,
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
| VS-Code-style chrome for free — activity bar, sidebar, title bar, status bar, bottom panel — and you fill in the *content* of each panel | [`ShellApp`] | `quadraui::tui::run_with_shell(app, config)` / the GTK equivalent | A window manager you configure, not one you build. |

**This guide uses `AppLogic`.** Our two-pane app is simple enough that
building the split ourselves is less code than configuring an
`AppShell`. Reach for `ShellApp` when you find yourself manually
reimplementing an activity bar, a collapsible sidebar, or a bottom
panel with tabs — `AppShell` (what `ShellApp` runs inside) already has
all of that, and `quadraui/docs/CONSUMER_PATTERNS.md` has recipes for
wiring your own panels into it.

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
quadraui = { path = "/path/to/quadraui/quadraui", features = ["tui"] }
```

(Once quadraui is on crates.io this becomes a version requirement
instead of a path — see the crate root `README.md` for the current
publish status.)

## Step 2: app state

```rust,ignore
use quadraui::prelude::*;

struct TwoPane {
    split: Split,
    files: Vec<&'static str>,
    selected: usize,
    body: String,
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
            style: Default::default(),
            has_focus: true,
        }
    }
}
```

`Split` only describes geometry — a ratio and two minimum sizes — it
doesn't hold either pane's content. That's the pattern every quadraui
container primitive follows: the primitive is the frame, your app draws
into the rectangles it hands back.

## Step 3: rendering

```rust,ignore
impl AppLogic for TwoPane {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let bounds = Rect::new(0.0, 0.0, viewport.width, viewport.height);

        // Split gives us two rectangles; we're responsible for what
        // goes in each one.
        let layout = backend.draw_split(bounds, &self.split);

        backend.draw_tree(layout.first_bounds, &self.tree_view());

        let body = TextDisplay {
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
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        backend.draw_text_display(layout.second_bounds, &body);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let bounds = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let split_layout = backend.split_layout(bounds, &self.split);

                match split_layout.hit_test(position.x, position.y) {
                    quadraui::SplitHit::FirstPane(_) => {
                        // Re-derive which tree row was clicked from the
                        // same layout the paint pass used — never
                        // hardcode a row height here. `tree_layout`'s
                        // row bounds are relative to `first_bounds`'s
                        // own top-left corner, so add it back to compare
                        // against `position`, which is in screen space.
                        let tree = self.tree_view();
                        let tree_layout =
                            backend.tree_layout(split_layout.first_bounds, &tree);
                        let origin_y = split_layout.first_bounds.y;
                        let hit_row = tree_layout.visible_rows.iter().find(|row| {
                            let top = origin_y + row.bounds.y;
                            position.y >= top && position.y < top + row.bounds.height
                        });
                        if let Some(row) = hit_row {
                            self.selected = row.row_idx;
                            self.body = format!("You picked: {}", self.files[self.selected]);
                            return Reaction::Redraw;
                        }
                        Reaction::Continue
                    }
                    _ => Reaction::Continue,
                }
            }
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Named(NamedKey::Escape) | Key::Char('q') => Reaction::Exit,
                Key::Named(NamedKey::Down) if self.selected + 1 < self.files.len() => {
                    self.selected += 1;
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Up) if self.selected > 0 => {
                    self.selected -= 1;
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
```

Two things worth calling out:

- **Click routing goes through the same layout paint used**
  (`backend.split_layout` / `backend.tree_layout`), never a hand-computed
  coordinate. This is the whole point of the primitive/layout split —
  paint and click can never drift apart because they're the same
  computation.
- **`render` takes `&self`, `handle` takes `&mut self`.** State changes
  happen in `handle`; `render` only reads.

## Step 4: running it

```rust,ignore
fn main() -> std::io::Result<()> {
    quadraui::tui::run(TwoPane::new())
}
```

`cargo run --features tui` and you should see a two-pane terminal app:
a file list on the left, a message pane on the right. Click a filename
or use the arrow keys; `q` or Esc quits.

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

- **More primitives**: `quadraui::primitives` has 40 of them; the crate
  root doc (`quadraui/src/lib.rs`) has a representative table, and every
  module has its own doc comment describing its backend contract.
- **`AppShell` / `ShellApp`**: if your app grows an activity bar, a
  resizable sidebar, or tabs, revisit Step 0 — `quadraui/docs/
  CONSUMER_PATTERNS.md` has recipes for wiring panels into `AppShell`.
- **A second backend**: everything above is TUI-specific only in
  `main()`. Swap `quadraui::tui::run` for `quadraui::gtk::run` (behind
  the `gtk` feature) and the exact same `AppLogic` impl draws with
  Cairo/Pango instead of ratatui — that's the cross-backend portability
  commitment `CLAUDE.md` describes.
- **Testing your own `Backend`-consuming code without a real backend**:
  [`quadraui::testing::RecordingBackend`] is a fully-implemented
  `Backend` for unit tests that need *some* backend to hand a function,
  not a specific one's real rendering — see its doc for what it records
  and what it deliberately leaves `unimplemented!()`.

[`AppLogic`]: ../src/runner.rs
[`ShellApp`]: ../src/shell.rs
[`quadraui::tui::testing::TuiDriver`]: ../src/tui/testing.rs
[`quadraui::testing::RecordingBackend`]: ../src/testing.rs
