# quadraui

Cross-platform UI primitives with native rendering backends for **TUI**
(via ratatui), **GTK4** (via gtk4-rs + Cairo + Pango), **macOS** (Core
Graphics + Core Text), and — designed-for, not-yet-implemented —
**Windows** (Direct2D + DirectWrite).

The premise: declarative widget descriptions (`TreeView`, `MultiSectionView`,
`TabBar`, etc.) are produced once by the host app, and each backend
rasterises them in its native idiom. Paint and click consume **one**
layout instance — primitives expose a `layout(...)` helper that the
rasteriser uses internally and that hosts call to drive hit-testing.
This rules out the "paint and click drift" bug class structurally.

## Status

`0.0.x` — pre-1.0, breaking changes allowed. The TUI and GTK backends
are exercised in production by [vimcode](https://github.com/JDonaghy/vimcode).
The Windows backend is scaffolded but not implemented yet.

**The macOS backend implements the whole `Backend` trait, and
`macos-latest` CI builds and tests it** (`.github/workflows/macos.yml`,
triggered by any PR touching `quadraui/src/macos/**`). Until quadraui#484
that claim was untested: the backend is gated on `target_os = "macos"`,
so `cargo check --features macos` on a Linux host compiled none of it,
and the first real macOS compile found seven missing trait methods, an
arity mismatch, and three primitives with no rasteriser at all. Those are
closed. Two documented divergences from the GTK twin remain, both in
`src/macos`'s own module docs: `tab_bar` returns bar-relative rather than
absolute hit x (quadraui#552 follow-up), and `terminal` does not yet do
wide-char (CJK / emoji) advance handling (quadraui#440).

Every in-window rasteriser shipped — chrome (StatusBar, TabBar,
ActivityBar, CommandCenter, MenuBar, CommandLine, settings chrome),
content (Tree, List, Form, Editor, DataTable, Chart, Board, DiffView),
MSV + Scrollbar, containers + indicators (Panel, Split, SplitTree, Toast,
Progress, Spinner), overlays (Tooltip, ContextMenu, Dialog, Palette,
Completions, FindReplace, RichTextPopup, DropOverlay), and
streaming/cell primitives (Terminal, TextDisplay, MessageList) — plus
the native-feel integration layer:
platform services (clipboard via `arboard`, `NSOpenPanel` /
`NSSavePanel` file dialogs, `osascript` notifications, `open` URL
handler), native `NSMenu` menu bar with auto-prepended app menu and
⌘-shortcuts, native right-click context menus via
`NSMenu.popUpMenuPositioningItem`, form chrome for ToggleGroup /
SegmentedControl / ButtonRow / PasswordInput, and animated
`InlineInput` caret blink driven by an `NSTimer` (~530 ms, pauses for
500 ms after a keystroke). See `SESSION_HISTORY.md` for details.

## Workspace

| Crate | Purpose |
|---|---|
| `quadraui` | The core library — primitives, types, theme, backend traits, TUI + GTK rasterisers. |
| `kubeui-core` | Domain logic for a Kubernetes dashboard demo (no rendering deps). |
| `kubeui` | TUI-rendered Kubernetes dashboard. Real consumer that exercises `MultiSectionView`, `TreeView`, `Form`, `StatusBar`, `Scrollbar`. |
| `kubeui-gtk` | GTK-rendered Kubernetes dashboard. Same domain logic as `kubeui`; different backend. |

Demo crates are kept inside this repo so primitive changes can be
validated end-to-end before merge — they're not example code, they're
real apps under development.

## Features

- `tui` — TUI rasteriser (`quadraui::tui::draw_*`).
- `gtk` — GTK4 rasteriser (`quadraui::gtk::draw_*`).

A consumer that needs both:

```toml
[dependencies]
quadraui = { version = "0.0.1", features = ["tui", "gtk"] }
```

Backend-specific tests are gated on the corresponding feature. CI builds
both sets.

## Primitives

Current set (declarative descriptions + layout + dual rasterisers):

- `TreeView` — flat-rendered, scroll-aware, hit-testable.
- `ListView` — single-column scrollable list.
- `Form` — field/value rows with caret-aware text input.
- `Tabs` (`TabBar`) — horizontal tab strip with active scroll.
- `StatusBar` — left/right segment list with action dispatch.
- `Scrollbar` — vertical scrollbar primitive.
- `MultiSectionView` — vertically stacked, individually sized,
  collapsible sections — each containing its own scrollable body.
  Composes other primitives as section bodies.
- `MessageList` — chat-style message history.
- `Editor` — code-editor primitive (gutter, virtual text, syntax spans).
- `MenuBar` — horizontal menu strip with dropdown menus via `ContextMenu`
  composition, hover-to-switch, Alt-key activation.
- `Split` — two-pane container with draggable divider, horizontal +
  vertical, min-size constraints.
- `SplitTree` — N-way recursive split tree (arbitrary nesting of
  horizontal/vertical `Split` nodes, addressed by pre-order index) for
  hosts like editor-group layouts or vim-style window splits that
  `Split`'s fixed two-pane shape can't express.
- `Panel` — container chrome with title bar, action buttons, content region.
- `Toast` (`ToastStack`) — corner-stacked notification boxes with
  severity tint, dismiss, action buttons.
- `ProgressBar` — determinate/indeterminate bar with optional cancel.
- `Spinner` — indeterminate braille animation glyph + label.
- `CommandCenter` — back/forward nav arrows + search box for menu bar row.
- `Minimap` — code-overview minimap: GTK paints real glyphs via font
  scaling, TUI packs `U+2800`-block braille dots — same `sample_lines` /
  `aggregate_spans` data on both (#382).
- Plus: `Tooltip`, `ContextMenu`, `Dialog`, `Palette`, `Terminal`,
  `RichTextPopup`, `TextDisplay` (with optional scrollbar), etc.

## Design

- [`quadraui/docs/DECISIONS.md`](quadraui/docs/DECISIONS.md) — primitive
  distinctness principles and architectural decision log.
- [`quadraui/docs/BACKEND_TRAIT_PROPOSAL.md`](quadraui/docs/BACKEND_TRAIT_PROPOSAL.md) §9 —
  resolved decisions log.
- [`quadraui/docs/NATIVE_GUI_LESSONS.md`](quadraui/docs/NATIVE_GUI_LESSONS.md) —
  pitfalls discovered while building the Win-GUI backend; apply when
  building macOS or any future native backend.

## Examples

Runnable from the workspace root with `cargo run --example <name> --features <backend>`:

| Example | Backend | What it shows |
|---|---|---|
| `tui_app` / `gtk_app` | `tui` / `gtk` | Minimal `AppLogic` with a single `StatusBar`. The smallest possible runner-driven app. |
| `tui_demo` / `gtk_demo` | `tui` / `gtk` | `TabBar` + `StatusBar` with focus cycling. Same `AppLogic` body across backends — only the runner call differs. |
| `msv_multi_tree` / `gtk_multi_tree` | `tui` / `gtk` | Debug-sidebar using `SidebarSystem` compose helper: 4 `EqualShare` `TreeView` sections with per-section scroll/selection, keyboard nav, scrollbar drag — all handled by `SidebarSystem`. See *Compose helpers* below. |
| `msv_sc_panel` | `tui` | Source-Control consumer pattern: `SectionAux::Input` commit message editor + N collapsible `TreeView` sections (Changes / Staged / Worktrees). Adds input-mode keystroke routing + chevron-click collapse toggle on top of the multi-tree shape. |
| `tui_menu_bar` / `gtk_menu_bar` | `tui` / `gtk` | Complete menu bar using `MenuSystem` compose helper: dropdown menus, hover-to-switch, keyboard navigation (Alt+key, arrows, Enter, Esc). See *Compose helpers* below. |
| `tui_split` / `gtk_split` | `tui` / `gtk` | Draggable `Split` with two labelled panes. Toggle horizontal/vertical, reset ratio. |
| `tui_split_tree` / `gtk_split_tree` | `tui` / `gtk` | `SplitTree` 3-way nested split (`Split(H, Split(V, A, B), C)`); every divider draggable via `DragTarget::SplitDivider`. |
| `tui_panel` / `gtk_panel` | `tui` / `gtk` | `Panel` with title bar, close/maximize actions, content area, collapse toggle. |
| `tui_toast` / `gtk_toast` | `tui` / `gtk` | `ToastStack` with severity tints, dismiss, action buttons. |
| `tui_tooltip` / `gtk_tooltip` | `tui` / `gtk` | `Tooltip` border vocabulary (#541): cycle `Sides` / `Full` / `None` chrome and toggle a title embedded in `Full`'s top border row. |
| `tui_indicators` / `gtk_indicators` | `tui` / `gtk` | `ProgressBar` + `Spinner` demo — determinate/indeterminate, cancel. |
| `tui_search_panel` / `gtk_search_panel` | `tui` / `gtk` | Search panel spike: `MultiSectionView` + `TreeView` composition for file-search results. |
| `tui_form_groups` / `gtk_form_groups` | `tui` / `gtk` | `Form` with `ToggleGroup` + `ButtonRow` horizontal field kinds, plus `FocusRing` for Tab/Shift+Tab cycling. Mini search/replace panel shape. |

## Compose Helpers

High-level controllers in `quadraui::compose` that combine multiple
primitives into reusable interaction patterns. Apps define structure,
the helper owns the state machine, and the app matches on semantic
events.

| Helper | Primitives | What it handles |
|---|---|---|
| `FocusRing` | Any focusable widgets | Tab/Shift+Tab cycling through a list of `WidgetId`s. `advance()`, `retreat()`, `set()`, `current()`. Eliminates repeated modulo arithmetic. |
| `MenuSystem` | `MenuBar` + `ContextMenu` | Open/close, Alt+key activation, arrow navigation, hover-to-switch, modal stack, dropdown anchoring. App matches on `MenuEvent::Activated(WidgetId)`. |
| `SidebarSystem` | `MultiSectionView` + `TreeView` | Per-section scroll/selection, Tab cycling, scrollbar drag, two-layer click dispatch (MSV → TreeView with coordinate translation). App matches on `SidebarEvent::RowSelected { section, path }`. |

## Testing

Each backend has paint↔click round-trip tests in `quadraui/src/tui/*::tests`
that paint into a virtual buffer, find painted glyphs, hit-test those
exact coordinates, and assert paint and click identify the same widget
region. These catch "paint and click coordinate-system drift" bugs that
unit tests of either path alone would miss. The pattern is being rolled
out across primitives — see PR history for `cell_quantum` (#297), MSV
harness (#298), TreeView harness (#299).

Consumer-pattern integrations get an additional **consumer-state**
round-trip layer alongside the primitive harness: paint, simulate the
host's click-routing + state mutations, assert the host's state
changes match the painted UI. See `quadraui/src/tui/multi_section_view.rs`
"Consumer-state round-trip harness" for the canonical block.

Whole examples get an **end-to-end driver** layer: `quadraui::tui::testing::TuiDriver`
runs a shipping `AppLogic` (the same type the `tui_*` examples instantiate)
through the real event → `handle` → `render` path against ratatui's
in-memory `TestBackend` — no TTY, no pty, deterministic. Script
keystrokes/clicks/drags (`press`, `click`, `drag`, `ctrl_char`) and
assert on the rendered screen. Because `AppLogic` is backend-neutral the
same event script is the basis for a future cross-backend `GtkDriver`.
See `quadraui/tests/tui_example_driver.rs` and `docs/TESTING.md`.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
