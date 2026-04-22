# Application architecture on top of quadraui

**Audience:** developers writing an app that consumes `quadraui` (today
only vimcode; tomorrow the k8s dashboard [#145], the SQL client [#46],
the Postman clone [#147]).

**Counterpart doc for backend implementers:** `docs/NATIVE_GUI_LESSONS.md`
in the vimcode repo. That file tells you what to watch for when porting
`quadraui_{tui,gtk,win}` to a new target (e.g. macOS Core Graphics).
This file is the other side of the coin: where does *your* feature
logic live, given that quadraui intentionally does not own layout or
event routing?

**Status disclosure.** quadraui is in Phase A — a catalog of
**declarative primitives** (Tree, Form, List, Palette, StatusBar, TabBar,
ActivityBar, Terminal, TextDisplay). The more ambitious surface
described in `UI_CRATE_DESIGN.md` §4.1 / §6 (`Window`, `Panel`, `Split`,
`Tabs`, `MenuBar`, `Accelerator`, a unified `Backend` trait) is
**roadmapped but not yet shipped**. Until it lands, app authors will
write more per-backend plumbing than the end-state vision implies.
This doc captures the current layering and points out which
duplications collapse when Phase B extraction happens.

---

## The layer cake

```
┌──────────────────────────────────────────────────────────────────┐
│  App state (vimcode's Engine)                                    │
│  — persistent state, ex-commands, key-handling core, settings    │
└──────────────────────────────────────────────────────────────────┘
                           │  each frame
                           ▼
┌──────────────────────────────────────────────────────────────────┐
│  Render adapter (src/render.rs::build_screen_layout)             │
│  — pure function: Engine + Theme + viewport rects → ScreenLayout │
│  — also: state → quadraui primitive (Tree, Form, List, …)        │
└──────────────────────────────────────────────────────────────────┘
                           │  ScreenLayout + primitives
                           ▼
┌──────────────────────────────────────────────────────────────────┐
│  Backend (src/tui_main, src/gtk, src/win_gui)                    │
│  — draw: consume ScreenLayout + primitives, rasterise            │
│  — route: key / mouse / resize events → Engine methods           │
│  — measure: provide native units (cells, Pango px, DirectWrite)  │
└──────────────────────────────────────────────────────────────────┘
                           │  events
                           ▼
                      back to Engine
```

Rules of the layer cake:

1. **State flows down.** Engine is the single source of truth for
   persistent app state. Render adapter is a pure view function.
2. **Measurements flow up.** Only the backend knows its native text
   width / row height / viewport pixels. Anything in render or engine
   that depends on measurement takes a closure or a struct filled in
   by the backend (see `PanelChromeDesc` below and
   `quadraui::StatusBar::fit_right_start<F>`).
3. **Events flow back to Engine.** Backends never retain app state;
   they translate key / click / resize into Engine method calls.

---

## Where does each kind of feature go?

| Kind of thing                         | Example                             | Lives in                                                                                            |
| ------------------------------------- | ----------------------------------- | --------------------------------------------------------------------------------------------------- |
| Pure render data                      | "show N items in a list"            | Primitive struct, built by adapter in `render.rs` each frame                                        |
| Persistent user-facing flag           | `terminal_maximized`, `sidebar_visible` | `Engine` field                                                                                      |
| User-preferred dimension              | `terminal_panel_rows`               | `SessionState` (persists to disk)                                                                   |
| Effective / derived value             | max-content-rows when maximized     | `Engine::effective_*` method — **called every frame by backends**                                   |
| Ex-command                            | `:TerminalMaximize`                 | `execute.rs` + `EngineAction` variant                                                               |
| Keybinding                            | `Ctrl+Shift+T`                      | `settings::PanelKeys` field + each backend's key dispatcher                                         |
| Viewport geometry read                | DA height, screen rows, client rect | Each backend, in its native event / draw callback                                                   |
| Chrome reservation math               | "how big can the panel be?"         | **Shared via `core::engine::PanelChromeDesc`** — backends fill in row counts, engine does subtraction |
| Mouse hit-test                        | toolbar click zones                 | Backend — but **must use the effective value** the draw code used                                   |
| Primitive-carried flag                | e.g. `TerminalPanel.maximized`      | `render.rs`, so backends can switch icon glyph / render differently                                 |
| PTY / subprocess dimension            | shell reflow on resize              | Engine method called from backend's resize handler                                                  |

---

## Worked example — terminal maximize

The maximize feature (issue #34, shipped `45511d7..5334fac`) touched
10 files and is a good stress-test of the current layering. Here's
the whole feature traced through the cake.

### Engine layer — `src/core/engine/`

| Artifact | File:line | Purpose |
|---|---|---|
| `terminal_maximized: bool` | `mod.rs` — `Engine` struct | Persistent flag |
| `terminal_saved_rows` — *deleted* | (was in struct) | **Anti-pattern:** originally saved the pre-maximize height and restored on un-maximize. Failed on window resize because the saved value was stale. |
| `toggle_terminal_maximize()` | `terminal_ops.rs` | Flips the flag only. No argument. |
| `effective_terminal_panel_rows(max_target_rows)` | `terminal_ops.rs` | `if maximized { max_target_rows.max(stored).max(5) } else { stored }` — **called every frame** |
| `PanelChromeDesc` + `max_panel_content_rows()` | `mod.rs` (near `EngineAction`) | Shared chrome arithmetic — backends fill a struct, engine subtracts |
| `EngineAction::ToggleTerminalMaximize` | `mod.rs` | Returned by `:TerminalMaximize` so backends can compute size then call the toggle |
| `:TerminalMaximize` / `:TerminalMax` | `execute.rs` | Ex command |

### Render adapter — `src/render.rs`

| Artifact | Purpose |
|---|---|
| `TerminalPanel.maximized: bool` | Field on the primitive so backends can flip the icon (`󰊗` vs `󰊓`) |
| No other changes | The `Terminal` primitive itself is still declarative — maximize is **app state**, not primitive state |

### Per-backend plumbing (currently duplicated across TUI, GTK, Win-GUI)

| Concern | TUI | GTK | Win-GUI |
|---|---|---|---|
| Key intercept (`Ctrl+Shift+T`) | `tui_main/mod.rs` main event loop | `gtk/mod.rs` key controller | `win_gui/mod.rs::on_key_down` |
| Toolbar click (`󰊗` button) | `tui_main/mouse.rs` | `gtk/mod.rs` in-terminal click branch | `win_gui/mod.rs` toolbar hit-test |
| Chrome-rows target | `terminal_target_maximize_rows_tui` | `gtk_terminal_target_maximize_rows` | `win_gui_terminal_target_maximize_rows` |
| Window-resize PTY reflow | terminal panel gets re-laid-out each frame; PTY resized inline | `connect_resize` signal | `WM_SIZE` / `on_resize` |
| Layout math | `bottom_panel_height = effective + 2` in `render_impl.rs` | `term_px = (effective + 2) * lh` in `draw.rs` + `gtk_editor_bottom` | `win_gui/draw.rs` terminal panel |
| Mouse hit-test geometry | `mouse.rs::effective_terminal_panel_rows_tui` helper | `gtk/mod.rs` terminal zone compute | `win_gui/mod.rs` click math |
| Breadcrumb suppression | `render_impl.rs` conditional on `engine.terminal_maximized` | `gtk/draw.rs` conditional | (not applicable — Win-GUI breadcrumb already handled globally) |
| Per-window status suppression | — (absent via viewport collapse) | `gtk/draw.rs` conditional | — |

**Three of those rows are shared via `PanelChromeDesc`** (all three
`*_target_maximize_rows` helpers now just fill the struct and call
`.max_panel_content_rows()`). The rest are genuinely per-backend —
window-resize events and keybinding dispatch work differently on
crossterm vs GTK4 vs Win32.

### What would collapse with Phase B layout primitives?

If `quadraui::Panel` (or `Layout`) ships per the roadmap:

- **Key intercept + toolbar click** would go through a shared
  `UiEvent` dispatch table the app registers once.
- **Chrome-rows math** would be owned by the `Layout` description —
  the panel's min/max/flex/hidden rules would be declarative, and
  backends would render to the computed rects without re-doing the
  subtraction.
- **Window-resize PTY reflow** would be a `LayoutEvent::PanelResized`
  the app subscribes to once, not per-backend wiring.

Until that ships, treat the maximize feature as the reference
pattern for any "stateful chrome" addition.

---

## Rules of thumb

**1. Mutating persisted state at toggle time breaks on window resize.**
Use a flag + a per-frame `effective_*` accessor that the render code
calls every frame. Commit `5bcb1bd` contains the refactor that
learned this.

**2. Mouse hit-tests mirror draw-time geometry.** Every backend site
that reads `session.terminal_panel_rows` (or any stored dimension)
for hit-testing needs the effective value when a maximize-class
state is involved. Grep for the raw field after every such feature
— we shipped bugs twice (`1d7141a` for GTK, `507d63a` for TUI)
because one more `_panel_rows` reference snuck through.

**3. Three backends means three hit-tests.** When you wire a new
clickable UI element, check every `quadraui_{tui,gtk,win}.rs` and
every backend's click/key dispatcher. A fourth (macOS) will come
eventually; keep the count-and-replace model tight.

**4. Factor chrome math into the engine.** Any subtraction of
"viewport rows − chrome" that spans more than one backend belongs in
a `*ChromeDesc`-style struct that backends fill with native-to-row
conversions. Don't duplicate the arithmetic; backends should
**provide measurements, not formulas**.

**5. Declarative primitives don't own app state.** If you find
yourself wanting to add a `maximized: bool` to a quadraui primitive,
stop. The primitive's field is for *rendering only* (e.g. flipping
an icon); the state itself belongs on the app's `Engine`. See
`DECISIONS.md` D-001 principle: "One primitive per UX concept, not
per algebraic reduction."

**6. Primitive-owned state is only scroll + text input.** Everything
else — selection, focus, hover, expansion — belongs to the app or
is passed per-frame as a separate argument alongside the primitive
(see `A.6d` lesson in `PLAN.md`: hover state goes *alongside* the
primitive, not *in* it).

---

## New-feature checklist

Before wiring any non-trivial feature:

1. **Persistent state** — does it belong on `Engine` (transient) or
   `SessionState` (persisted to disk)? User preferences → session;
   transient flags (maximize, focus, which-panel-open) → engine.
2. **Effective / derived value** — will it be computed from viewport
   geometry each frame? Write an `effective_*` method on `Engine`
   that takes the backend-supplied measurement as an argument.
3. **Chrome math** — if the feature reserves rows / pixels for
   chrome, fill a `PanelChromeDesc`-style struct instead of
   open-coding arithmetic in each backend.
4. **Ex command** — add to `execute.rs` + a new `EngineAction`
   variant if the command needs backend context (window size, etc.).
5. **Keybinding** — add a field to `settings::PanelKeys` with a
   sensible default; wire each backend's key dispatcher.
6. **Toolbar / UI click target** — each backend's hit-test must use
   the *effective* value, not the raw stored value.
7. **Window-resize sensitive** — does the backend's resize handler
   propagate the new dimensions correctly? Check `connect_resize`
   (GTK), `WM_SIZE` (Win32), and the per-frame re-layout in TUI.
8. **PTY / subprocess dimension** — if the feature changes the
   visible size of a running subprocess (shell, LSP, DAP), the
   backend must call `terminal_resize` (or equivalent) in its
   resize handler.
9. **Primitive plumbing** — does the render primitive need a new
   field (e.g. `maximized: bool` on `TerminalPanel` for icon
   switching)? Add to `render.rs` and each backend's draw function.
10. **Tests** — unit-test the state machine; integration-test the
    helper (see `tests/terminal_maximize.rs`); add a lib-test for
    any new `*ChromeDesc` arithmetic (see `test_panel_chrome_*` in
    `engine/tests.rs`).
11. **Docs** — README key reference, PROJECT_STATE session note,
    and an `APP_ARCHITECTURE.md` worked-example entry when the
    feature touches ≥3 layers of the cake.

---

## References

- `quadraui/docs/UI_CRATE_DESIGN.md` — the vision (primitives +
  backend trait + layout primitives, most of it still roadmapped)
- `quadraui/docs/DECISIONS.md` — primitive-distinctness decisions
  and the "One primitive per UX concept" principle
- `docs/NATIVE_GUI_LESSONS.md` — backend-implementer counterpart to
  this file (geometry bugs, multi-group testing, hit-test parity)
- `PLAN.md` § "Lessons learned" — rolling log of cross-backend
  patterns discovered during Phase A (including render-time
  effective values and hit-test parity, added alongside this doc)
- Reference commits for the maximize feature:
  `45511d7` (initial ship), `5bcb1bd` (render-time refactor),
  `1d7141a` + `507d63a` (hit-test parity fixes)
