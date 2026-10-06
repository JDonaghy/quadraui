# quadraui

**One Rust UI codebase that runs in a terminal and as a native desktop window.**

quadraui is a widget toolkit with four rendering backends: **TUI** (via
ratatui), **GTK4** (Cairo + Pango), **macOS** (Core Graphics + Core Text)
and **Windows** (Direct2D + DirectWrite). You write your app once, as a
single `AppLogic` implementation. The same code runs over SSH in a
terminal or as a native window, and only the one-line runner call in
`main` changes.

```rust
use quadraui::prelude::*;

struct Hello { keys_pressed: u32 }

impl AppLogic for Hello {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        // Describe the UI as plain data; the backend rasterises it natively.
        let bar = StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Hello, quadraui! (keys: {}) ", self.keys_pressed),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let vp = backend.viewport();
        let h = backend.measure().line_height.max(1.0) * 1.4;
        let _ = backend.draw_status_bar_interactive(
            Rect::new(0.0, vp.height - h, vp.width, h),
            &bar,
            &quadraui::InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key: Key::Char('q'), .. } => Reaction::Exit,
            UiEvent::KeyPressed { .. } => { self.keys_pressed += 1; Reaction::Redraw }
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::io::Result<()> {
    quadraui::tui::run(Hello { keys_pressed: 0 })
    // quadraui::gtk::run(...), quadraui::macos::run(...) or
    // quadraui::win::run(...) for a native window: same `Hello`.
}
```

The full version is `quadraui/examples/hello.rs`:
`cargo run --example hello --features tui`.

## Why it is different

- **Terminal and native from one codebase.** Ratatui and Textual have no
  native backend; egui, iced and Slint have no terminal backend. quadraui
  has both, behind one trait.
- **Paint and click share one layout.** Every primitive exposes a
  `layout(...)` that the rasteriser paints from and the host hit-tests
  against. Mouse clicks cannot drift from what is on screen, because both
  read the same computed rectangles.
- **Small, and idle when idle.** A macOS release binary measured 0.7–2.0 MB
  in the 2026-09-26 audit. The runtime is event-driven: an app with
  nothing to do renders nothing.
- **Declarative, serialisable widgets.** Primitives are plain data
  (`Serialize`), events are plain data routed by `WidgetId`, and no
  closures cross the API. That is what makes scripting practical (vimcode
  lets Lua extensions declare quadraui views), and later, language
  bindings.
- **Honest about capabilities.** `BackendCaps` tells an app what the
  current backend can do. Where a terminal cannot do something (a tray
  icon, a native dialog) the backend degrades and says so, rather than
  silently doing nothing.
- **Heavily tested.** Around 4,000 tests, including headless
  paint-then-click round trips on all four backends, an end-to-end
  `TuiDriver` that drives real example apps through the event loop, and
  benchmarks for 100k-row tables and trees and a 4k-line editor.

## Who it is for

**A good fit:** keyboard-driven developer, ops and data tools (editors,
dashboards, log viewers, database and HTTP clients) that should run in a
terminal over SSH *and* as a native window, written in Rust, by people
comfortable with a pre-1.0 API.

**Not a fit, today:**

- **Accessibility.** There is no assistive-technology support: no
  AccessKit, AT-SPI, UI Automation or NSAccessibility. A screen reader sees
  nothing. That rules quadraui out wherever a Section 508, EN 301 549 or
  WCAG obligation applies. Tracked in quadraui#1309.
- **CJK and other IME input.** No backend implements an input-method
  protocol, so composed input (CJK, and dead-key accents) does not work.
  The design is `quadraui/docs/IME_INPUT_PROPOSAL.md`; the backend work is
  quadraui#900.
- **Right-to-left text.** East-Asian character width is handled; RTL,
  bidi and complex shaping are not, and are out of scope.
