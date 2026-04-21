# quadraui

Cross-platform UI primitives for keyboard-driven desktop and terminal apps.

One declarative API renders to four backends: Windows (Direct2D), Linux
(GTK4 + Cairo), macOS (Core Graphics, planned), and TUI (ratatui).

## What problem this solves

You're writing a keyboard-driven app — an editor, a database client, a
k8s dashboard, a Postman replacement. You want it to:

- Run as a real GUI on every desktop OS.
- Run in a terminal over SSH for the same UI.
- Eventually accept Lua plugins that declare their own panels.

If you write the UI directly against GTK / WinUI / SwiftUI / ratatui,
you write it three or four times and the plugin story is hopeless.

quadraui is the in-between layer: a small set of declarative primitives
(tabs, trees, lists, forms, status bars, palettes, terminals, …) that
your app constructs from its state, and that backends rasterise. Apps
swap backends without rewriting; plugins describe UI as data; one
primitive update propagates to every backend.

It is not a layout library, not a widget toolkit, not a retained-mode
DOM. It is the smallest declarative-data shape that lets one app target
four rendering models without forking.

## Quick start

Add to `Cargo.toml`:

```toml
[dependencies]
quadraui = { path = "path/to/quadraui" }   # or git/version once published
```

Build a primitive from your app state, hand it to a backend draw
function. Example using the TUI backend pattern:

```rust
use quadraui::{StatusBar, StatusBarSegment, Color, WidgetId};

let bar = StatusBar {
    id: WidgetId::new("status:editor"),
    left_segments: vec![
        StatusBarSegment {
            text: " NORMAL ".to_string(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(30, 30, 30),
            bold: true,
            action_id: None,
        },
        StatusBarSegment {
            text: " main.rs".to_string(),
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(30, 30, 30),
            bold: false,
            action_id: None,
        },
    ],
    right_segments: vec![/* cursor pos, LSP, etc. */],
};

// Your backend's draw function consumes &bar and rasterises it.
my_backend::draw_status_bar(target, &bar, &theme);
```

When the user clicks, the backend resolves the click column to a
segment via `bar.resolve_click_fit_chars(col, width, gap)` and emits a
`StatusBarEvent::SegmentClicked { id }`. Your app maps the `WidgetId`
back to its own action dispatch — no closures cross the primitive
boundary.

## Primitives

| Primitive | Use for |
|-----------|---------|
| `TreeView` | File explorers, source-control panels, hierarchical lists |
| `ListView` | Quickfix, search results, flat selectable lists |
| `Form` | Settings panels, config editors |
| `Palette` | Command palettes, fuzzy pickers |
| `StatusBar` | Mode/file/cursor strips, footer bars |
| `TabBar` | Editor tabs, document tabs |
| `ActivityBar` | Vertical icon strips (VSCode-style) |
| `Terminal` | Cell grids for terminal emulators |
| `TextDisplay` | Streaming logs, AI chat output |

Each primitive has its own module under `src/primitives/` with
rustdoc covering: declarative shape, event variants, and the
**backend contract** — what backends MUST do every frame to make the
primitive function correctly. Read the contract for a primitive
*before* implementing it in a new backend, otherwise the primitive
may render but interactions will silently break.

## How it fits together

```
                           ┌──────────────────────┐
                           │  Your app's state    │
                           │  (engine / model)    │
                           └──────────┬───────────┘
                                      │ build primitive
                                      ▼
   ┌──────────────────────────────────────────────────────────────┐
   │             quadraui primitives (declarative)                │
   │   StatusBar { left_segments, right_segments, ... }           │
   │   TabBar { tabs, scroll_offset, right_segments, ... }        │
   │   TreeView { rows, selection, ... }                          │
   └──┬─────────────┬───────────────┬──────────────────┬──────────┘
      │             │               │                  │
      ▼             ▼               ▼                  ▼
   ┌──────┐    ┌─────────┐    ┌─────────┐      ┌────────────┐
   │ TUI  │    │   GTK   │    │ Win-GUI │      │   macOS    │
   │ratatui│    │D2D/Cairo│    │ Direct2D│      │CoreGraphics│
   └──────┘    └─────────┘    └─────────┘      └────────────┘

      ▲             ▲               ▲                  ▲
      │             │               │                  │
      └─────────────┴────────────┬──┴──────────────────┘
                                 │ events (data, not closures)
                                 ▼
                           ┌──────────────────────┐
                           │  Your app's state    │
                           └──────────────────────┘
```

The data flow is one-way per frame: state → primitive → pixels →
events → state. No retained widget tree, no two-way binding, no
cross-backend coordination. Each backend is independent.

## Cross-backend invariants

Three patterns make multi-backend primitives work. **All three are
contracts; violating them breaks at least one backend silently:**

### 1. Owned data only — no `&'static str`, no closures

`WidgetId` is `String`. Events are plain enums with `WidgetId`
references — never `Box<dyn Fn(...)>`. Primitives are
`Serialize + Deserialize` so Lua plugins can describe UI as JSON.

