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

**Trait bounds** — `UiEvent` must implement:

- `Debug` — dev-facing formatting for panic messages and log output
- `Clone` — explicit copy when an app wants to preserve the event
  beyond its single-handler lifetime (e.g. recording for replay)
- `PartialEq` — equality checks in tests (`assert_eq!(got, expected)`)
- `Serialize + Deserialize` — JSON / IPC / replay-file round-trip,
  required for the Lua plugin boundary

`Send` is implied by the "no lifetimes beyond `'static`" rule below, so
events can be moved to another thread. `Sync` is **not** required —
events flow one-way through the dispatch loop; there's no cross-thread
sharing of a single event. `Eq` is also **not** required because
nested value types may include floats (which don't implement `Eq`
because of NaN); `PartialEq` is strictly weaker and sufficient for
the testing use case.

**Data shape:**

- **No closures, no lifetimes beyond `'static`** — identical discipline
  as primitive data (§10 of `UI_CRATE_DESIGN.md`). A `UiEvent` is
  inert, owned data; it never borrows from its producer, never carries
  a callback, never depends on the backend's internal state. This is
  what makes it serialisable, thread-transferable, and plugin-friendly.
- Mouse events carry `Option<WidgetId>` because the backend does
  hit-testing **before** emitting — apps dispatch on widget identity,
  not screen coordinates. `None` means the click landed outside any
  declared widget (e.g. on the editor content area or background).
- `Modifiers` is the existing `quadraui::types::Modifiers` — no
  per-event-type fork.

**Lifecycle — events are discarded by default:**

The trait bounds above enable *optional* preservation; they do not
mandate it. The normal path is:

```rust
for ev in backend.poll_events() {
    handle(&mut engine, ev);  // ev is consumed, then dropped
}
// at end of loop every event is gone — Rust's deterministic drop
```

Events are moved into `handle`; when `handle` returns they fall out
of scope and their memory is freed immediately. No GC, no log write,
no clone, no allocation beyond the `Vec` returned by `poll_events`.

An app that wants to preserve events (recording for replay, logging
for debugging, forwarding to another thread) must **explicitly** do
so — typically via `.clone()` before dispatch, or by pushing a
serialised copy onto a recorder. The invariants guarantee that every
such preservation path *can* work; they do not automatically do it.
This matters for performance (hot-path event dispatch allocates
nothing by default) and for reasoning about side effects (no hidden
retention or accidental aliasing).

**Event routing — hit-test vs focus:**

A clean boundary apps rely on: **mouse events route by hit-test at
cursor position; keyboard events route by focus.** The dispatcher
never conflates them.

| Event class | Routed by | Why |
|---|---|---|
| `MouseDown` / `MouseUp` / `MouseMoved` / `MouseEntered` / `MouseLeft` / `DoubleClick` / **`Scroll`** | Hit-test at cursor position | The user is *pointing at* something — they mean that thing, regardless of where the keyboard focus happens to be. |
| `KeyPressed` / `CharTyped` | Focus | The user has declared "this is where I'm typing." |
| `Accelerator` | `AcceleratorScope` field (see §3) | Keybindings can be scoped narrower than focus (per-mode, per-widget) or broader (global). |
| `WindowResized` / `WindowClose` / `WindowFocused` / `DpiChanged` | Application-global | Platform-level, not user-targeted. |
| `FilesDropped` | Hit-test at drop position | Same rationale as mouse — user dropped there. |
| `ClipboardPaste` | Focus | Paste goes to whichever text-input has focus. |

The practical consequence — and the one users notice first if we get
it wrong: **scroll-wheel events dispatch to the widget under the
cursor, regardless of keyboard focus.** Hover over a sidebar list,
spin the wheel, list scrolls — even if the editor has keyboard focus.
This matches native behaviour on Win32, Cocoa, and GTK. Getting this
wrong produces "the scroll wheel only works on the focused widget"
bugs that are instantly recognisable as non-native.

Backends do the hit-test **before emitting** and set `widget:
Option<WidgetId>` in every mouse variant. Apps dispatch on the widget
ID without consulting their own focus state. `None` widget means the
cursor was over non-widget area (bare editor content, empty
background); apps handle those via `position` if meaningful.

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
    /// Parser accepts two formats — see "Input formats for Literal" below.
    Literal(String),
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

### Input formats for `Literal` (Decision 5 = C)

The parser accepts **both** vim-style and plus-style strings. Which
one to parse is detected from the first character — `<` means
vim-style, anything else means plus-style. Internal representation
after parse is the same `Modifiers + Key` tuple; apps never care
which format the string came in as.