- **Multiple windows** (quadraui#1120), and **languages other than Rust**
  (bindings are a later phase, quadraui#1096).

## Status

`0.1.x` — pre-1.0. The crate is prepared for its first publish to
crates.io (quadraui#1111); until an operator actually runs `cargo
publish` and tags `v0.1.0`, depend on it by git revision as shown
above. Breaking changes are batched and recorded in `CHANGELOG.md`; the
release order and the 1.0 bar are in [`ROADMAP.md`](ROADMAP.md).

| Backend | State |
|---|---|
| TUI | In production use in vimcode and coord-tui. Every rasteriser shipped. |
| GTK4 (Linux) | In production use in vimcode. Every rasteriser shipped. |
| macOS | Implements the whole `Backend` trait, including native menus, file dialogs and a client-side title bar. Built and tested on `macos-latest` CI for every PR that touches `quadraui/src`. |
| Windows | Every rasteriser shipped, with no `todo!()` left in `src/win/backend.rs`. Builds and tests are blocking on `windows-latest` CI. The cross-backend conformance matrix still reports it as a burn-down column rather than a blocking one: some paint-then-click cases do not pass yet. |

The real consumers are [vimcode](https://github.com/JDonaghy/vimcode), a
Vim-compatible editor that runs on all four backends, and
[coord-tui](https://github.com/JDonaghy/coord-tui), a terminal dashboard.
Both are by the same author, so quadraui has not yet been tried by an
outside team. If you build something on it, please open an issue; that
feedback is the most useful thing the project can get right now.

## How it was built

quadraui is developed almost entirely by AI coding agents. Of its first
1,314 commits (April to October 2026), 1,107 are co-authored by Claude.
The agents are coordinated by
[code-coordinator](https://github.com/JDonaghy/code-coordinator), which
dispatches each issue to a worker on one of several machines and gates
every merge on tests and on an adversarial review by a separate agent
with no shared context. That pipeline, not a person reading every diff,
is what holds the agents to the repository's rules.

You do not have to take that on trust. An independent audit of the
framework, commissioned and published unedited, is at
[`quadraui/docs/audits/FRAMEWORK_AUDIT_2026-09-26.md`](quadraui/docs/audits/FRAMEWORK_AUDIT_2026-09-26.md).
It covers code quality, the alternatives, and what has to be true before
1.0. Its recommendations are tracked as epics on this repository's issue
tracker.

## Getting started

```toml
[dependencies]
quadraui = { git = "https://github.com/JDonaghy/quadraui", rev = "<commit-sha>", features = ["tui", "gtk"] }
```

Pin a commit rather than following `develop`; both real consumers do. For
sibling-checkout development, use
`quadraui = { path = "../quadraui/quadraui", features = [...] }`.

Then:

- Run `hello`, then `tui_demo` / `gtk_demo`, which use the same `AppLogic`
  body under two runners.
- Read [`quadraui/docs/GUIDE.md`](quadraui/docs/GUIDE.md) for the app
  model and [`quadraui/docs/APP_ARCHITECTURE.md`](quadraui/docs/APP_ARCHITECTURE.md)
  for how a larger app is put together.
- Browse `quadraui/examples/`. Most examples come in `tui_*` / `gtk_*`
  pairs (some also `macos_*` / `win_*`), so you can compare backends.

## Features

Pick the backends you ship; `terminal` is independent of them.

- `terminal` — PTY + vt100 + scrollback engine (`quadraui::terminal_engine`)
  for embedding a terminal. No rasteriser; pairs with any backend below.
- `tui` — terminal backend (`quadraui::tui`), via ratatui.
- `gtk` — GTK4 backend (`quadraui::gtk`), via gtk4-rs + Cairo + Pango.
- `macos` — macOS backend (`quadraui::macos`), via Core Graphics + Core
  Text. Compiles only on `target_os = "macos"`.
- `win` — Windows backend (`quadraui::win`), via Direct2D + DirectWrite
  (`windows-rs`). Compiles on every host; only the WinAPI calls are
  `cfg(target_os = "windows")`.
- `layout` — optional flex/grid layout module (`quadraui::flex`), via
  [Taffy](https://github.com/DioxusLabs/taffy). Unit-agnostic (plain
  `f32` in, plain `f32` out); pairs with any backend above, or none.

## What's in the box

**41 primitives**, one module each under `quadraui/src/primitives/`: a
declarative description, a shared layout, and a rasteriser per backend.
Among them:

- Content: `TreeView`, `ListView`, `DataTable`, `Form`, `Editor`,
  `TextDisplay`, `MessageList`, `DiffView`, `Chart`, `Board`, `Terminal`,
  `Minimap`, `Canvas` (app-defined drawing, painted through the public
  `PaintSurface` seam).
- Chrome: `TabBar`, `StatusBar`, `MenuBar`, `ActivityBar`, `Toolbar`,
  `CommandCenter`, `Scrollbar`.
- Containers: `Split`, `SplitTree`, `Panel`, `MultiSectionView`.
- Overlays: `Dialog`, `Palette`, `ContextMenu`, `Tooltip`, `Completions`,
  `FindReplace`, `Toast`, plus `ProgressBar` and `Spinner`.

**Compose controllers** in `quadraui::compose` own the interaction state
machines, so an app matches on semantic events (`MenuEvent::Activated`,
`SidebarEvent::RowSelected`) instead of raw mouse coordinates:

| Controller | What it handles |
|---|---|
| `AppShell` | Activity bar + sidebar + editor area: the VS Code-style application frame. |
| `BottomPanel` | A tabbed, dockable panel along the bottom of an `AppShell`. |
| `WorkspaceController` | Many open documents, one visible: open, switch, close. |
| `TabGroup` | Tabbed split panes. |
| `MenuSystem` | Menu bar + dropdowns: Alt-key activation, arrows, hover-to-switch, modal stack. |
| `ContextMenuController` | One call for a context menu, native or painted per platform. |
| `SidebarSystem` | Multi-section sidebar: per-section scroll and selection, Tab cycling, scrollbar drag. |
| `SidebarPanelBody` | The standard layers of a sidebar panel body, composed. |
| `TreeController` | A keyboard-navigable tree: selection, expand/collapse, scroll-follow. |
| `FormController` | A form's focus, editing and validation. |
| `DualModePaletteController` | A command palette that switches between modes. |
| `ChatController` | A chat overlay: transcript plus input. |
| `FilePickerController` / `FolderPickerController` | In-app file and folder pickers for backends without a native dialog. |
| `MessageDialogController` | Message and confirmation boxes. |
| `ToastStackController` | Keyboard focus for actionable notifications. |
| `FocusRing` / `FocusGroup` | Tab / Shift+Tab focus cycling. |
| `HelpOverlayController` | A context-sensitive key-binding help overlay. |
| `StatusBarInteraction` / `ToolbarHoverTracker` | Hover and press state for status-bar segments and toolbar buttons. |

Also in `compose`: a Markdown-to-`StyledText` adapter, and
`notify_or_toast`, which sends a system notification where the platform
has one and shows a toast where it does not.

See [`quadraui/docs/COMPOSE.md`](quadraui/docs/COMPOSE.md) for what each
controller owns, the events it emits, the smallest wiring snippet, and
which example file demonstrates it.

**Platform services:** clipboard, file open/save and folder dialogs,
message dialogs, notifications, `open_url`, reveal-in-file-manager, move
to trash, secret storage, OS file drop, window control and a tray icon.
Each is reported through `BackendCaps`, and degrades honestly on the
terminal.

## Testing

- **Paint-then-click round trips.** Paint a primitive into a headless
  surface, find the painted glyphs, click those exact coordinates, and
  assert paint and click identify the same widget.
- **Conformance matrix.** `quadraui/tests/conformance.rs` runs the same
  scenarios against every backend and reports a per-primitive matrix.
- **End-to-end drivers.** `quadraui::tui::testing::TuiDriver` runs a real
  `AppLogic` through the event → `handle` → `render` path against an
  in-memory terminal, with scripted keys, clicks and drags. See
  [`quadraui/docs/TESTING.md`](quadraui/docs/TESTING.md).
- **Real terminals.** A pty tier spawns example binaries in a real
  pseudo-terminal and checks the bytes they emit. Its results generate the
  table below.

### Terminal compatibility

Generated from `quadraui/tests/tui_pty_smoke.rs`'s pty tier, not
hand-written. ✅ is verified by the named pty fixture, 🚫 is a cited
known limitation, ❔ is untested. See `quadraui/tests/terminal_matrix.rs`
for how to regenerate it.

<!-- TERMINAL_MATRIX:START — generated by quadraui/tests/terminal_matrix.rs (quadraui#829); do not hand-edit, see that file's module doc for how to regenerate -->
| Axis | Condition | Status | Verified by |
|---|---|---|---|
| Multiplexer | tmux, no passthrough configured (default) | ❔ untested | no tmux binary in CI — docs/KITTY_KEYBOARD_PROTOCOL.md tmux row, docs/CLIPBOARD.md |
| Multiplexer | tmux, `set-clipboard on` + `allow-passthrough on` | ❔ untested | no tmux binary in CI — docs/CLIPBOARD.md tmux knobs |
| Multiplexer | mosh | ❔ untested | no mosh available in CI — docs/KITTY_KEYBOARD_PROTOCOL.md |
| Colour depth | `COLORTERM=truecolor` — 24-bit | ✅ verified | sgr_color_depth::tui_pipeline_with_colorterm_truecolor_is_byte_identical_to_before |
| Colour depth | `TERM` contains `256color`, no `COLORTERM` — 256-indexed | ✅ verified | sgr_color_depth::tui_pipeline_under_256color_term_emits_indexed_sgr_not_truecolor |
| Colour depth | `TERM=xterm` (no `256color`, no `COLORTERM`) — 16-colour fallback | ✅ verified | sgr_color_depth::tui_pipeline_under_plain_xterm_emits_ansi16_sgr |
| Mouse mode | Default `RunConfig` — capture negotiated (`?1000h` et al. sent) | ✅ verified | no_mouse::tui_split_negotiates_mouse_capture_by_default |
| Mouse mode | `RunConfig::no_mouse()` — capture withheld, keyboard-only stays operable | ✅ verified | no_mouse::tui_no_mouse_never_negotiates_mouse_capture_and_stays_keyboard_operable |
| Mouse mode | SGR mouse-motion report decodes without leaking into a focused text input (#293) | ✅ verified | tui_chat_sgr_mouse_motion_does_not_leak_into_input |
| Mouse mode | Escape glued to an SGR motion report in one `write()` (#293 race) | ✅ verified | tui_chat_escape_glued_to_sgr_motion_in_one_write_does_not_leak |
| Key protocol (kitty) | `TERM=xterm-kitty`, live query unanswered — env-heuristic fallback fires | ✅ verified | kitty_keyboard_protocol::tui_pipeline_under_kitty_term_pushes_enhancement_flags |
| Key protocol (kitty) | Plain `TERM=xterm-256color`, no kitty/WezTerm/foot signal | ✅ verified | kitty_keyboard_protocol::tui_pipeline_under_plain_term_does_not_push_enhancement_flags |
| Key protocol (kitty) | Real kitty terminal, live query actually answered | ❔ untested | no kitty binary in CI — docs/KITTY_KEYBOARD_PROTOCOL.md degrade table |
| Key protocol (kitty) | WezTerm / foot / Alacritty ≥0.12 | ❔ untested | unit-tested heuristic only, no real binary in CI — docs/KITTY_KEYBOARD_PROTOCOL.md |
| Key protocol (kitty) | Windows Terminal / any Windows console host | 🚫 known-unsupported | crossterm-0.29.0's Windows `supports_keyboard_enhancement()` is an unconditional `Ok(false)` — docs/KITTY_KEYBOARD_PROTOCOL.md |
| Key protocol (kitty) | Terminal.app (macOS) | ❔ untested | no macOS runner exercises the TUI backend under Terminal.app — docs/KITTY_KEYBOARD_PROTOCOL.md |
| Key protocol (kitty) | Serial console | ❔ untested | no serial hardware/emulation in CI — docs/KITTY_KEYBOARD_PROTOCOL.md |
<!-- TERMINAL_MATRIX:END -->

## Workspace

| Crate | Purpose |
|---|---|
| `quadraui` | The library. |
| `kubeui-core`, `kubeui`, `kubeui-gtk` | A small Kubernetes dashboard demo: TUI and GTK front ends over shared domain logic. A demo, not a production consumer. |
| `tools/lint` | Repository lints run in CI. |

## Design documents

- [`quadraui/docs/UI_CRATE_DESIGN.md`](quadraui/docs/UI_CRATE_DESIGN.md) — goals, non-goals and the core invariants.
- [`quadraui/docs/COMPOSE.md`](quadraui/docs/COMPOSE.md) — every compose controller: what it owns, the events it emits, the smallest wiring example, and which example file shows it.
- [`quadraui/docs/PRIMITIVE_RULES.md`](quadraui/docs/PRIMITIVE_RULES.md) — the rules every primitive follows, including the public-API lifecycle.
- [`quadraui/docs/decisions/DECISIONS.md`](quadraui/docs/decisions/DECISIONS.md) — architectural decision log.
- [`quadraui/docs/NATIVE_GUI_LESSONS.md`](quadraui/docs/NATIVE_GUI_LESSONS.md) — pitfalls found building the native backends.
- [`quadraui/docs/CLIPBOARD.md`](quadraui/docs/CLIPBOARD.md) — how terminal copy reaches the system clipboard, including tmux setup.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
