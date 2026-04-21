# Cross-Platform UI Crate — Design Sketch

**Status:** Draft for discussion. No code yet.
**Audience:** The author and collaborators deciding whether this direction is sound before committing to a refactor.

---

## 1. Vision

Extract a **cross-platform UI crate** from vimcode that lets developers build rich, fast, keyboard-driven desktop + terminal applications from a single codebase.

Targets:
- **Windows** — native via Direct2D + DirectWrite (reference implementation in vimcode's `src/win_gui/`)
- **Linux** — native via GTK4 + Cairo + Pango (reference implementation in vimcode's `src/gtk/`)
- **macOS** — native via Core Graphics + Core Text (planned)
- **TUI** — ratatui + crossterm (reference implementation in vimcode's `src/tui_main/`)

**Test app:** vimcode. It stresses the library: 20+ panels, complex text editing, drag-and-drop splits, tree views, forms, modals, command palette, dialogs.

**Ambition:** Someone should be able to write a SQL client, a k8s dashboard (Lens-alike), a log viewer, or a package manager UI on top of this crate and get all four backends for free. Apps should **feel native** (platform font, chrome, accent colour, menus, keybindings) without **being native** (no `gtk::TreeView`, no `NSOutlineView`, no Win32 Common Controls). The app writes once; the crate renders everywhere.

---

## 2. Non-goals

- **Not a general-purpose GUI framework** like GTK, Qt, Iced, Slint, or egui. We're biased toward keyboard-driven, productivity-class apps: IDEs, database tools, devops consoles. Apps with heavy custom drawing (games, video editors, Figma) are out of scope.
- **Not a web-tech shim** like Tauri or Electron. No WebView. No HTML/CSS. No JavaScript.
- **Not a pixel-perfect Figma-style renderer.** Widgets look *consistent* with each platform's conventions (font, accent, spacing, scrollbar style) but do not attempt to mimic `NSButton` or `ttk::Button` exactly.
- **Not retained-mode DOM.** No virtual DOM diffing. Full rebuild each frame — see §3.
- **Not an animation framework.** Transitions are out of scope v1. Fades and spinners can be added later as primitives.

---

## 3. Core architecture

### 3.1 Rendering model: build-and-draw each frame

Each frame:

1. App produces a **description tree** of the UI for the current state (pure function of app state).
2. Crate consumes the tree, walks it with the active backend, and renders.
3. Backend captures input (key, mouse) and returns a **list of events** to the app.
4. App mutates its state and loops.

This is the same loop vimcode already runs: `build_screen_layout()` is called every frame; backends just draw. We're generalizing it.

No diffing. No virtual DOM. No retained widget state managed by the crate. All state lives in the app; the crate is a pure view function with an input event channel.

**Why not retained-mode with diffing (React/Relm)?** Simpler mental model, fewer footguns (stale widget state, identity mismatches), faster to implement per-backend. vimcode already proves the rebuild-every-frame approach scales to dozens of panels.

**Exception — scroll and text input state.** Scroll offsets, cursor position in text inputs, and fuzzy-search query boxes are *primitive-owned*: the crate retains them to avoid forcing every app to thread them through. Primitives emit events when these change so the app can persist or react.

### 3.2 Event model

Each primitive has a typed event enum. Example:

```rust
pub enum TreeEvent {
    RowClicked { path: TreePath, modifiers: Modifiers },
    RowDoubleClicked { path: TreePath },
    RowExpanded { path: TreePath },
    RowCollapsed { path: TreePath },
    ContextMenuRequested { path: TreePath, screen_pos: Point },
    SelectionChanged { path: TreePath },
    KeyPressed { key: Key, handled: bool },
}
```

Backend produces events; the crate bubbles them up to the app wrapped in an `UiEvent` discriminator:

```rust
pub enum UiEvent {
    Tree(WidgetId, TreeEvent),
    Form(WidgetId, FormEvent),
    TabBar(WidgetId, TabEvent),
    StatusBar(WidgetId, StatusEvent),
    ActivityBar(ActivityEvent),
    Palette(PaletteEvent),
    Window(WindowEvent),
    // ...
}
```

`WidgetId` is a stable ID the app assigns when describing the tree. It's how the app routes events to the right handler.

### 3.3 Fully-drawn, not native-widget

Every widget is rendered by the backend's 2D drawing API. No `gtk::TreeView`. No `NSOutlineView`. No Win32 `EDIT`. Win-GUI has proven this works end-to-end: zero native widgets, everything rasterized via Direct2D + DirectWrite.

**Benefits:**
- Identical visual behaviour across platforms (no "it works on Windows, different on Linux")
- One code path for selection, scroll, keyboard nav — no per-platform widget API to wrangle
- Testable: ScreenLayout-style snapshots are platform-agnostic

**Costs (real, must be mitigated):**
- **Accessibility** — no free a11y tree. We must publish a parallel accessibility structure to UI Automation (Win), NSAccessibility (Mac), AT-SPI (Linux). This is a meaningful amount of work and is a v2 topic. Apps built on the crate that ignore a11y will not meet screen-reader expectations out of the box.
- **IME / Unicode composition** — Korean/Japanese/Chinese input requires platform IME integration. Each backend must implement IME client protocol on text-input primitives. Non-trivial.
- **Platform right-click menus, scroll gestures, kinetic scrolling** — we reimplement.
- **Drag-and-drop across apps** — platform DnD protocols need explicit handling per backend.

These costs are acceptable for v1 (accessibility parked until there's a real user need; IME is a v1.1 goal; DnD within the app only). They'd be dealbreakers for a mass-market app framework.

### 3.4 "Native feel" without native widgets

Each backend supplies a `PlatformStyle`:

- **Default font:** SF Pro Text (macOS), Segoe UI Variable (Windows 11), system sans-serif (Linux), terminal default (TUI)
- **Default mono font:** SF Mono, Cascadia Code, JetBrains Mono, terminal
- **Accent colour:** system accent on Windows/macOS; VSCode blue fallback on Linux/TUI
- **Scrollbar style:** auto-hide overlay on macOS/Win11; always-visible on Linux/TUI
- **Window chrome:** platform-provided (Win32 title bar, macOS unified title, GTK CSD)
- **Menu placement:** macOS global menu bar; in-window menu bar on Windows/Linux; `:command` mode in TUI
- **Modifier conventions:** `Cmd` on macOS is `Ctrl` on Win/Linux for common shortcuts. The crate exposes a `Accelerator` type that renders appropriately per platform.

Apps don't hardcode hex colours; they read from `theme.palette`. Apps don't hardcode "Ctrl+S"; they register `Accelerator::Save` which renders as `⌘S` on macOS and `Ctrl+S` elsewhere.

---

## 4. Primitive catalog

A primitive is a widget type the crate renders across all backends. Apps compose primitives into screens.

### 4.1 Layout & containers

| Primitive | Purpose |
|-----------|---------|
| `Window` | Top-level frame; holds menu bar, status bar, content. One per logical window. |
| `Split` | Recursive binary split with draggable divider. Horizontal or vertical. Ratio or fixed px. |
| `Tabs` | Container with tab strip + content area. Drag-to-reorder, drag-to-split, close buttons. |
| `Panel` | Fixed or resizable panel (sidebar, bottom drawer). Has header + body. |
| `Stack` | Vertical or horizontal stack with per-child sizing (fixed, fill, ratio). |
| `Modal` | Overlay with backdrop. Blocks input to parent. Used for dialogs. |

### 4.2 Navigation chrome

| Primitive | Purpose |
|-----------|---------|
| `MenuBar` | Top menu bar (in-window on Win/Linux, global on macOS). Declarative menu tree. |
| `ActivityBar` | Vertical icon strip (VSCode-style left rail). Selected state, click events, tooltips. |
| `TabBar` | Horizontal tab strip, usable standalone or inside `Tabs`. Hit regions pre-computed by crate. |
| `StatusBar` | Segmented bottom bar with clickable regions. Left/right segment lists. |
| `BreadcrumbBar` | Optional row above editor showing file path, symbols, etc. |

### 4.3 Content widgets

| Primitive | Purpose |
|-----------|---------|
| `TreeView` | Hierarchical rows, expand/collapse, icons, multi-selection, context menu. |
| `TreeTable` | Tree whose rows have multiple columns; leaves can be grid rows. Subsumes `DataTable` (depth-0 tree). Tracking: #139. |
| `ListView` | Flat rows with optional section headers, selection, context menu. |
| `DataTable` | Grid with sortable column headers, row selection, cell editing. Likely implemented as `TreeTable` with `max_depth=0` — decision tracked in #140. |
| `TextEditor` | Multi-cursor text editor with syntax highlighting, folds, gutter, virtual text. vimcode-class. Consumes a `BufferView` description; text engine stays in a separate crate. |
| `TextDisplay` | Read-only rich/plain text view; used for markdown preview, LSP hover, log output. Live-append for streaming tracked in #144. |
| `Terminal` | VT100-compatible terminal emulator. vimcode already ships one; Phase A extracts it as a primitive. |
| `Form` | Vertical stack of labeled fields: `Toggle`, `TextInput`, `Dropdown`, `Slider`, `Button`, `ColorPicker`. Richer field types tracked in #143. |
| `SearchBox` | Text input with embedded search affordances (clear button, match count, up/down). |
| `ProgressBar` | Determinate or indeterminate. Tracking: #142. |
| `Spinner` | Indeterminate activity indicator. Tracking: #142. |

### 4.4 Transient / popup

| Primitive | Purpose |
|-----------|---------|
| `Palette` | Fuzzy-search list, anchored at top of window. Command palette, file picker, symbol picker. |
| `ContextMenu` | Right-click menu, anchored at cursor. Nested submenus. |
| `Completions` | Autocomplete popup, anchored to a caret position. |
| `Tooltip` | Hover or focus-triggered info bubble. |
| `Dialog` | Built on `Modal`; standard button layouts (OK/Cancel, Yes/No/Cancel, error/warn/info). |
| `Toast` | Transient notification, anchored to corner. |

### 4.5 Primitive data shape (example)

```rust
pub struct TreeView<'a> {
    pub id: WidgetId,
    pub rows: &'a [TreeRow],           // flat, pre-expanded
    pub selection: SelectionMode,      // Single, Multi, None
    pub selected: &'a [TreePath],
    pub scroll: ScrollState,           // crate-owned; use ScrollState::id(widget_id)
    pub context_menu: Option<MenuDescription>,
    pub style: TreeStyle,              // indent_px, icon_size, row_height
}

pub struct TreeRow {
    pub path: TreePath,                // [0, 3, 1] style
    pub indent: u16,
    pub icon: Option<Icon>,
    pub text: StyledText,              // supports spans + colour
    pub badge: Option<Badge>,          // right-aligned indicator (git status, count, etc.)
    pub is_expanded: Option<bool>,     // None = leaf
    pub decoration: Option<Decoration>,// italic, strikethrough, muted
}
```

Apps build `TreeRow` lists from their own state each frame. Scroll is primitive-owned: `ScrollState::id(widget_id)` tells the crate "this is the same scrollable region across frames; retain my scroll offset."

---

## 5. Sample app sketches

Purpose of this section: **stress-test the primitive set** against three concrete apps to check for gaps.

### 5.1 vimcode (current)

| Screen area | Primitive composition |
|-------------|----------------------|
| Main window | `Window { menu_bar, content, status_bar }` |
| Sidebar | `Panel` containing `ActivityBar` + body (swaps between `TreeView` for explorer, `TreeView` for git/debug/extensions, `Form` for settings, etc.) |
| Editor area | `Tabs` with `TextEditor` inside each tab; supports `Split` containers recursively |
| Status bar | `StatusBar { left_segments, right_segments }` |
| Find/replace | `Panel` (overlay) containing `SearchBox` + `Form` |
| Command palette | `Palette` |
| Dialogs | `Dialog` (modal) |
| Completions | `Completions` anchored to `TextEditor` caret |
| Hover | `Tooltip` anchored to `TextEditor` caret |

**Coverage check:** ✅ All current vimcode UI is expressible. The settings panel becomes a `Form`; the explorer becomes a `TreeView`; the source-control panel is a `TreeView` with custom `TreeRow` badges for staged/unstaged state.

### 5.2 SQL client (pgcli / DBeaver-class)

| Screen area | Primitive composition |
|-------------|----------------------|
| Left sidebar | `Panel` with `TreeView` of connections → databases → schemas → tables → columns |
| Main area | `Tabs` — each tab is a query + results. Inside: `Split(vertical) { TextEditor (SQL), DataTable (rows) }` |
| Results view | `DataTable` with sortable columns, cell editing, paging |
| Connection dialog | `Dialog` containing `Form` (host, port, user, password, database) |
| Query history | `Panel` with `ListView` |
| Status bar | `StatusBar` showing connection name, rows returned, query time |
| Command palette | `Palette` for "New connection", "Export results", etc. |

**Gaps found:**
- `DataTable` is a new primitive (not needed by vimcode) with its own complex event surface (sort clicked, cell edited, row selected, column resized). **Must design now**, don't defer — it's essential for any data-oriented app.
- **Column resize** is a new interaction pattern (drag the column separator). Probably a general pattern: divider drags exist in `Split` and `Tabs` too. Worth a shared `Divider` primitive.
- **Multi-select rows with shift-click / ctrl-click** — needs to be a first-class mode in `ListView` and `DataTable`.
- **Export to CSV / long-running tasks** — suggests a `ProgressBar` primitive with cancel, and a toast/notification surface.

### 5.3 k8s dashboard (Lens-class)

| Screen area | Primitive composition |
|-------------|----------------------|
| Left sidebar | `Panel` with `TreeView` of clusters → namespaces → resource kinds (pods, services, deployments…) |
| Main area | Depends on selection: `DataTable` (resource list) OR `Split { DataTable, TextDisplay (YAML/logs) }` |
| Detail pane | `Tabs` with Overview/YAML/Events/Logs/Shell — each is a different primitive |
| Shell tab | `TextEditor` in terminal mode (streaming output, input at bottom) — stretches `TextEditor` |
| Logs tab | `TextDisplay` with live-tail and search |
| Edit YAML | `TextEditor` with YAML syntax highlighting |
| Apply/delete confirms | `Dialog` |
| Cluster switcher | `Palette` |

**Gaps found:**
- **Live-streaming content** — logs and `kubectl exec` output arrive asynchronously. The crate needs a primitive or pattern for "content appended to `TextDisplay`/`TextEditor` from a channel" without the app rebuilding the whole buffer each frame. Probably: `TextDisplay` has an append API the app calls when new data arrives, and only that slice invalidates.
- **Terminal emulation (VT100)** — `Terminal` is a v1 core primitive (§4.3). vimcode's existing `terminal_*` methods provide the reference implementation; extraction is Phase A work.
- **Status indicators with colour** — pods have states (Running/Pending/Failed). `ListView`/`DataTable` cells need rich content (coloured dot + text). `StyledText` + per-cell rendering handles this.
- **Custom keyboard shortcuts per-panel** — `l` opens logs, `e` opens edit, etc. The crate must support per-widget keybinding tables, not just global accelerators.

### 5.4 Summary of primitive gaps vs vimcode-only

Building just for vimcode we'd get away with ~12 primitives. To support SQL and k8s apps too, add:

- `TreeTable` (tracking #139) — subsumes `DataTable` as a depth-0 case (#140)
- `Divider` (shared drag semantics across `Split` / `Tabs` / column resize)
- Live-append support on `TextDisplay` / `TextEditor` (tracking #144)
- `Toast` + long-running task abstraction with cancel (tracking #141, #142)

`Terminal` is already a v1 primitive (vimcode ships one). None of these are surprises. None invalidate the retained-tree + events model.

---

## 6. Backend responsibilities

Each backend implements the same trait surface. Very rough sketch:

```rust
pub trait Backend {
    fn begin_frame(&mut self, viewport: Rect);
    fn end_frame(&mut self) -> Vec<UiEvent>;

    // Primitives
    fn draw_tree(&mut self, tree: &TreeView);
    fn draw_list(&mut self, list: &ListView);
    fn draw_table(&mut self, table: &DataTable);
    fn draw_text_editor(&mut self, ed: &TextEditor);
    fn draw_text_display(&mut self, td: &TextDisplay);
    fn draw_form(&mut self, form: &Form);
    fn draw_tab_bar(&mut self, tb: &TabBar);
    fn draw_status_bar(&mut self, sb: &StatusBar);
    fn draw_activity_bar(&mut self, ab: &ActivityBar);
    fn draw_menu_bar(&mut self, mb: &MenuBar);
    fn draw_palette(&mut self, p: &Palette);
    fn draw_dialog(&mut self, d: &Dialog);
    fn draw_context_menu(&mut self, cm: &ContextMenu);
    fn draw_completions(&mut self, c: &Completions);
    fn draw_tooltip(&mut self, t: &Tooltip);
    fn draw_toast(&mut self, t: &Toast);
    fn draw_spinner(&mut self, s: &Spinner);
    fn draw_progress(&mut self, p: &ProgressBar);

    // Platform services
    fn open_file_dialog(&mut self, opts: FileDialogOptions) -> Option<PathBuf>;
    fn clipboard(&self) -> &dyn Clipboard;
    fn send_notification(&mut self, n: Notification);
}
```

Each primitive's `draw_*` method is responsible for:
- Rendering from the primitive's description
- Producing events from input captured while that primitive was under the pointer / focused
- Retaining primitive-owned state (scroll offset, caret) across frames keyed by `WidgetId`

**What the backend never does:**
- Know about app concepts (no `draw_debug_sidebar`, no `draw_source_control`)
- Retain app state
- Make layout decisions beyond the primitive's own intrinsic sizing
- Hardcode widget colours (always via `PlatformStyle` + `theme.palette`)

---

## 7. Key design decisions

All 13 decisions below were resolved in the 2026-04-18 design session (either explicitly or by accepting the default recommendation).

1. ✅ **Retained-tree vs immediate-mode API.** Retained tree, built each frame. Matches existing `build_screen_layout`.
2. ✅ **Primitive-owned vs app-owned scroll state.** Primitive-owned keyed by `WidgetId`. Exposed via events for the scroll-binding case.
3. ✅ **One `Backend` trait vs separate traits per primitive.** One trait, to keep the contract in one file. Primitives with complex state may get helper traits.
4. ✅ **How does a `TextEditor` primitive consume vimcode's Engine?** The `TextEditor` primitive consumes a `BufferView` description that any app can produce. vimcode's engine becomes an adapter that produces `BufferView` per frame. The text engine (rope, tree-sitter, LSP) stays in a separate crate — not part of quadraui.
5. ✅ **Do we support multiple `Window`s per process?** Yes, v1. Required for detaching tabs, dialogs on multi-monitor, and cross-platform parity. TUI backend collapses to one Window.
6. ✅ **Accessibility.** v1 ships with a11y-ready data fields on every primitive (`a11y_role`, `a11y_label`, focus order). Platform wiring (UI Automation, NSAccessibility, AT-SPI) lands in v1.1. This keeps the door open without blocking v1.
7. ✅ **IME / composition.** v1.1. Ship without full IME; add when first non-Latin user complains. Text input primitives must at minimum not crash.
8. ✅ **Theming system.** Palette-based `Theme` struct with derived colours, like vimcode's existing one. Exposed; apps can override. VSCode theme JSON importer.
9. ✅ **Native menu bars.** v1. macOS uses global menu; Win/Linux uses in-window. Crate owns the platform integration.
10. ✅ **Packaging.** Single crate `quadraui` with backends behind Cargo features: `gtk`, `tui`, `win-gui`, `cocoa` (v1.1). Apps pick.
11. ✅ **Naming.** `quadraui` — evokes the four backends, available on crates.io as of 2026-04-18. Working name; will be confirmed at Phase B extraction.
12. ✅ **Language / toolchain.** Rust 2021, MSRV tracks latest stable −2. No C/C++ except vendored trees like tree-sitter. mlua for plugin support.
13. ✅ **License.** MIT + Apache-2.0, standard Rust dual. vimcode's current license prevails.

**Additional decisions from the same session:**

- **Workspace layout.** `quadraui/` is a workspace member of the vimcode repo from day one. `vimcode` depends via path. Extraction to a standalone repo is a Phase B activity. See §8.
- **Branching.** Stage-by-stage PRs to `develop` — no long-lived refactor branch. Each Phase A stage adds primitives alongside existing code (coexistence rule) so nothing ever ships half-migrated.
- **macOS timing.** quadraui 1.0 ships Win+Linux+TUI; macOS lands in 1.x. Don't block v1 on a new backend.
- **Validation app.** A cross-platform k8s dashboard (tracking issue #145) is the planned second consumer that proves extraction. Phase D work.
- **Postman-like HTTP client** will be a bundled vimcode extension (tracking #147), not a standalone consumer. Requires a plugin API extension (#146) to expose quadraui primitives to Lua.

---

## 8. Migration path from vimcode

Gradual, in-place. No big-bang rewrite.

**Phase A — primitives live alongside `ScreenLayout` (months 1–2)**
Phase A.0 scaffolds the `quadraui/` workspace member (empty crate, path dependency from vimcode). Each subsequent stage (A.1 through A.9 — see §11 for the full table) adds one primitive and migrates one vimcode panel to consume it. Each stage is a short-lived branch + normal PR to `develop` — no long-lived refactor branch, no big-bang rewrite.

**Phase B — extract the crate (month 3)**
The `quadraui/` workspace member exists from Phase A.0. Phase B is the point where we promote it from "internal vimcode helper" to publishable library: stabilize the API, add a real README + examples, publish a pre-release to crates.io. Since workspace membership is in place from the start, there's no "move files" step — just API hardening.

**Phase C — macOS backend (v1.x, issue #47)**
Implement the `Backend` trait for macOS (Core Graphics + Core Text). Deferred out of v1 per §7 decision #6 / §7 macOS timing note — v1 ships Win+Linux+TUI, macOS lands in 1.x. `NATIVE_GUI_LESSONS.md` captures cross-backend pitfalls that should transfer.

**Phase D — polish + v1 release (month 5)**
Stabilize the API. Build the cross-platform k8s dashboard (#145) as the second consumer to prove extraction works. Document public API. Publish to crates.io.

**Phase E — mature (months 6+)**
Platform a11y wiring (v1.1). IME / composition (v1.1). macOS backend (Phase C deferred to v1.x). Animation primitives. Community examples.

This timeline is aggressive but achievable given how much of the work is already done in vimcode. The unknown unknowns are `TreeTable` (#139), live-streaming content (#144), and the k8s app's uncovered design pressure.

---

## 9. Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Primitive set doesn't cover real-world apps and requires frequent breaking changes post-v1 | Medium | Stress-test with SQL and k8s sketches *before* extraction (§5). Build one non-vimcode example app in Phase D. |
| Accessibility debt blocks adoption | Low (for target users) | v1 ships with a11y-ready data fields on every primitive; platform wiring (UIA / NSAccessibility / AT-SPI) lands in v1.1 once a concrete user need surfaces. |
| Fully-drawn approach feels "off" on macOS vs native Cocoa apps | Medium | Careful `PlatformStyle` work. Use platform font/accent religiously. Hide window chrome behind native title bar. |
| macOS backend blows out timeline due to CG/CT learning curve | Medium-High | Budget extra time. `NATIVE_GUI_LESSONS.md` from Win-GUI should transfer. |
| Text editor primitive too coupled to vimcode's Engine to extract | Medium | `BufferView` adapter pattern (§7.4) separates concerns. Validate during Phase A. |
| Performance regressions from rebuilding description tree every frame on complex UIs | Low | vimcode already does this. Profile before optimising. Add primitive-level dirty bits only if measured. |
| Scope creep — "one more primitive" forever | High | Freeze v1 primitive set after Phase A. Everything else is v1.1+. |

---

## 10. Design invariants for plugin-driven UI

Properties that every primitive's API **must** preserve so vimcode's Lua plugin system can later declare UI using quadraui primitives (tracking issue #146, #147). Violating any of these would force a breaking change to quadraui when we add plugin-driven UI.

1. **`WidgetId` is owned / allocatable** (e.g. `String` or `Cow<'static, str>`) — not `&'static str`. Plugins generate IDs at runtime.
2. **Events dispatched as plain data**, not Rust closures. Backend emits `UiEvent`; app routes to plugin by widget ID. No closures crossing the Rust/Lua boundary.
3. **Primitive data structs are serde-compatible** (`Serialize` + `Deserialize`). Lua tables convert via JSON to Rust structs.
4. **WidgetId namespace support.** Apps can reserve an ID prefix per plugin (e.g. `"plugin:my-ext:send"`) so plugin IDs don't collide with core IDs.
5. **No global event handlers.** Every widget has an ID; every event references it. No magic "on_change" that doesn't carry the widget.
6. **Primitives don't borrow from app state** (or use clear `'a` lifetimes). Plugins can pass owned data without borrow-checker puzzles.

These are already the design's intent. This section exists so a future refactor doesn't drift from them without noticing.

---

## 11. What happens next

**Resolved decisions recap:** The 2026-04-18 design session closed all open questions (see §7). Milestone **"Cross-Platform UI Crate"** on GitHub tracks the work.

**Immediate next step — release vimcode 0.10.0.** Cut a minor release from the current develop so quadraui work starts from a stable baseline. PR develop → main, tag `v0.10.0`.

**Then Phase A.0 — workspace scaffold.** Single PR that adds an empty `quadraui/` workspace member with a placeholder `lib.rs`. vimcode adds it as a path dependency. No primitives yet. Compile passes. This is the smallest possible first PR and unblocks all subsequent stages.

**Phase A.1 onward — primitives one at a time.** Each stage is its own PR:

| Stage | Primitive | vimcode panel to migrate |
|-------|-----------|--------------------------|
| A.1 | `TreeView` | Source-control panel |
| A.2 | `TreeView` | Explorer (replaces native `gtk4::TreeView`) |
| A.3 | `Form` | Settings panel |
| A.4 | `Palette` | Command palette (formalize existing pattern) |
| A.5 | `ListView` | Quickfix, git status list |
| A.6 | `StatusBar` / `TabBar` / `ActivityBar` | Complete issue #133 |
| A.7 | `Terminal` | Extract from vimcode's `terminal_*` methods |
| A.8 | `TextDisplay` | Markdown preview, LSP hover |
| A.9 | `TextEditor` + `BufferView` adapter | Main editor (biggest stage) |

**If this sketch is wrong, easiest places it's wrong:**
- The retained-tree model is insufficient for `TextEditor` (text editing + IME may need finer-grained incremental updates than a rebuild-every-frame description tree supports)
- The primitive set is too coarse — validation app (#145) surfaces something not on the list
- The "fully drawn, no native widgets" call is wrong on macOS specifically (Cocoa conventions run deeper than Win11 and Linux; decision deferred since macOS is v1.x)

Those are the three places to kick the tyres hardest as Phase A work proceeds.