| Example | Style | Notes |
|---|---|---|
| `<C-S-t>` | Vim | `C` = Ctrl, `S` = Shift, `A` = Alt, `D`/`M` = Cmd/Super |
| `<C-A-Left>` | Vim | Named keys inside brackets |
| `Ctrl+Shift+T` | Plus | Case-insensitive modifiers |
| `Cmd+Shift+K` | Plus | `Cmd` renders as `⌘` on macOS, `Ctrl` elsewhere |
| `Alt+F4` | Plus | Named keys work unadorned |

**Recommended convention for new quadraui apps:** plus-style
(`Ctrl+Shift+T`). Matches what OS-native menus, tooltips, and docs
show users; what Postman / k8s / SQL-client audiences expect.

**Vim-native apps** (vimcode, potential vim-workflow extensions) may
stay on vim-style; zero migration cost and the convention is internal
to those codebases. The parser supports both indefinitely.

**Canonical rendering** (what `render_accelerator(acc)` returns for
display in menus, tooltips, palette entries, help overlays) is
**always** platform-appropriate: `⌘⇧T` on macOS, `Ctrl+Shift+T` on
Win/Linux/TUI — regardless of which input format the app used.
Input format ≠ render format.

**Case sensitivity:**

- Modifier names (`Ctrl`/`ctrl`/`CTRL`, `C`, `cmd`/`Cmd`/`CMD`) are
  case-insensitive on input.
- Key character is lowercased internally (`T` → `t`) so `Ctrl+T` and
  `Ctrl+t` parse to the same binding. Shift-T on a keyboard requires
  writing `Shift+T` explicitly (or `<S-t>`).

### Registration API (draft)

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
    // ── Frame + viewport ────────────────────────────────────────
    /// Viewport geometry in native units. Caller converts to rows
    /// via `viewport.rows_at(line_height)` etc.
    fn viewport(&self) -> Viewport;

    /// Begin a frame. Backends may set up render target, clear, etc.
    fn begin_frame(&mut self, viewport: Viewport);

    /// Flush to screen.
    fn end_frame(&mut self);

    // ── Events + keybindings ────────────────────────────────────
    /// Poll all queued native events. Returns a fully-translated
    /// `Vec<UiEvent>` ready for app dispatch. Never blocks.
    fn poll_events(&mut self) -> Vec<UiEvent>;

    /// Block for up to `timeout` waiting for at least one event.
    /// Returns empty vec on timeout. Used by apps that don't want
    /// to busy-poll.
    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent>;

    /// Register an accelerator with the backend's native keybinding
    /// system. Backend stores it and emits `UiEvent::Accelerator`
    /// when matched.
    fn register_accelerator(&mut self, acc: &Accelerator);

    // ── Platform services ───────────────────────────────────────
    /// Clipboard, file dialogs, notifications, open-url, platform name.
    fn services(&self) -> &dyn PlatformServices;

    // ── Drawing — one method per primitive (Decision 2 = B) ────
    // Implementations are thin wrappers around the backend crate's
    // internal draw functions, e.g.:
    //
    //   impl Backend for WinBackend {
    //       fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
    //           quadraui_win::draw_tree(self.ctx(), tree,
    //                                   self.theme(), rect);
    //       }
    //       // ... same pattern for the other primitives
    //   }
    //
    // Adding a primitive is a breaking change to this trait. That's
    // intentional: the "which backends need this?" conversation
    // happens at trait-update time, not as a runtime panic from a
    // defaulted method.
    fn draw_tree(&mut self, rect: Rect, tree: &TreeView);
    fn draw_list(&mut self, rect: Rect, list: &ListView);
    fn draw_form(&mut self, rect: Rect, form: &Form);
    fn draw_palette(&mut self, rect: Rect, palette: &Palette);
    fn draw_status_bar(&mut self, rect: Rect, bar: &StatusBar);
    fn draw_tab_bar(&mut self, rect: Rect, bar: &TabBar);
    fn draw_activity_bar(&mut self, rect: Rect, bar: &ActivityBar);
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal);
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay);
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

### How apps use this

The per-method shape lets app render code be **fully backend-generic**:

```rust
fn render_sidebar<B: Backend>(backend: &mut B, tree: &TreeView, rect: Rect) {
    backend.draw_tree(rect, tree);
}
```

One codebase, runs on every backend. No `#[cfg(...)]` gates sprinkled through app code selecting between `quadraui_gtk::draw_tree` vs `quadraui_tui::draw_tree` vs `quadraui_win::draw_tree`.

Plugins and apps that want **backend-specific** rendering (e.g. a bespoke primitive defined outside quadraui) still call the free function directly — the quadraui backend crates expose both the trait impl and the underlying `pub fn draw_tree(...)`, so neither path is walled off.

### Why not `AnyPrimitive` enum dispatch

An alternative design — a single `fn draw_primitive(&mut self, rect: Rect, p: &AnyPrimitive)` with `AnyPrimitive` being an enum of every primitive variant — was considered and rejected. Reasons:

1. Adding a primitive still requires work in every backend (a new match arm, same as a new trait method), but changes become **runtime panics via unhandled arms**, not compile errors.
2. The `AnyPrimitive` enum ossifies every primitive's public shape into one bulky type and brings `'a` lifetime parameters everywhere the enum flows.
3. Violates `quadraui/docs/DECISIONS.md` D-001 principle: "one primitive per UX concept, not per algebraic reduction." `AnyPrimitive` is the reduction.
4. The speculative benefit — "plugins can pass heterogeneous primitive lists through one call" — has no concrete use case today and can be added later as `AnyPrimitive` + a single `draw_primitive` method alongside the per-method ones if a real need appears.

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

✅ **RESOLVED 2026-04-22.** Per-primitive methods, explicit impls
required. Backend implementations are thin wrappers around each
backend crate's internal `pub fn draw_*` free functions — apps
benefit from `<B: Backend>` generics without app-side `cfg` gates.
See §4 for the updated trait shape and rejection rationale for
`AnyPrimitive` enum dispatch.

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

Focus is a **keyboard-only** concern — the event-routing table in §2
makes this explicit. Mouse events (including scroll) route by
hit-test at cursor position, not by focus. Focus only determines who
receives `KeyPressed` / `CharTyped`, and factors into `Accelerator`
when scope is `Widget` or `Mode`.

The focus *model* itself still needs a dedicated design pass:

- **Transitions** — does focus move on click, on Tab, on
  app-directed `set_focus(id)`, or all three?
- **Destruction** — what happens when the focused widget is removed?
  Fall back to parent? To an app-designated default?
- **Declaration** — do primitives explicitly opt in to being
  focusable, or is it implicit?
- **Modal interaction** — a `Dialog` opens while a text input has
  focus; what happens on close? Stack-like focus history?
- **Native focus bridging** — on GTK each `DrawingArea` has its own
  native focus; on Win32 the top-level HWND has focus and we
  simulate per-widget focus in-app. The quadraui focus model needs
  to abstract over this without leaking.

None of these block Phase B.1 (types + `poll_events` alongside native
dispatch). The terminal-maximize pilot in B.2 uses
`Accelerator::Global` scope only, so focus isn't required for that
migration. The focus model will want its own proposal doc before
Phase B.3, since `Panel` / `Tabs` / `Dialog` need focus transition
rules to be useful.

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
   ✅ **RESOLVED 2026-04-22: A (ship all three abstractions together,
   phased migration per §5).** `UiEvent` + `Accelerator` + `Backend`
   are interlocking; splitting them would produce half-abstractions.
   Phase-by-phase risk control lives in §5, not in the decision count.

2. **`AnyPrimitive` vs per-method draw?** (§6.1)
   ✅ **RESOLVED 2026-04-22: B (per-method, explicit impls required).**
   Trait impls are thin wrappers around each backend crate's internal
   `pub fn draw_*` free functions. App code uses `<B: Backend>`
   generics without app-side `cfg` gates. See §4 for the trait shape.

3. **Scope of Phase B.1** — is "ship UiEvent + poll_events alongside
   existing code" the smallest viable first PR? Or should we pair
   it with B.2's pilot feature?
   ✅ **RESOLVED 2026-04-22: A (B.1 ships scaffolding alone).** Pure
   additive types + trait skeleton + empty impls. Reviewer focus on
   the shape of the trait/enum, undistracted by a feature migration.
   B.2 lands as a separate PR that uses the scaffolding for real.

4. **Pilot feature for B.2** — terminal maximize is the current
   candidate (painful, contained, measurable). Any reason to pick a
   different one?
   ✅ **RESOLVED 2026-04-22: A (terminal maximize).** Fresh pain =
   clear win narrative; moderate surface exercises the key
   `Accelerator::Global` + `UiEvent::Accelerator` + `register_accelerator`
   path without prematurely forcing the §6.4 focus design. Target:
   ~-60 LOC net after deleting duplicate key dispatch in three
   backends.

5. **`Accelerator::Literal(String)` format** — stick with Vim-style
   `<C-S-t>` or switch to a platform-agnostic `"Ctrl+Shift+T"`? Vim
   style is already used throughout vimcode; carries over cleanly.
   ✅ **RESOLVED 2026-04-22: C (accept both, parser auto-detects).**
   First character determines dispatch (`<` = vim, otherwise plus).
   Zero migration cost for vimcode; Postman/k8s/SQL-client audiences
   get the format they expect. See §3 "Input formats for Literal"
   for the full convention.

---

### All decisions resolved — next step is code

With all five resolved, the first PR (Phase B.1) is small
(~200 LOC — `UiEvent` + `Accelerator` + `Backend` trait skeleton,
empty impls per backend, no feature migration). B.2 lands as a
separate PR migrating terminal maximize; target -60 LOC net.

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
