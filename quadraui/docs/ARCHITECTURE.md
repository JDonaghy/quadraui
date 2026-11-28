# Architecture

**Workspace members:**

| Crate | Purpose |
|---|---|
| `quadraui/` | Core library: primitives, types, theme, backend traits, and the TUI, GTK, macOS and Windows backends. |
| `kubeui-core/` | Domain logic for a Kubernetes dashboard demo. No rendering deps — testable in isolation. |
| `kubeui/` | TUI Kubernetes dashboard. Real consumer of the TUI backend. |
| `kubeui-gtk/` | GTK Kubernetes dashboard. Same domain logic as `kubeui`. |

**The widget contract** inside `quadraui/`
([`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md) has the rules,
[`UI_CRATE_DESIGN.md`](UI_CRATE_DESIGN.md) §3.2 the design):

- `quadraui/src/primitives/` — one file per leaf widget: the descriptor,
  its `Layout`, and its single implementation of `Primitive` (layout,
  `paint` on a `PaintSurface`, `paint_cells` on a `CellSurface`) and
  `Interactive` (focus, `handle`, sub-regions, popups). **Must NOT
  depend on `ratatui`, `gtk4`, `cairo`, or any backend crate.** Layout
  tests live as inline `#[cfg(test)] mod tests` blocks at the bottom of
  each primitive file.
- `quadraui/src/compose/` — composites built from other widgets (a
  dialog, a picker, the workspace and shell chrome) and the controllers
  that coordinate several widgets (`ContextMenuController`, `ShellView`).
- `quadraui/src/tui/`, `quadraui/src/gtk/`, `quadraui/src/macos/`, and
  `quadraui/src/win/` — the four backends (`tui`, `gtk`, `macos`, `win`
  feature-gated respectively). A backend is platform services plus one
  paint target: it implements `PaintSurface` (GTK, macOS, Windows) or
  `CellSurface` (TUI), measures text, translates native input into
  `UiEvent`s, and runs the event loop. **It contains no widget code** —
  no per-widget painter, layout or routing. See the root `README.md`'s
  *Status* section for how far each backend gets today, and
  [`../BACKEND.md`](../BACKEND.md) for what implementing one involves.

**Cross-backend runtime** sits at the top of `quadraui/src/`, shared by
all four backends:

- `primitive.rs` / `interactive.rs` — the two widget traits, `PaintEnv`
  (theme, style, hover/press, the widget's persistent state) and the
  painted-widget record hosts hit-test with (`painted_hit`).
- `backend.rs` — the sealed `Backend` trait (frame lifecycle, metrics,
  platform services) and `DrawsPrimitives`, whose `draw_dyn` /
  `draw_primitive` resolve a widget's layout with the backend's metrics
  and call its `paint` or `paint_cells`; `layout_primitive` re-resolves
  the same layout for a hit-test.
- `paint_surface.rs` / `cell_surface.rs` — the paint-verb traits a
  widget paints through, reached via `Backend::paint_surface` /
  `Backend::cell_surface`.
- `frame.rs` / `frame_tree.rs` / `widget_state.rs` — frame composition
  and routing. `ScreenLayout` + `Surface::Widget` paints a screen from
  one call site; `AppLogic::frame_tree` (or
  `ScreenLayout::into_frame_tree`) returns the same widgets as a
  `FrameTree`, which the runtime hit-tests, focuses and routes to
  `Interactive::handle`; `WidgetStateStore` keeps each widget's
  `Interactive::State` between events.
- `runtime.rs` — shared plumbing for the per-backend runners (`tui::run`,
  `gtk::run`, `macos::run`, `win::run`): the post-event `EventOutcome`
  enum and "apply this outcome to the live window".
- `dispatch.rs` / `modal_stack.rs` — cross-backend mouse + scroll
  dispatch. A shared `ModalStack` decides whether an open modal — never
  a base-layer widget behind it — receives the event.
- `shell.rs` — the `ShellApp` trait + `ShellConfig` for apps that want
  the full shell chrome (activity bar + sidebar + main content) instead
  of implementing `AppLogic` directly. Each backend's `run_with_shell()`
  owns window creation, event wiring, and chrome rendering; the
  consumer renders only its own content, in
  `ShellApp::render_content(backend, &ShellLayout)`, from the rects the
  `ShellView` chrome (`compose/app_shell.rs`) laid out.
  `shell_adapter.rs` is the shared `AppLogic` impl every backend's shell
  runner instantiates.

`Backend` is `pub` but sealed: no implementation outside this crate
(D-013), and `tests/backend_method_ratchet.rs` pins its body to an exact
list of platform-service methods. A new widget never adds a `Backend`
method or a file under a backend directory; if a painter needs something no surface can do, the
new verb goes on `PaintSurface` or `CellSurface` for every backend.

**Audits.** `docs/audits/FRAMEWORK_AUDIT_2026-09-26.md` is the most recent
independent review of quadraui as an app framework: its verdict, the
measured numbers, and the severity-ranked roadmap behind epics #1095
(widget model) and #1096 (language bindings). The living, release-ordered
version of that roadmap is [`ROADMAP.md`](../../ROADMAP.md) at the repo root. `docs/SMELL_AUDIT_2026-07.md`
is the earlier code-smell audit.