This is plugin invariant #1, #2, #3 from the design doc — verify
when adding/extending primitives.

### 2. Measurer-parameterised algorithms

Any "fit X within Y / where does Z scroll to / which slice fits in N
units" algorithm must be **generic over a measurement closure**. The
TUI uses cell counts; GTK uses Pango pixel widths; Win-GUI uses
DirectWrite; macOS uses Core Text. Hardcoding a unit silently breaks
every backend that doesn't share it.

Two examples shipped:

- `StatusBar::fit_right_start<F>(width, gap, measure)` — drops
  low-priority right segments when the bar is narrow.
- `TabBar::fit_active_scroll_offset<F>(active, count, width, measure)`
  — finds the scroll offset that keeps the active tab visible.

When adding a new primitive with similar logic, follow the same
pattern. *Do not* put unit-dependent geometry in the engine/app side
of the API.

### 3. Backend contract per primitive

Some primitives are purely declarative (give the backend the data,
it paints). Others have measurement-dependent state that backends
MUST update each frame. The contract for each primitive is in its
rustdoc under "Backend contract"; the two non-trivial ones today are:

- `StatusBar` — backend must compute `fit_right_start` per frame and
  render only the visible slice; click handlers must skip dropped
  segments (otherwise narrow bars trigger actions on invisible items).
- `TabBar` — backend must measure each tab in its native unit, call
  `fit_active_scroll_offset`, write the result back to wherever the
  app stores it, and repaint if it changed (otherwise the active tab
  lands off-screen after layout changes).

If you skip the contract for a primitive, it'll *render* but
interactions will be subtly broken in the same ways every other
backend was before the contract was discovered. Read the rustdoc.

## Implementing a new backend

See **[`BACKEND.md`](BACKEND.md)** for the full guide: mental model,
the three contracts every backend must honour, the two-pass paint
pattern for backends without mid-draw mutability, click-intercept
hierarchy, and a checklist to run through when standing up a new
backend.

The short version:

1. Pick your render API (GPU, GUI toolkit, terminal, etc.).
2. For each primitive your app uses, write a `draw_<primitive>(target,
   primitive, theme)` function that renders the declarative struct.
3. For primitives with backend contracts (`StatusBar`, `TabBar`),
   wire the contract into your paint cycle. Read each primitive's
   "Backend contract" rustdoc section first.
4. Translate clicks/keys/mouse-events into `*Event` enums for your app.

Two runnable examples ship with the crate — same demo app rendered
two ways:

```bash
cargo run --example tui_demo
cargo run --example gtk_demo --features gtk-example   # needs libgtk-4-dev
```

Both exercise the `TabBar` contract (measure → fit → correct →
repaint) and the `StatusBar` contract (fit → render-slice →
click-skip). The TUI version is the easy case (cell units = engine
defaults); the GTK version shows the **pixel-unit measurer pattern**
that every non-TUI backend needs. The app code (state struct,
primitive builders, event handlers) is identical between the two —
only the `draw_*` internals differ. Read both alongside `BACKEND.md`
for the patterns in working form.

Production backends live in the [vimcode] repository:

- `src/tui_main/quadraui_tui.rs` — TUI (ratatui)
- `src/gtk/quadraui_gtk.rs` — GTK4 (Cairo + Pango)
- `src/win_gui/quadraui_win.rs` — Windows (Direct2D + DirectWrite)

vimcode itself is the largest known consumer of quadraui (~16,000
lines of editor logic, 5000+ tests). Both `*_to_*` adapter naming
and the per-primitive `draw_*` shape are stable patterns to copy.

[vimcode]: https://github.com/JDonaghy/vimcode

## Status

Pre-1.0 (`v0.1.x`). All nine primitives shipped. Workspace member
of the vimcode repository; not yet published to crates.io. API will
stabilise before publishing.

Battle-testing per backend:

- **TUI**: full feature parity, used as vimcode's `vcd` binary.
- **GTK4**: full feature parity, vimcode's default GUI build.
- **Win-GUI**: SC panel + explorer migrated; tab bar / status bar /
  activity bar / terminal queued (optional A.6/A.7 stages in vimcode's
  plan).
- **macOS**: not started; planned for v1.x.

## License

MIT OR Apache-2.0 (matches vimcode).

## Related

- [vimcode](https://github.com/JDonaghy/vimcode) — the reference
  consumer; quadraui is extracted from vimcode's UI layer.
- `docs/UI_CRATE_DESIGN.md` (in this crate) — full design doc, plugin
  invariants, decision history.
- `docs/DECISIONS.md` (in this crate) — running log of which
  primitives shipped, which deferred, and why this shape vs that one.
- vimcode's `docs/NATIVE_GUI_LESSONS.md` §12-14 — cross-backend
  bug patterns discovered shipping vimcode's three backends. Will
  migrate into `BACKEND.md` here as that doc lands.
