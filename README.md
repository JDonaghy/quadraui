# quadraui

Cross-platform UI primitives with native rendering backends for **TUI**
(via ratatui), **GTK4** (via gtk4-rs + Cairo + Pango), and — designed-
for, not-yet-implemented — **Windows** (Direct2D + DirectWrite) and
**macOS** (Core Graphics + Core Text).

The premise: declarative widget descriptions (`TreeView`, `MultiSectionView`,
`TabBar`, etc.) are produced once by the host app, and each backend
rasterises them in its native idiom. Paint and click consume **one**
layout instance — primitives expose a `layout(...)` helper that the
rasteriser uses internally and that hosts call to drive hit-testing.
This rules out the "paint and click drift" bug class structurally.

## Status

`0.0.x` — pre-1.0, breaking changes allowed. The TUI and GTK backends
are exercised in production by [vimcode](https://github.com/JDonaghy/vimcode).
Windows and macOS backends are scaffolded but not implemented yet.

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
- Plus: `Tooltip`, `ContextMenu`, `Dialog`, `Palette`, `Terminal`,
  `RichTextPopup`, etc.

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
| `msv_multi_tree` / `gtk_multi_tree` | `tui` / `gtk` | Debug-sidebar consumer pattern: 4 `EqualShare` `TreeView` sections in a `MultiSectionView`, with per-section `scroll_offset` + `selected_path` owned by the host. Both are ~24-line runner shells; the shared `AppLogic` impl lives in `examples/common/multi_tree.rs`. See *Consumer patterns* in `CLAUDE.md`. |
| `msv_sc_panel` | `tui` | Source-Control consumer pattern: `SectionAux::Input` commit message editor + N collapsible `TreeView` sections (Changes / Staged / Worktrees). Adds input-mode keystroke routing + chevron-click collapse toggle on top of the multi-tree shape. |
| `tui_menu_bar` / `gtk_menu_bar` | `tui` / `gtk` | Complete menu bar with dropdown menus via `MenuBar` + `ContextMenu` composition. Hover-to-switch, keyboard navigation (Alt+key, arrows, Enter, Esc), realistic File/Edit/View menus. |
| `tui_split` / `gtk_split` | `tui` / `gtk` | Draggable `Split` with two labelled panes. Toggle horizontal/vertical, reset ratio. |

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

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
