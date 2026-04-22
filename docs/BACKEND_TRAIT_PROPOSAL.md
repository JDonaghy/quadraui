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

---

## 11. Phase B.2 implementation notes — terminal-maximize pilot

**Status:** Sketched 2026-04-22 (pre-code), grounded in a read of the
three current backend dispatch paths. Five questions called out by
`PLAN.md` §"Phase B.2 starting notes". Answers below; each cites
specific file:line references where the existing code is the
load-bearing constraint.

The plan: the terminal-maximize migration is a **near-total
recreation** of the existing `pk.toggle_terminal_maximize` keybinding
plumbing in B.1's `Backend` trait shape — same behaviour, new path.
It is intentionally narrow (one accelerator) so the trait shape is
stress-tested without the noise of a multi-key feature.

### Existing dispatch path (the thing being replaced)

For reference — this is what each backend does today for `Ctrl+Shift+T`:

| Backend | Native event arrives | Translation | Match site | Engine call |
|---|---|---|---|---|
| TUI | `crossterm::event::read` at `src/tui_main/mod.rs:1608` (poll at `:1429`) | `translate_key` at `:4131` (KeyCode → key_name + modifiers); `matches_tui_key` at `:143` | **Two sites**: early intercept at `:2888` (terminal-panel context); `EngineAction::ToggleTerminalMaximize` arm at `:3586` (editor context, via `engine.handle_key` return) | `engine.toggle_terminal_maximize()` (`:2892` and `:3590`); `terminal_target_maximize_rows_tui()` at `:116`; `terminal_resize`/`terminal_new_tab` |
| GTK | GTK4 `EventControllerKey` (Capture phase) `connect_key_pressed` closure at `src/gtk/mod.rs:1213` | `matches_gtk_key` at `:1386` (key + GDK modifier mask vs `pk.toggle_terminal_maximize`) | One funnel for the editor DA: closure at `:1386` calls `sender.input(Msg::ToggleTerminalMaximize)` (variant decl at `:629`) | `App::update` arm at `:7219` calls `App::terminal_target_maximize_rows()` then `engine.toggle_terminal_maximize()` then `terminal_resize`/`terminal_new_tab` |
| Win-GUI | `WM_KEYDOWN` in `wnd_proc_inner` at `src/win_gui/mod.rs:868` (delegates to `on_key_down` at `:1791`) | `translate_vk` at `:1797` produces `Key { key_name }`; modifiers from three `GetKeyState` calls at `:1793-1795` | Inline cascade in `on_key_down`: `if ctrl && shift && !alt && (key.key_name == "t" || "T")` at `:1832` | `state.engine.toggle_terminal_maximize()` at `:1843`; `win_gui_terminal_target_maximize_rows()` at `:1640`; `terminal_resize`/`terminal_new_tab`; `InvalidateRect` |

Total duplication: 3 backends × (key match + size compute + engine
call + repaint trigger) ≈ ~80 LOC of native-event plumbing for one
keybinding. Plus the TUI's *internal* duplication (sites `:2888` and
`:3586` both compute `terminal_target_maximize_rows_tui` + dispatch).

That cluster is what B.2 deletes (and replaces with one
`register_accelerator` call per backend + one `Engine::handle_ui_event`
arm matching `Accelerator("terminal.toggle_maximize", _)`).

---

### Q1 — `TuiBackend` struct shape

**Recommendation:** the backend owns the ratatui `Terminal` end-to-end.
Apps never see a raw `Frame<'_>`; they call `backend.begin_frame()` /
`backend.draw_*(...)` / `backend.end_frame()` in Backend-trait order.

