# `Backend` trait + `UiEvent` dispatch — proposal

**Status:** Draft. Blocks #169 (Postman-class validation app).
**Date:** 2026-04-21.
**Audience:** Decision-makers evaluating whether the design is right, and
implementers if it is.

---

## TL;DR

Phase A shipped a declarative **primitive catalog** (Tree, Form, List,
Palette, StatusBar, TabBar, ActivityBar, Terminal, TextDisplay). It
works. But every non-trivial feature (terminal maximize, VSCode mode,
find/replace, multi-cursor) ends up duplicating **event routing**,
**keybinding**, and **resize handling** across three backends because
the crate never owned those — each backend reads its native event
stream and dispatches directly into the app's `Engine`.

For vimcode this is tolerable (Claude can re-grep every backend on
every feature). For the Postman-class app (#169) and every future
consumer, it's a wall: a dev would need to understand GTK key
controllers AND Win32 message pumps AND crossterm event polling AND
Cocoa responder chain before shipping anything.

This proposal introduces two abstractions:

1. **`UiEvent`** — a backend-neutral event enum. Everything a user
   can do (press a key, click a widget, resize the window, drop a
   file) surfaces as one variant. Primitives emit their own events
   as sub-variants (`UiEvent::Tree(id, TreeEvent::RowClicked { … })`).

2. **`Backend` trait** — one method to poll events, one to draw, one
   to expose platform services. Each backend (`quadraui_tui`,
   `quadraui_gtk`, `quadraui_win`, future `quadraui_macos`) becomes an
   impl of this trait. The app's main loop is `for ev in backend.poll_events()
   { engine.handle(ev); }` — platform-independent.

The value is **quadratic**: today every (feature × backend) pair is
its own bespoke wiring. With the trait, every feature is wired once
against `UiEvent` and every backend translates its native events to
`UiEvent` once. New features add linearly; new backends add linearly;
the cross-product stops.

Migration is **gradual and coexists with the current dispatch** — no
big-bang rewrite. Details in §5.

---

## 1. Why now

Three concrete pieces of evidence:

**1a. The terminal-maximize wave (#34).** A single feature took 11
commits, spread across 10 files, introduced 61 references to helper
functions, and revealed two bug classes:

- Per-backend hit-test parity (fixed in `1d7141a`, `507d63a`)
- Per-backend resize handling differences (fixed in `187a7c6`)

Both bugs were structural: the app owns state, but *every backend
re-implements the plumbing to route events into that state*. See
`quadraui/docs/APP_ARCHITECTURE.md` §"Worked example — terminal
maximize" for the trace.

**1b. The #169 Postman app can't ship without it.** A typical
Postman-class feature (say, "Ctrl+Enter sends the current request")
would today require: define a `PanelKeys` field, add a matcher in
TUI's event loop, add a GTK key-controller check, add a Win-GUI
`on_key_down` arm, handle the action in three action-dispatch
branches. Five files per feature. Hundred-feature apps need a better
abstraction.

**1c. macOS is coming.** Phase C (issue #47) will add a fourth
backend. Without `Backend` trait + `UiEvent`, the macOS impl
duplicates all the dispatch code from the other three — a known
pain point per `docs/NATIVE_GUI_LESSONS.md` and PLAN.md lessons.

---

## 2. `UiEvent` — what a user did

```rust
pub enum UiEvent {
    // ── Input ───────────────────────────────────────────────────
    /// A declared accelerator fired. See §3 for Accelerator.
    Accelerator(AcceleratorId, Modifiers),

    /// A raw key was pressed (for text input primitives that want
    /// every keystroke, not just accelerators).
    KeyPressed { key: Key, modifiers: Modifiers, repeat: bool },

    /// A character was typed (already IME-composed, ready for insertion).
    CharTyped(char),

    // ── Mouse ───────────────────────────────────────────────────
    /// Mouse button pressed over a widget (or the background if none).
    MouseDown {
        widget: Option<WidgetId>,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
    },
    MouseUp { widget: Option<WidgetId>, button: MouseButton, position: Point },
    MouseMoved { position: Point, buttons: ButtonMask },
    MouseEntered { widget: WidgetId },
    MouseLeft { widget: WidgetId },
    DoubleClick { widget: Option<WidgetId>, position: Point },
    Scroll { widget: Option<WidgetId>, delta: ScrollDelta, position: Point },

    // ── Window ──────────────────────────────────────────────────
    WindowResized { viewport: Viewport },
    WindowClose,
    WindowFocused(bool),
    DpiChanged(f32),

    // ── Files (drop / paste / etc.) ─────────────────────────────
    FilesDropped { paths: Vec<PathBuf>, position: Point },
    ClipboardPaste(String),

    // ── Primitive-specific events bubble up by WidgetId ─────────
    Tree(WidgetId, TreeEvent),
    List(WidgetId, ListEvent),
    Form(WidgetId, FormEvent),
    Palette(WidgetId, PaletteEvent),
    TabBar(WidgetId, TabBarEvent),
    StatusBar(WidgetId, StatusBarEvent),
    ActivityBar(WidgetId, ActivityBarEvent),
    Terminal(WidgetId, TerminalEvent),
    TextDisplay(WidgetId, TextDisplayEvent),

    // ── Escape hatches ──────────────────────────────────────────
    /// Backend-specific event the crate couldn't normalise. Apps
    /// ignore unless they want to special-case a platform.
    BackendNative(BackendNativeEvent),
}
```

**Invariants:**

- `UiEvent` is `Debug + Clone` (for logging, replay, testing).
- **No closures, no lifetimes beyond `'static`** — identical discipline
  as primitive data. A plugin can receive a `UiEvent` over JSON.
- Mouse events carry `Option<WidgetId>` because the backend does
  hit-testing **before** emitting — so apps can dispatch on widget
  identity, not screen coordinates.
- `Modifiers` is the existing `quadraui::types::Modifiers`.

---

## 3. `Accelerator` — declarative cross-platform keybindings

```rust
/// A named keybinding. Apps register these; backends translate
/// platform-native key events to `UiEvent::Accelerator`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accelerator {
    pub id: AcceleratorId,  // String — the name the app will match on
    pub binding: KeyBinding, // Vim-style: `<C-S-t>`, `<A-f>`, etc.
    pub scope: AcceleratorScope,
    pub label: Option<String>, // For menu rendering and help text
}

pub enum AcceleratorScope {
    /// Fires regardless of what's focused. "Ctrl+P" for the palette.
    Global,
    /// Fires only when a specific widget or widget family is focused.
    Widget(WidgetId),
    /// Fires only when a specific mode is active (Normal / Insert / Visual for vim).
    Mode(String),
}

pub enum KeyBinding {
    /// Platform-appropriate rendering: `⌘S` on macOS, `Ctrl+S` elsewhere.
    Save,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    Find,
    Quit,
    // ...the universal ones — renders natively per platform

    /// Literal, no platform substitution. Used for app-specific bindings.
    Literal(String), // Vim-style, e.g. `<C-S-t>`
}
```

**Why an `Accelerator` type, not raw key-binding strings?**

- **Platform idiom parity.** `⌘S` on macOS renders as `Ctrl+S` on
  Win/Linux **without** the app caring. Menus, tooltips, palette
  entries can all show the correct modifier glyph.
- **Scope enforcement.** `Ctrl+P` in a text input shouldn't open the
  palette; backends respect the scope.
- **Mode dispatch.** Vim modes (Normal/Insert/Visual) are just
  `AcceleratorScope::Mode`. No more `handle_vscode_key` vs
  `handle_normal_key` branching — the backend picks the right scope.

Registration API (draft):

```rust
impl<B: Backend> App<B> {
    pub fn register_accelerator(&mut self, acc: Accelerator);
    pub fn unregister_accelerator(&mut self, id: &AcceleratorId);
}
```

---

## 4. `Backend` trait — one impl per platform

```rust
pub trait Backend {
    /// Viewport geometry in native units. Caller converts to rows
    /// via `viewport.rows_at(line_height)` etc.
    fn viewport(&self) -> Viewport;

    /// Poll all queued native events. Returns a fully-translated
    /// `Vec<UiEvent>` ready for app dispatch. Never blocks.
    fn poll_events(&mut self) -> Vec<UiEvent>;

    /// Block for up to `timeout` waiting for at least one event.
    /// Returns empty vec on timeout. Used by apps that don't want
    /// to busy-poll.
    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent>;

    /// Begin a frame. Backends may set up render target, clear, etc.
    fn begin_frame(&mut self);

    /// Draw a primitive or layout within a given rect. The `Rect`
    /// comes from the app's layout pass — backend just rasterises.
    fn draw_primitive(&mut self, rect: Rect, primitive: &AnyPrimitive);

    /// Flush to screen.
    fn end_frame(&mut self);

    /// Platform services (clipboard, file dialog, notifications, …).
    fn services(&self) -> &dyn PlatformServices;

    /// Register an accelerator with the backend's native keybinding
    /// system. Backend stores it and emits `UiEvent::Accelerator`
    /// when matched.
    fn register_accelerator(&mut self, acc: &Accelerator);
}

pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;
    fn send_notification(&self, n: Notification);
    fn open_url(&self, url: &str);
    fn platform_name(&self) -> &'static str;
}
```

**`AnyPrimitive`** is an enum wrapping every primitive type so a single
`draw_primitive` can dispatch. Alternative: per-primitive methods
(`draw_tree`, `draw_list`, …). Per-primitive is more verbose but gives
better type safety; `AnyPrimitive` is cleaner for plugin-declared UI.
**Decision deferred to implementation** — we'll try `AnyPrimitive`
first; if the match-dispatch becomes a pain point, split.

---

## 5. Migration — gradual, coexist-first

**Non-negotiable:** this must not break vimcode or block unrelated
feature work. Every stage is a short-lived branch + PR to `develop`.

### Phase B.1 — `UiEvent` enum + `Backend::poll_events` alongside existing dispatch

Add the types in `quadraui/src/` but don't force anyone to use them.
Each backend implements `poll_events` returning a translation of its
existing native events. Vimcode's `Engine` grows a single
`handle_ui_event(UiEvent)` entry point that dispatches to existing
methods — thin wrapper, no behaviour change.

At the end of B.1: both dispatch paths work. Vimcode's main loops in
`tui_main/mod.rs` / `gtk/mod.rs` / `win_gui/mod.rs` can optionally
call `backend.poll_events()` and route through `handle_ui_event`, or
keep the existing native-event paths. **Coexistence, not switch.**

### Phase B.2 — `Accelerator` type + one pilot feature

Ship `Accelerator` + `register_accelerator` on `Backend`. Pick one
vimcode feature — **terminal maximize is the obvious candidate** —
and migrate its keybinding from the per-backend `matches_*_key`
checks to an `Accelerator`. All three backends translate their
native event → `UiEvent::Accelerator("toggle_terminal_maximize", …)`.
Vimcode handles it once.

**Success criterion:** 3 backends' worth of keybinding plumbing for
maximize collapses to 1 call to `register_accelerator` + 1 match arm
on `UiEvent::Accelerator`. Measure LOC delta.

### Phase B.3 — Layout primitives (`Panel`, `Split`, `Tabs`, `Stack`, `MenuBar`, `Modal`, `Dialog`)

Ship the §4.1 primitives from `UI_CRATE_DESIGN.md`. These are the
ones the Postman-class app (#169) actually needs. Each primitive
declares its own hit-regions; `draw_primitive` stores them; the
backend's `poll_events` reads them for click dispatch. `UiEvent`
grows new primitive-specific variants.

**The terminal maximize scrollbar-on-top bug (#167)** would be
naturally fixed here: `Panel` owns z-order; `Maximized` is a property
of the panel that hides everything behind it, including scrollbars,
without each backend needing to suppress them individually.

### Phase B.4 — Migrate vimcode subsystems

Once the trait is proven on one feature (B.2) and the layout
primitives exist (B.3), start migrating vimcode subsystems one at
a time. Candidates roughly in order of pain-to-benefit ratio:

1. Terminal maximize + sidebar visibility + menu bar toggle
   (stateful-chrome cluster)
2. Editor group splits (`Split` primitive consumer)
3. All modals and dialogs (`Dialog` primitive)
4. Status bar click targets (already using `StatusAction`; adapt)
5. Command palette (already the closest; adapt)
6. Text editor (Phase A.9, un-deferred — biggest, last)

Each migration is its own PR. The old dispatch code stays until the
migration is complete; removal of native event handlers is the final
PR per subsystem.

### Phase B.5 — Postman-class app (#169) starts

With the trait + enough primitives in place, scaffold `postman-clone/`
as a workspace member and start shipping screens. This is when the
abstraction proves itself.

### Phase B.6 — Remove vestigial per-backend dispatch

Once every vimcode subsystem uses `UiEvent`, the native-event paths
in TUI/GTK/Win-GUI can be deleted. At this point the three backend
files are uniformly thin: `Backend` trait impl + `draw_primitive` +
`poll_events` + services.

---

## 6. Open questions

### 6.1 `AnyPrimitive` vs per-primitive trait methods

`fn draw_primitive(&mut self, rect: Rect, p: &AnyPrimitive)` is
cleaner for plugin UI but loses static dispatch. Alternative is
`fn draw_tree`, `fn draw_list`, … with a default impl that panics
("not implemented") so backends opt in. **Defer to implementation
— try `AnyPrimitive` first.**

### 6.2 Where does layout computation live?

Today `build_screen_layout` in vimcode's `render.rs` is the layout
pass. Under the new model, that's ambiguous: is layout an app
concern or a quadraui concern? Proposal: layout is a **pure function
in quadraui** (`quadraui::layout::compute(root: &Layout, viewport)
-> LayoutResult`), and apps declare their layout description. Apps
can still do custom layout for complex cases — the primitive layout
is just a convenience.

### 6.3 What about multi-window?

§7 decision #5 says v1 supports multiple windows. The trait draft
above is single-window. Extension: `Backend` becomes `BackendWindow`
and there's an outer `BackendManager` that owns N windows. Not
blocking for Phase B.1-B.3 — vimcode and Postman clone are
single-window.

### 6.4 How do we handle focus?

Focus-within semantics (which widget gets keyboard input) is its own
sub-design. Backends know native focus; primitives declare
focusability; apps manage focus transitions. Worth its own
proposal doc before Phase B.3.

### 6.5 Text input / IME

§7 decision #7 parks IME for v1.1. Text input primitives need to
plumb composition events through `UiEvent` — probably
`UiEvent::TextComposing { candidate, position }` plus the existing
`CharTyped`. Defer until first non-Latin user or the text editor
primitive lands.

### 6.6 Performance

Vimcode currently rebuilds primitives every frame. Most primitive
data is cheap (indices + small strings); the expensive one is
`Terminal` (3600+ cells). Already measured as OK. Don't
pre-optimise; profile after B.5 if necessary.

---

## 7. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| `UiEvent` enum bloats with one variant per primitive-event-type | Medium | Accept. The alternative (trait objects, closures) violates plugin invariants. |
| Native event translation is lossy — some app wants a platform-specific gesture | Low | `UiEvent::BackendNative(…)` escape hatch. Apps opt in to platform branches when needed. |
| Migrations get stuck half-done; both dispatch paths linger forever | Medium | Single-session-per-subsystem rule. Full migration or revert. PLAN.md tracks in-flight. |
| The trait is wrong and Phase B.5 discovers it mid-Postman-build | Medium-High | Phase B.2's pilot feature (terminal maximize migration) is the early-warning system. If the collapsed LOC isn't compelling, iterate on the trait before further migration. |
| Multiple consumers (vimcode + Postman) force conflicting API shapes | Low | Both apps are ours; we control the iteration. Community consumers come post-Phase-C. |
| Backend trait ossifies too early (pre-macOS) | Medium | Phase C (macOS) lands last and may force changes. That's OK — nothing is stabilised for crates.io until Phase D. |

---

## 8. What this proposal is NOT

- **Not a full framework.** quadraui remains a primitive catalog + a
  thin event/layout abstraction. Not Electron, not Qt, not egui.
- **Not a replacement for the render adapter.** Apps still have
  their own `render_adapter(engine, theme, viewport) -> ScreenLayout`.
  The trait adds an event side, not a state side.
- **Not a virtual DOM.** Primitives are still built every frame.
  Diffing is not on the table.
- **Not retained widget state.** Scroll offsets and text-input
  state remain the only primitive-owned state; everything else
  stays on the app (see `DECISIONS.md` D-001 principle).

---

## 9. Decisions needed before code starts

1. **Confirm the overall shape** — does this capture what we want?
2. **`AnyPrimitive` vs per-method draw?** (§6.1)
3. **Scope of Phase B.1** — is "ship UiEvent + poll_events alongside
   existing code" the smallest viable first PR? Or should we pair
   it with B.2's pilot feature?
4. **Pilot feature for B.2** — terminal maximize is the current
   candidate (painful, contained, measurable). Any reason to pick a
   different one?
5. **`Accelerator::Literal(String)` format** — stick with Vim-style
   `<C-S-t>` or switch to a platform-agnostic `"Ctrl+Shift+T"`? Vim
   style is already used throughout vimcode; carries over cleanly.

Once these are answered, the first PR is small (~200 LOC — types +
empty trait impls returning empty event vecs) and unblocks B.2.

---

## 10. References

- `quadraui/docs/UI_CRATE_DESIGN.md` §6 ("Backend responsibilities")
  — the original sketch this proposal expands
- `quadraui/docs/APP_ARCHITECTURE.md` — layer cake + worked example
  that motivated this
- `docs/NATIVE_GUI_LESSONS.md` — cross-backend pitfalls the trait
  should absorb
- `PLAN.md` §"Lessons learned" — maximize-era lessons
- Issue #169 — Postman-class validation app (depends on this)
- Issues #47 (macOS backend), #139 (TreeTable), #143 (Form fields),
  #146 (plugin UI API), #147 (bundled Postman extension — likely
  subsumed by #169)
