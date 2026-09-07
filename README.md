# quadraui

Cross-platform UI primitives with native rendering backends for **TUI**
(via ratatui), **GTK4** (via gtk4-rs + Cairo + Pango), **macOS** (Core
Graphics + Core Text), and **Windows** (Direct2D + DirectWrite via
`windows-rs`) — see *Status* below for how far each backend actually
gets.

The premise: declarative widget descriptions (`TreeView`, `MultiSectionView`,
`TabBar`, etc.) are produced once by the host app, and each backend
rasterises them in its native idiom. Paint and click consume **one**
layout instance — primitives expose a `layout(...)` helper that the
rasteriser uses internally and that hosts call to drive hit-testing.
This rules out the "paint and click drift" bug class structurally.

## Status

`0.0.x` — pre-1.0, breaking changes allowed. The TUI and GTK backends
are exercised in production by [vimcode](https://github.com/JDonaghy/vimcode).

**The Windows backend is further along than "scaffolded, no code" — its
window/event infrastructure is real and blocking on CI.** `.github/workflows/ci.yml`'s
`tui` job runs a `[ubuntu-latest, windows-latest]` matrix where *both*
legs are blocking (#590): on `windows-latest` it builds and clippy-checks
every `win_*` example against the real `windows` crate, and runs `cargo
test -p quadraui --features win` for real on that host — including the
headless `ID2D1DCRenderTarget` surface in `src/win/testing.rs`, which
needs no `HWND`, GPU, or desktop session. What's still incomplete: most
`Backend::draw_*`/`*_layout` methods on `WinBackend` are `todo!()` stubs
(`quadraui/src/win/backend.rs`) — the window, event-translation, and
platform-services layers work, but per-primitive rasterisers are largely
unwritten. This is tracked honestly rather than silently: the conformance
matrix (`quadraui/tests/conformance.rs`) registers `win` as a **burn-down**
column (quadraui#708/#722) — its cells are reported in the artifact but
don't gate CI, precisely because the rasterisers aren't done yet.

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

### What is not supported

Three capabilities are absent. They are stated here because each one is
load-bearing for somebody's adoption decision, and because
`quadraui/docs/` contains design documents for two of them that a reader
can otherwise mistake for shipped work.

**Accessibility — no assistive-technology support at all.** There is zero
AccessKit, AT-SPI, UI Automation or NSAccessibility code in the crate. A
screen reader sees nothing. `docs/UI_CRATE_DESIGN.md` decision #6 planned
`a11y_role` / `a11y_label` data fields on every primitive with platform
wiring to follow; the data fields are groundwork only and do not
constitute AT support even once they land (quadraui#835). Full
integration is a multi-backend programme, not a patch. This rules
quadraui out where a Section 508 / EN 301 549 / WCAG obligation applies.

**IME / composition — CJK input does not work.** No backend implements an
IME client protocol. GTK sees already-resolved keysyms, macOS's
`objc_key_down` bypasses `NSTextInputClient`, and Windows has no
`WM_IME_*` handling. Dead-key composition for accented Latin does not
work either. `quadraui/docs/IME_INPUT_PROPOSAL.md` is a design (#502);
the four backend integrations are tracked in quadraui#900 and are
unbuilt.

**i18n — East-Asian width only, no RTL or bidi.** Text handling accounts
for East-Asian character width via `unicode-width`, and that is the
whole of it. There is no right-to-left layout, no bidirectional
reordering, and no shaping for scripts that need it, so Arabic, Hebrew,
and Indic text render incorrectly rather than partially. This is not
planned; treat it as a scope boundary rather than a gap awaiting a fix.

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

quadraui is not published to crates.io — a bare `version = "0.0.1"` crates.io
dependency line will not resolve for anyone. Consumers pin it either by git rev or by
relative path, the same two shapes this repo's own downstream consumers
use (see `CLAUDE.md`'s *Downstream consumers* table):

```toml
[dependencies]
# Pin to a commit (coord-tui's approach):
quadraui = { git = "https://github.com/JDonaghy/quadraui", rev = "<commit-sha>", features = ["tui", "gtk"] }

# Or, for in-tree/sibling-checkout development (vimcode's approach):
quadraui = { path = "../quadraui/quadraui", features = ["tui", "gtk"] }
```

Backend-specific tests are gated on the corresponding feature. CI builds
both sets.

## Primitives

40 primitives (one module each under `quadraui/src/primitives/`),
declarative descriptions + layout + dual rasterisers. The most-used ones:

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

- [`quadraui/docs/decisions/DECISIONS.md`](quadraui/docs/decisions/DECISIONS.md) — primitive
  distinctness principles and architectural decision log.
- [`quadraui/docs/decisions/BACKEND_TRAIT_PROPOSAL.md`](quadraui/docs/decisions/BACKEND_TRAIT_PROPOSAL.md) §9 —
  resolved decisions log.
- [`quadraui/docs/NATIVE_GUI_LESSONS.md`](quadraui/docs/NATIVE_GUI_LESSONS.md) —
  pitfalls discovered while building the Win-GUI backend; apply when
  building macOS or any future native backend.
- [`quadraui/docs/CLIPBOARD.md`](quadraui/docs/CLIPBOARD.md) — how TUI
  copy reaches the system clipboard (arboard / OSC 52 / native tool),
  the tmux `set-clipboard on` + `allow-passthrough on` requirement, and
  a troubleshooting order for "Ctrl-C showed `Copied:` but nothing was
  copied" (#331).
- [`quadraui/docs/IME_INPUT_PROPOSAL.md`](quadraui/docs/IME_INPUT_PROPOSAL.md) —
  IME/composition input model proposal (issue #502): `UiEvent` preedit
  contract, GTK `IMContext` / macOS `NSTextInputClient` / Windows TSF
  mapping, caret-rect feedback channel. **Unimplemented design** — no
  backend emits a composition event; the four backend integrations are
  tracked in quadraui#900.
- [`quadraui/docs/ROWS_PROVIDER_PROPOSAL.md`](quadraui/docs/ROWS_PROVIDER_PROPOSAL.md) —
  `Rows<T>` virtualised row storage for the five collection descriptors
  (issue #837). **Design sketch, deliberately deferred**: records why a
  `Provider` arm is a breaking change to the descriptor `PartialEq` /
  `Serialize` derives rather than an additive one, that hit-testing is
  already index-based and unaffected, and what would make it worth
  building. Slice in the host until then.

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

## Terminal compatibility

quadraui's TUI backend runs over real terminal protocols — colour-depth
negotiation, SGR mouse decoding, the kitty keyboard protocol — that vary by
terminal, multiplexer, and session. The table below is **generated from
`quadraui/tests/tui_pty_smoke.rs`'s pty tier**, which spawns a real example
binary inside a real pseudo-terminal and reads the literal bytes it emits —
not hand-written, and not something a doc edit alone can change. Every row
is one of ✅ verified by a pty fixture named in the last column,
🚫 known-unsupported (a definite fact, cited, but not from a pty fixture —
usually because no such host exists in this repo's CI), or ❔ untested (no
row is ever left blank). See `quadraui/tests/terminal_matrix.rs` for how
this is enforced and how to regenerate it.

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

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