The constraint: ratatui's `Frame<'_>` is only obtainable inside the
closure passed to `terminal.draw(|frame| ...)`, and its lifetime is
tied to that closure. There is no way to store a `Frame` on the
backend struct across method calls. So the backend must either:

- **(A) Defer draws** — `draw_*` methods paint into a frame-scoped
  `ratatui::buffer::Buffer` owned by the backend; `end_frame` runs
  `terminal.draw(|f| f.buffer_mut().merge(taken_buffer))`.
- **(B) Closure-pattern API** — extra trait method
  `with_frame(&mut self, f: impl FnOnce(&mut Self))` that opens a
  ratatui draw context. App code becomes
  `backend.with_frame(|b| { b.draw_tree(...); ... })` instead of
  `begin_frame` / draw / `end_frame`. Violates the §4 trait shape.

Pick **(A)**. Trait stays clean; deferred buffer is cheap (one
`Buffer::empty(viewport)` per frame, all painting is mutation). Same
pattern works for GTK (Cairo `Context` from `connect_draw` is
similarly closure-scoped) and Win-GUI (Direct2D render target is
already long-lived, no constraint).

Concrete struct:

```rust
pub struct TuiBackend {
    terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    accelerators: Vec<Accelerator>,    // small N; linear scan is fine
    event_queue: VecDeque<UiEvent>,
    current_buffer: Option<ratatui::buffer::Buffer>,  // Some between begin/end_frame
    services: TuiServices,             // clipboard, etc.
    // Modifier-aware key naming (parity with src/tui_main/mod.rs:4131).
    keyboard_enhanced: bool,
}
```

`begin_frame(viewport)`: allocate `current_buffer` sized to viewport.
`draw_tree(rect, tree)` etc.: paint into `self.current_buffer.as_mut().unwrap()` via the existing `quadraui_tui::draw_tree` free functions (which already accept a `&mut Buffer` after the A.1a pattern; verify this when implementing).
`end_frame()`: take the buffer, run `self.terminal.draw(|f| { let b = f.buffer_mut(); /* merge taken into b */ })`.

### Q2 — Native event → `UiEvent` translation algorithm

**TUI** — `TuiBackend::poll_events`:

1. While `crossterm::event::poll(Duration::ZERO).unwrap_or(false)`:
2. Drain one `crossterm::event::Event` via `event::read()`.
3. Translate by variant:
   - `Event::Key(k)` → reuse the existing `translate_key` algorithm
     (`src/tui_main/mod.rs:4131`), which produces
     `(key_name: String, unicode: Option<char>, ctrl: bool)`. Build a
     `Key` enum (per §2 type) + `Modifiers` from this.
   - `Event::Mouse(m)` → `UiEvent::MouseDown` / `MouseUp` / `MouseMoved`
     / `Scroll` with `widget: None` for B.2 (hit-testing comes in B.3
     when `Panel` exists). Position via `Point { x: m.column, y: m.row }`.
   - `Event::Resize(c, r)` → `UiEvent::WindowResized { viewport: ... }`.
   - `Event::Paste(s)` → `UiEvent::ClipboardPaste(s)`.
   - `Event::FocusGained`/`FocusLost` → `UiEvent::WindowFocused(_)`.
4. For key events, **before** emitting `UiEvent::KeyPressed`, check
   the accelerator registry. First-match-wins:
   - For each `Accelerator { binding, scope: Global, id }` in
     `self.accelerators`, ask `binding.matches(&key_name, modifiers)`
     (helper exists in B.1's `Accelerator` impl, line refs in
     `quadraui/src/accelerator.rs`). If true → emit
     `UiEvent::Accelerator(id.clone(), modifiers)` and skip the
     `UiEvent::KeyPressed` for this event.
   - For `Widget` and `Mode` scopes: defer to B.3 / B.4. In B.2 the
     registry only ever holds `Global` accelerators, so the algorithm
     above is exhaustive.
5. Push translated `UiEvent` onto `self.event_queue`.
6. After the drain loop, return `self.event_queue.drain(..).collect()`.

**Algorithm rationale:** "first-match-wins, scope-filtered" mirrors
how editors universally resolve keymap conflicts. Walking the
registry every key press is O(N) where N is the registered count
(maximize alone = 1; full vimcode under B.4 = ~30); a Vec scan
beats a HashMap for that range. If N grows past ~50, swap to a
two-level dispatch (modifier → Vec) — but premature for B.2.

**GTK** — `GtkBackend::poll_events` is a queue-drain only (events
arrive via `EventControllerKey` callbacks, not by polling — see Q4).
Translation happens *inside* the callback before push. Same accelerator
match logic as TUI; same fall-through to `UiEvent::KeyPressed` if
unmatched.

**Win-GUI** — same shape as GTK: translation happens inside
`wnd_proc_inner` (`src/win_gui/mod.rs:832`) before push to the
`WinBackend.event_queue`. See Q5.

### Q3 — Main-loop integration

**Recommendation:** keep the existing main-loop structure in each
backend; replace the inner native-event dispatch with
`backend.poll_events()` → `engine.handle_ui_event(ev)`. Minimum-diff,
preserves all the surrounding machinery (LSP polling, frame-rate
limiting, terminal-panel polling, idle hooks).

**TUI** (`src/tui_main/mod.rs:1022` `event_loop`):

- Currently: `event::poll` at `:1429`, then `event::read` at `:1608`,
  then dispatch through `matches_tui_key` (early-intercepts) or
  `engine.handle_key` at `:3564`.
- After B.2: keep the structure. **Add** `let ui_events =
  backend.poll_events();` near the top of each loop iteration. **For
  each ui_event**, call `engine.handle_ui_event(ev)` — `Engine`
  dispatches `Accelerator("terminal.toggle_maximize", _)` to
  `engine.toggle_terminal_maximize()` etc. **Delete** the early
  intercept at `:2888-2899` (the `matches_tui_key(&pk.toggle_terminal_maximize, …)`
  block) and the `EngineAction::ToggleTerminalMaximize` arm at
  `:3586-3596`. Other keys still flow through the legacy
  `event::read` → `translate_key` → `engine.handle_key` path —
  coexistence per §5 Phase B.1's contract.
- The viewport-rows computation (`terminal_target_maximize_rows_tui`
  at `:116`) moves into the `Engine::handle_ui_event` arm itself —
  it's pure, takes no terminal-specific args other than the screen
  height which is on `Viewport`. Engine grows a small wrapper that
  reads `viewport.height_rows` from cached state, builds the
  `PanelChromeDesc`, and calls the existing
  `toggle_terminal_maximize` + `terminal_resize` chain.

**GTK** (`src/gtk/mod.rs` Relm4 main loop):

- Relm4 owns the GLib loop; we don't touch it directly. App.update
  drains the backend on every Msg cycle (see Q4 for the wake-up
  path). The `Msg::ToggleTerminalMaximize` arm at `:7219` stays
  initially as the dispatch target of `Engine::handle_ui_event`; the
  caller chain becomes:
  - `EventControllerKey` closure at `:1386` translates +
    accelerator-matches → pushes `UiEvent::Accelerator(...)` to
    `backend.event_queue` → fires `sender.input(Msg::PollUiEvents)`.
  - `Msg::PollUiEvents` arm: `for ev in backend.poll_events() {
    engine.handle_ui_event(ev); }`. (This Msg variant is added in
    B.2; in B.4 it absorbs all the per-widget `Msg::*Key` variants.)
  - `engine.handle_ui_event` for the maximize accelerator computes
    target rows via the engine wrapper (same as TUI) and calls
    `toggle_terminal_maximize` etc. directly — bypassing the
    `Msg::ToggleTerminalMaximize` arm. We can then delete that arm
    and `Msg::ToggleTerminalMaximize` in the same PR.
- The `App::terminal_target_maximize_rows()` GTK helper currently
  reads DA pixel height + `cached_line_height` (line refs in
  `src/gtk/mod.rs` per the existing helper). Under B.2, the engine
  needs that info — so the GTK Relm4 wrapper passes
  `Viewport { width_px, height_px, line_height_px, char_width_px }`
  on `WindowResized` events, and `Engine` caches the latest viewport
  to use inside `handle_ui_event`. This is a small change with
  reusable value (other accelerators in B.4 will need viewport too).

**Win-GUI** (`src/win_gui/mod.rs:726-732` message pump):

- Pump unchanged. **After** `DispatchMessageW` returns at `:731`,
  drain: `for ev in backend.poll_events() { engine.handle_ui_event(ev); }`.
  WndProc's job (Q5) is to push, not dispatch. This keeps event
  handling on the main thread between message-pump iterations and
  matches the natural Win32 pattern.
- The maximize arm at `:1832-1853` collapses to: `if ctrl && shift &&
  !alt && key.key_name == "t" { backend.push_accelerator_match("terminal.toggle_maximize", mods); return true; }`,
  which is what `WinBackend::translate_and_push` does for any
  registered accelerator. Specific `if ctrl && shift && key == "t"`
  string disappears.

### Q4 — GTK event ownership: where does the queue live?

**Recommendation: side-channel queue + Relm4 wake-up Msg.** Concretely:

- `GtkBackend` owns `Rc<RefCell<VecDeque<UiEvent>>>` plus
  `Rc<RefCell<Vec<Accelerator>>>` (the registry).
- Each `EventControllerKey` closure (the editor DA at
  `src/gtk/mod.rs:1213` is the primary one; see also six per-widget
  controllers at `:2075` Settings, `:2145` Ex, `:2759` Debug, `:2818`
  Sc, `:2874` Ext, `:2921` ExtPanel, `:3155` AI) clones both `Rc`s at
  `init_widgets` time, alongside the existing `sender` clone.
- When a key arrives: closure runs `backend_match(key, modifier,
  &accelerators)`; on match, pushes
  `UiEvent::Accelerator(id, mods)` to `event_queue` and calls
  `sender.input(Msg::PollUiEvents)`.
- On no match: pushes `UiEvent::KeyPressed { key, modifiers, repeat:
  false }` and same `sender.input(Msg::PollUiEvents)`.
- `App::update` arm `Msg::PollUiEvents`:
  `for ev in self.backend.poll_events() { self.engine.borrow_mut().handle_ui_event(ev); }`.

**Why side-channel + sender-wake, not `Msg::UiEvent(UiEvent)` directly:**
direct `Msg::UiEvent` looked attractive but breaks ordering when two
keys arrive in close succession — Relm4 may interleave Msg arrivals
with other events, and we want strict FIFO across all sources (key,
mouse, paste). A single queue + wake-up Msg gives strict FIFO; the
Msg payload doesn't matter (carries nothing).

**Why not a Relm4 sub-component:** Relm4 components have their own
update + view; `GtkBackend` has no view (it draws via the existing
DA `connect_draw` callbacks) and no Msg-cycle of its own. Wrapping
it as a sub-component is overhead without benefit. Side-channel is
the right primitive — it just lives next to App, not inside Relm4.

**`backend.poll_events()` impl on GTK:** drains the `Rc<RefCell<VecDeque>>`
into a `Vec`. Trivial.

**Edge case — drop ordering at App teardown:** the queue's Rc clones
in closures must drop before App. Relm4 takes care of this naturally
when the App component is dropped (controllers are removed first).
Verify when implementing.

### Q5 — Win-GUI WndProc → `WinBackend` queue hookup

**Recommendation: WndProc translates inline (option (b) from PLAN.md
§"Phase B.2 starting notes" Q5).** WndProc pushes a fully-translated
`UiEvent` to `WinBackend.event_queue`; `poll_events` just drains.

**Why inline beats deferred:**

- Modifier state must be read at the moment of the key event via
  `GetKeyState` (`src/win_gui/mod.rs:1793-1795`). Deferring to
  `poll_events` means `GetKeyState` returns the *current* modifier
  state, not the state at the moment of `WM_KEYDOWN`. Possible to
  capture-and-store in a "raw event" struct, but that's just inline
  translation with extra boxing.
- The existing `translate_vk` at `:1797` already produces the
  canonical `Key { key_name }` we need. Reusing it in WndProc costs
  nothing.
- `state.line_height` (`:307`) and `state.engine` are accessible via
  `APP.with(|app| ...)` inside WndProc (line refs across the file).
  A future `WinBackend` lives on `state` next to `engine`; same
  access.
- Inline is symmetric with how GTK handles it (Q4) — both push-based
  GUI backends do translation in the native callback; only the pull-
  based TUI does it in `poll_events`.

**Concrete WndProc rewrite for the maximize key (replaces
`:1832-1853`):**

```rust
if let Some((id, mods)) = state.backend.match_key_to_accelerator(&key) {
    state.backend.push(UiEvent::Accelerator(id, mods));
    InvalidateRect(state.hwnd, None, false);
    return LRESULT(0);
}
// fall through to UiEvent::KeyPressed push, plus legacy on_key_down
// dispatch for keys not yet migrated to UiEvent
state.backend.push(UiEvent::KeyPressed { key, modifiers: mods, repeat: false });
```

**Drain site:** in the message pump at `:726-732`, after
`DispatchMessageW(&msg)`:

```rust
APP.with(|app| {
    let mut state = app.borrow_mut();
    let events = state.backend.poll_events();
    for ev in events {
        state.engine.handle_ui_event(ev);
    }
});
```

This adds one `APP.with` per pump iteration. Cheap; `GetMessageW`
blocks in normal paint cycles anyway.

**Why not keep WndProc dispatching directly to engine (skip the
queue):** breaks the `Backend::poll_events` trait contract. Apps that
want to inspect or filter events before dispatch (e.g. recording for
replay per §2 invariants, or a future reduced-input mode) need the
queue as a chokepoint. Cost of one VecDeque push per event is
negligible.

---

### Recommended next step after §11 lands — TUI-only spike

Worth considering **before** going broad on B.2: implement
`impl Backend for TuiBackend` end-to-end for the maximize accelerator
*alone*, and verify it runs in-tree without integrating with vimcode's
main `event_loop` yet (e.g. via a tiny standalone example in
`quadraui/examples/maximize_pilot.rs`). Goals:

1. Validate Q1 (the deferred-buffer pattern actually works with
   ratatui's `Frame` API).
2. Validate Q2's accelerator-match algorithm against real crossterm
   key events including the `keyboard_enhanced` path.
3. Iterate §11 if any of the above answers turn out wrong, *before*
   replicating the pattern to GTK and Win-GUI.

Cost: ~half-day. Reduces "big bang" risk for the GTK + Win-GUI parts,
which are harder to iterate on (both require running GUI builds).

The pattern would be: tiny binary that opens a TUI surface, registers
one accelerator (`"toggle_maximize"` → `<C-S-t>`), runs the event
loop, prints "MAXIMIZED" on each `Accelerator` event. ~50-80 LOC,
disposable after B.2 ships. Optional but recommended.

### Spike findings (2026-04-22) — TUI accelerator caveats

The recommended TUI spike was implemented at
`quadraui/examples/maximize_pilot_tui.rs` (commits `ed2e8a9` +
`f775d06` on branch `quadraui-spike-tui-maximize`). The spike is a
single-file `impl Backend for TuiBackend` that registers exactly the
maximize accelerator, paints a status bar via the deferred-buffer
pattern, and toggles state on each fire.

**What it validated:**

1. **Q1 deferred-buffer pattern works.** Owned `Buffer` in
   `current_buffer: Option<Buffer>`, allocated in `begin_frame`,
   manual cell-copy into `f.buffer_mut()` inside `terminal.draw` in
   `end_frame`. Renders cleanly. Note that `Buffer::merge` was
   sidestepped in favour of an explicit cell loop — equal-area
   constraints on the merge API made the manual copy more robust for
   the spike. B.2 can revisit if there's a perf reason.
2. **Q2 algorithm is sound.** First-match-wins against the registered
   accelerators with `parse_key_binding` cached at registration time
   matches correctly *when the input arrives correctly*. The match
   logic itself is not the bottleneck.
3. **Trait shape is implementable end-to-end** without extension
   traits. All 9 `draw_*` methods + `register_accelerator` +
   `poll_events` + `wait_events` + `services` + lifecycle methods
   compile and behave as expected.

**What it surfaced — and §11 did not predict:**

TUI accelerator dispatch is at the mercy of a three-layer stack
*outside* the Backend trait's control:

| Layer | Behaviour | Outcome |
|---|---|---|
| **Terminal emulator** | gnome-terminal, iTerm intercept `Ctrl+Shift+T` for "new tab" | App never sees the keystroke |
| **Multiplexer (tmux, screen)** | tmux strips Shift bit unless `extended-keys on` is configured | Spike reported `Last key: Ctrl+t` (Shift dropped) inside tmux on alacritty |
| **Kitty enhancement protocol** | Only kitty/foot/recent-alacritty/wezterm/ghostty/recent-iTerm support `DISAMBIGUATE_ESCAPE_CODES` | Older terminals silently ignore the push |

Vimcode TUI today already pushes the same enhancement flags (see
`src/tui_main/mod.rs:931-938`), so this is a **pre-existing
limitation** of the TUI binding `pk.toggle_terminal_maximize`, not a
B.2 regression. Users who run vimcode TUI inside tmux on alacritty
have *never* been able to use `Ctrl+Shift+T` for terminal maximize;
the spike just made the failure mode visible.

**Implications for B.2:**

- **Algorithm is fine — proceed with B.2.** The accelerator-match
  code in vimcode's `TuiBackend` will work correctly for any input
  the terminal stack delivers intact.
- **Default bindings should be chosen for lowest-common-denominator
  TUI environments.** Function keys (`F11`), single-modifier chords
  (`Ctrl+Space`), or leader-key sequences (vim-style `<leader>tm`)
  survive the most stack configurations. `Ctrl+Shift+<letter>` is
  fragile.
- **Settings.toml should remain authoritative.** Apps must let users
  rebind any accelerator that doesn't survive their specific
  terminal stack. The B.2 trait API already supports this — users
  just override the `KeyBinding` of any registered `Accelerator`.
- **Document the failure modes for users.** TUI section of the
  vimcode README should call out tmux's `extended-keys on` config
  and gnome-terminal's preferences → shortcuts override pattern.
  This is product-doc work, not architecture work.
- **B.2 does NOT need a "fallback binding" mechanism in quadraui
  itself.** Letting apps register multiple accelerators with the
  same `id` is a thinkable feature (try `<C-S-t>`, fall back to
  `<F11>`), but it's solving a layer-cake problem at the wrong
  layer. The right layer is app config + user-facing docs.

**The spike branch.** `quadraui-spike-tui-maximize` carries the
working TuiBackend impl + diagnostic harness (commits `ed2e8a9` +
`f775d06`). Three reasonable dispositions, choose at B.2 kickoff:

- (a) **Discard.** Lessons captured in this section; commits stay in
  reflog if needed. Cleanest.
- (b) **Squash + keep as reference example.** Single commit landing
  `quadraui/examples/maximize_pilot_tui.rs` to develop. Small
  ongoing maintenance cost (must compile against future Backend
  trait changes); high pedagogical value for the next backend
  implementer (e.g. macOS Phase C).
- (c) **Promote.** Move `TuiBackend` from the example into
  `quadraui::tui::TuiBackend`, drop the diagnostic harness, use it
  as B.2's starting code. Skips re-writing what already works;
  forces an early decision on whether quadraui ships per-backend
  modules vs separate sub-crates (currently neither; just one
  crate).

Recommendation: (c) is the most diff-efficient path to B.2 if there's
no architectural objection to a `quadraui::tui` module. (b) is a
lower-commitment alternative. (a) is fine if the spike code is
considered too entangled with the diagnostic harness to extract
cleanly.

### B.2 implementation notes (2026-04-22) — engine-owned registry

When B.2 was implemented, the "backend owns the event loop" shape
from §11 Q3 proved more invasive than the maximize pilot justified.
The final shape is **engine-owned registry, backend lookup-on-demand**.
Same user-visible B.1 types (`Accelerator`, `UiEvent`,
`KeyBinding`, `parse_key_binding`); same deletion of per-backend
`matches_*_key` checks for the migrated keybinding; smaller blast
radius to vimcode's three event loops.

**What changed from §11 Q3:**

§11 described a full event-loop swap: backend owns crossterm /
GTK signals / WndProc, drains via `poll_events()`, emits typed
`UiEvent`s for everything, apps consume in a `for ev in
backend.poll_events()` loop. For B.2 with exactly one accelerator
to migrate, that meant translating every crossterm mouse + paste +
focus event into a faithful `UiEvent` variant and back-translating
`UiEvent::KeyPressed` to vimcode's existing `(key_name, unicode,
ctrl)` shape for all the keys *not* yet migrated — ~400 lines of
back-translation code to light up one accelerator.

The simpler shape implemented in B.2:

```rust
// Engine (src/core/engine/mod.rs)
pub struct RegisteredAccelerator { acc, parsed }
pub struct UiEventContext { terminal_cols, terminal_max_rows }
impl Engine {
    pub fn register_accelerator(&mut self, acc: Accelerator);
    pub fn unregister_accelerator(&mut self, id: &AcceleratorId);
    pub fn match_accelerator(&self, ctrl, shift, alt, key_char, is_tab, is_space, is_escape) -> Option<AcceleratorId>;
    pub fn handle_ui_event(&mut self, ev: UiEvent, ctx: UiEventContext);
    pub fn register_default_accelerators(&mut self);  // called from Engine::new()
}
```

At each backend's existing key-handler site, the `matches_*_key`
check becomes an `engine.match_accelerator(...)` lookup; on match,
the backend fills a `UiEventContext` (terminal cols + max-rows in
native units) and calls `engine.handle_ui_event(...)`. The engine
owns the flip-and-resize sequence; backends keep their native
viewport math because it genuinely differs (TUI: cells; GTK/Win-GUI:
`floor(px/lh)`).

**What this buys:**

- **One registry, three backends.** Adding the second accelerator
  in B.4 costs exactly one line (`register_accelerator(...)` in
  `register_default_accelerators`) + one arm in `handle_ui_event`.
  No new `matches_*_key` literal in any backend.
- **Zero event-loop disruption.** TUI's `event_loop`, GTK's Relm4
  dispatcher, and Win-GUI's WndProc all stay exactly as they are.
  The migration sits inside existing key-handler blocks.
- **No back-translation cost.** Keys that aren't registered
  accelerators flow through legacy paths unchanged. No
  `UiEvent::KeyPressed → (key_name, unicode, ctrl)` shim.

**What it defers:**

- **Full backend-owned event loops** (§11 Q3's recommendation).
  Real payoff is in B.4 when accelerator count grows; B.2's one
  accelerator doesn't justify the rewrite cost. Each stack of
  `engine.match_accelerator(...)` calls across the three backends
  could still migrate to `backend.poll_events()` later, and the
  engine-owned registry is the natural home for that logic.
- **`Backend` trait impls for vimcode's own backends.** There are
  no `TuiBackend` / `GtkBackend` / `WinBackend` structs in vimcode
  after B.2. The trait from B.1 is exercised by the spike (and
  available to Postman / future consumers); vimcode uses the
  engine-owned helpers directly. When B.5 kicks off the Postman
  validation app it'll write its own `PostmanTuiBackend` etc.,
  which is when the trait's value proposition lands.
- **Live rebinding.** `register_accelerator` replaces prior
  entries by id (tested), but there's no hook on settings reload
  yet. Rebinding requires restart. B.4+ can add a
  `Engine::rebuild_accelerators()` hook alongside
  `rebuild_user_keymaps()`.

**Five sites migrated:**

| File | Before | After |
|---|---|---|
| `src/tui_main/mod.rs:2888` (terminal-panel key intercept) | `matches_tui_key(&pk.toggle_terminal_maximize, ...)` + 8-line flip+resize | `engine.match_accelerator(...)` + `engine.handle_ui_event(...)` |
| `src/tui_main/mod.rs:3586` (EngineAction dispatch arm) | 9-line flip+resize sequence | `engine.handle_ui_event(...)` |
| `src/gtk/mod.rs:1386` (EventControllerKey closure) | `matches_gtk_key(&pk.toggle_terminal_maximize, ...)` | `engine.match_accelerator(...)` + `sender.input(Msg::ToggleTerminalMaximize)` |
| `src/gtk/mod.rs:7219` (`Msg::ToggleTerminalMaximize` handler) | 14-line flip+resize sequence | `engine.handle_ui_event(...)` |
| `src/win_gui/mod.rs:1832` (WndProc cascade) | `if ctrl && shift && !alt && key.key_name == "t"` + inline flip+resize | `engine.match_accelerator(...)` + `engine.handle_ui_event(...)` |
| `src/win_gui/mod.rs:4586` + `:6178` (EngineAction handlers) | 15 + 10 line flip+resize sequences | `engine.handle_ui_event(...)` at both |

**LOC delta:** +339 / -74 across 6 files. Net +265 for one
accelerator. The payoff shows up at accelerator #2: each new
registered binding adds ~1 line per backend (just the `match` arm on
`AcceleratorId` if backends dispatch per-id, or zero lines if the
backend routes all matched accelerators through a single
`Msg::DispatchAccelerator`).

**Integration tests:** `tests/accelerator_registry.rs` — 10 tests
covering default registration, match positive/negative, case
insensitivity, idempotent toggle, unknown-accelerator no-op,
re-register-same-id replaces, unregister removes, scope filtering
(non-Global rejected).

### What §11 explicitly does NOT decide

- **Focus model.** Per §6.4, deferred. B.2 only uses `Accelerator::Global`,
  which doesn't need focus. Widget/Mode scope arrive in B.3+ with
  the focus design.
- **`Backend` trait method for hit-testing.** Mouse events for B.2
  carry `widget: None`. Real hit-testing arrives with `Panel` in B.3.
- **`Engine::handle_ui_event` dispatch shape.** B.2 has exactly one
  arm (`Accelerator("terminal.toggle_maximize", _)`). The
  match-statement-vs-HashMap question is premature with N=1.
- **What happens to `EngineAction::ToggleTerminalMaximize`.** Likely
  deleted after B.2 since the new path doesn't need it; verify
  during implementation that no other code references it.
- **Lua plugin API shape for accelerator registration.** Out of scope;
  separate proposal before plugin authors get access.
