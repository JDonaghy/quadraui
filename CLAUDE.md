# CLAUDE.md — quadraui

Agent-facing rules for the **quadraui** repo. Re-read on every turn, so it holds
only what a diff can violate. Rationale, history and long playbooks live in
[`docs/AGENT_REFERENCE.md`](docs/AGENT_REFERENCE.md) — read a
section of it only when your task touches that area.

quadraui is **self-contained at design time** (no primitive encodes a consumer's
domain model) but **not at delivery time**: published consumers build against it.
Read *Downstream consumers* before changing any `pub` item.

## North star: extend, don't fork

Before adding a primitive, painter, controller or layout helper, find the closest
existing one and **build from its parts**. A widget that combines existing ones
(a table with a tree column, a list with checkboxes) composes them: it holds the
existing descriptor, calls its layout, and paints through its helpers. It does not
get its own copy of them.

- **If the part you need isn't callable, extracting it is in scope.** That applies
  when it is private, inside a monolithic `paint`/`draw_*_inner` function, or tied
  to one row type. Do it even when the file is outside your issue's `## Files`, and
  name the extraction in your final message.
- **Copying more than ~20 lines from another primitive or backend is a review
  reject.** That includes "modelled on X" rewrites that reproduce X's logic.
- **A combination widget adds nothing under `src/{tui,gtk,macos,win}`.** It
  holds its children's descriptors and paints them through their own
  `Primitive` painters, reading their hover through
  `PaintEnv::child_interaction`. If a child's painter is missing a hook, add
  the hook.
- **If your briefing tells you to write a parallel copy**, say so in your final
  message. Don't follow it.

## Codebase navigation — query the graph first

`graphify-out/` holds a knowledge graph of this repo. For "where is this handled /
what calls this" questions, query it (the `graphify` skill or CLI) before grep/Read.

## Background docs — on demand only

No reading chain is required at session start. The background docs (README,
`quadraui/docs/decisions/DECISIONS.md`, `ARCHITECTURE.md`, `TESTING.md`,
`LESSONS.md`, …) are large; open one only when the task needs it — the index is
in `AGENT_REFERENCE.md`. **Exception:** read `quadraui/docs/PRIMITIVE_RULES.md`
rule 8 before removing or renaming anything `pub`.

## The widget contract

A widget has **one implementation**: a descriptor plus `Primitive`
(+ `A11yPrimitive`) for layout and paint, and `Interactive` for behaviour
(`non_interactive!` when it has none). A leaf widget lives in
`quadraui/src/primitives/<name>.rs`; a composite built from other widgets (a
dialog, a picker, an app shell) lives in `quadraui/src/compose/`. Backends are
**platform services and rasterisers only**: GTK, macOS and Win implement
`PaintSurface`, TUI implements `CellSurface`, and none of them knows any widget.
A new backend therefore implements `Backend` (platform services) plus one of
those two surfaces, and every widget paints on it with no further change. The
Python/Node bindings reach the same implementation: `quadraui-binding-core`
decodes a widget by its `widget_kinds!` kind and paints, routes and keeps its
state through its own `Primitive`/`Interactive`, with no binding-side copy. How
to write a widget: `quadraui/docs/PRIMITIVE_RULES.md`. Non-negotiable:

1. **A widget never gets a `Backend` method or a per-backend file.** `layout()`
   resolves geometry once; `paint()` draws it through `&mut dyn PaintSurface`;
   `paint_cells()` draws it through `&mut dyn CellSurface` for TUI. Hosts reach
   it through `DrawsPrimitives::draw_dyn` (or `Surface::Widget` in a
   `ScreenLayout`), never a widget-named `draw_*`. `Backend` is sealed;
   `tests/backend_method_ratchet.rs` pins it to its platform methods and
   `tests/per_backend_file_ratchet.rs` pins each backend directory to its
   platform modules, so neither grows for a widget.
2. **Input routes through `FrameTree`.** An app returns its widgets from
   `AppLogic::frame_tree`; the runtime hit-tests, focuses, and calls
   `Interactive::handle` with a `RoutedEvent` in widget-local coordinates.
   Persistent per-widget state is `Interactive::State`, kept by id in
   `WidgetStateStore` and handed back at paint as `PaintEnv::widget_state`.
   Visible state the app owns (selection, open, scroll) stays a descriptor
   field the app sets from the widget's events.
3. **Hover and press come from `PaintEnv`** (`env.is_hovered(id)`,
   `env.pressed_sub(id)`, …), not from descriptor fields, hover trackers, or
   a per-widget hit map.
4. **One layout feeds paint, routing and hit-testing.** A host that hit-tests
   outside the runtime resolves the same layout with
   `DrawsPrimitives::layout_primitive`, or reads the frame's painted record
   (`primitive::painted_hit`). It never re-derives geometry.
5. **Apps and examples go through `AppLogic` + `quadraui::{tui,gtk,macos,win}::run`.**
   Render code is backend-generic; one `AppLogic` drives every backend. An
   example with its own event loop means the trait is missing something —
   fix the trait. Examples pair by shape: one `AppLogic` in
   `examples/common/<shape>.rs`, one ~10-line runner per backend.
6. **Events are unified at `UiEvent`** before reaching `AppLogic::handle`; a
   widget's `Action` becomes one through `WidgetAction::into_ui_event`.

**TUI is a first-class backend.** A user-facing capability (menus, pickers,
dialogs, buttons, text boxes, lists) must *work* on TUI, not return
`Unsupported`/`None` — reserve that for things physically absent on a terminal
(tray icon, dock badge, OS global shortcut). **The crate owns the degrade, in
`compose/`, behind the same call** (pattern: `compose::FolderPicker`).
When adding a capability, state its TUI story and ship a `tui_*` test.

**One backend per issue.** A quadraui issue targets exactly one backend (`tui`, `gtk`, `macos` or `win`) and is verified only on that backend's host; the Test stage routes it there. A cross-backend feature is split into a backend-neutral seam issue (trait method, primitive, layout, with a `tui_*`/headless test) plus one issue per backend that implements it. TUI issues are verified with the headless `TuiDriver`, which runs on any host; a real-terminal check on Linux, macOS and Windows is needed only when the issue is about terminal behaviour itself. If your issue spans several backends, do the one it names (or the seam) and list the rest in your final message instead of doing them.

**Clipboard events:** `UiEvent::ClipboardPaste(String)` inserts pasted text into
the focused input; `UiEvent::TextCopied(String)` only confirms a copy happened.
Ctrl-C copy in a new backend/primitive emits `TextCopied`, never `ClipboardPaste`.

## Review gate: no new `Backend` methods, no widget duplication

The adversarial reviewer reads this file. A diff that does any of the following
gets an unconditional `request-changes` — same weight as the sealed-suite rule
below — unless the linked issue's body carries an explicit operator-approved
exemption:

- adds a method to `trait Backend` in `quadraui/src/backend.rs`;
- adds a non-test file under `quadraui/src/{tui,gtk,macos,win}/` that paints,
  lays out or routes a widget, rather than a platform service (clipboard,
  trash, secret store, dialogs, …) or a rasteriser verb;
- adds a widget that does not implement `Primitive` + `A11yPrimitive` +
  `Interactive` (or `non_interactive!`), or a `Surface`/`FrameZone` variant
  for one widget instead of using `Surface::Widget`;
- adds a second implementation of logic that already exists elsewhere —
  navigation, scroll-into-view, hit-testing, focus, hover tracking, or
  painting the same widget twice via two different call paths.

**Reviewers: grep the diff for `fn draw_` / `fn *_layout` inside `impl Backend`
or `trait Backend`, and for new files under the four backend directories.** A
hit is the signal this rule exists to catch — see *The widget contract* and
`quadraui/docs/PRIMITIVE_RULES.md`'s "Adding a widget" checklist for the path a
new widget takes instead.

## Finish the issue — `partial` is not a scope choice

An issue is done when everything in its body is done. Don't do a slice of it and merge that as `ISSUE_RESOLUTION: partial`. That includes "the rest migrate in follow-up PRs" and a hand-written stand-in for something the issue says must be generated.

`partial` is allowed only when:
- the issue spans backends and you did the one it names (or the seam), per *One backend per issue*;
- the rest is blocked on another repo (`BLOCKED_ON_UPSTREAM`); or
- the issue body defines tranches and you finished one whole tranche.

If the whole issue really is too big for one session, don't merge a slice. Say in your final message that it needs splitting, and propose the split.

**Reviewers:** any other `partial`, such as scope the worker cut on its own or a main deliverable swapped for an approximation, is a blocking finding and gets `request-changes`, however good the code that landed is. List what's missing so the fix round can finish it.

## Downstream consumers — READ BEFORE CHANGING ANY `pub` ITEM

quadraui is **published on crates.io** (`quadraui/Cargo.toml` `version`, `0.MINOR.x`);
`release.yml` publishes when a `develop` → `main` promotion carries a version bump.

| Consumer | Declaration | Breaks when |
|---|---|---|
| `vimcode` — `JDonaghy/vimcode` | `quadraui = { version = "0.MINOR", … }` from crates.io | the next patch release it `cargo update`s to — so **a breaking change needs a minor bump; in a patch release it is a semver violation** |
| `coord-tui` — `JDonaghy/coord-tui` | `quadraui = { git = …, rev = "<pinned sha>" }` | its next deliberate rev bump |

`ci.yml`'s `downstream` job `cargo check --all-targets`s both consumers against the
PR (coord-tui only when the `COORD_TUI_TOKEN` secret exists). It proves "compiles",
not "behaves" — you are the gate for the rest. **If you touch that job, keep its
three load-bearing properties** (`working-directory:` not `--manifest-path`,
`--all-targets`, `RUSTFLAGS: ""`); details in `AGENT_REFERENCE.md`.

### Before you change, rename, or remove any `pub` item

1. **Measure the blast radius** and **paste the grep output in the PR body**:
   `grep -rn '<symbol>' ~/src/coord-tui/src ~/src/vimcode/src`. Zero hits and no
   in-tree use ⇒ free to remove. Any hit ⇒ breaking; rules 2–4 apply.
2. **Prefer a non-breaking shape**, in order: a default impl on a trait consumers
   implement (`ShellApp`, `AppLogic`, `Primitive`, `Interactive` — *not*
   `Backend`, which is in-tree only); `#[non_exhaustive]` on public
   structs/enums; a new `Default` field or builder instead of a new required
   argument; a new function alongside the old one instead of a rename.

   **Keep renderer-derived state off the descriptor.** Descriptors
   (`primitives/*`) are built by consumers with full struct literals, so a
   required-field add or a field-type change is *breaking*, and it also breaks
   the in-repo sealed acceptance oracle — which a work leg cannot edit. Breaking
   is allowed, but it costs a same-PR sealed-oracle update (operator /
   test-author, not a work leg) and a minor version bump at the develop→main
   promotion, enforced by `cargo-semver-checks`. So default to additive: state
   the widget derives for itself (hover, press, a drag in progress) lives in
   `Interactive::State` or comes from `PaintEnv`, not in a new descriptor
   field. Descriptor fields are for state the app owns and sets.
3. **If it must break, deprecate first — two PRs.** PR 1 adds the new shape and
   keeps the old one compiling behind `#[deprecated(since = "…", note = "use X
   instead")]` (alias / `From` / forwarding method), migrates every **in-repo**
   call site in the same PR (`deprecated` is denied in-repo by `-D warnings`), and
   opens the consumer migration issue. PR 2 deletes the shim after the migrations
   merge. A `deprecated` warning must never fail a consumer's CI — if it does, fix
   that gate, don't stop deprecating.
4. **One breaking change per PR.** Don't batch unrelated removals.
5. **Declare it.** A PR touching a `pub` item has a `## Downstream impact` section
   naming each consumer file that must move (or "no consumer hits" + the grep), and
   a `changelog.d/<issue>.<kind>.md` fragment (`added` / `deprecated` / `removed` /
   `fixed`). Review sends back a public-API PR missing either.

Mechanics and shim patterns: `quadraui/docs/PRIMITIVE_RULES.md` rule 8.

## Development workflow

- Work on a branch off `develop` (`issue-{number}-{short-description}`); never
  commit code directly to `develop`. PRs target `develop`. Interactive sessions:
  ask the user before pushing or merging.
- Label issues blocked on another repo `blocked` and reference the prereq as
  `<owner>/<repo>#<N>`.

## Quality Gate

**Workers: run the narrowest relevant tests, not the matrix below.** For your diff:
`cargo test -p quadraui --features <feature> <filter>` for the modules/features you
touched, `cargo clippy -p quadraui --features <feature> -- -D warnings` for those
features, and `cargo fmt --all --check`. The full matrix is CI's job (and the
coordinator's Test stage runs the routed command it is given, not this whole
block). Never run a bare root-level `cargo test --features tui`: it drags in
`kubeui-gtk` → GTK → `pkg-config` and fails for reasons unrelated to your diff.

The matrix CI runs (kept in sync with ci.yml by `quadraui-repo-lint`; annotated
copy in `AGENT_REFERENCE.md`):

```bash
cargo build  --features tui --workspace --exclude kubeui-gtk
cargo test   --features tui --workspace --exclude kubeui-gtk
cargo clippy --features tui --workspace --exclude kubeui-gtk -- -D warnings
cargo build  --features gtk,tui --workspace --exclude kubeui
cargo test   --features gtk,tui --workspace --exclude kubeui --no-fail-fast
cargo clippy --features gtk,tui --workspace --exclude kubeui -- -D warnings
cargo check -p quadraui --features win
cargo test  -p quadraui --features win
cargo fmt --all --check
```

**CHANGELOG enforcement (#1500, #2419).** CI rejects PRs that change non-test
`quadraui/src/**` Rust files without adding a new changelog fragment:
`changelog.d/<issue>.<kind>.md`, `<kind>` one of `added` / `changed` /
`deprecated` / `removed` / `fixed`, holding the bullet exactly as it would read
under `[Unreleased]` (see `changelog.d/README.md`). **Never edit `CHANGELOG.md`'s
`[Unreleased]` section directly** — CI rejects that too; `scripts/release.py
prepare` assembles the fragments at release time. Comment-only and test-only
(`#[cfg(test)]` block) diffs are exempt automatically; the `no-changelog` label
skips the check entirely.

## Win-GUI: building and testing for real

The `--features win` lines above are a type-check on non-Windows hosts, not a test
of Windows code. **Win-GUI milestone work runs for real on `dell64`**: build with
`cargo xwin build` and test with `tools/win-test.sh` (playbook + traps in
`quadraui/docs/TESTING.md` and `AGENT_REFERENCE.md`). Rasteriser issues there all
edit `quadraui/src/win/backend.rs` — touch only the `PaintSurface` verbs
(`surface_*`) or platform-service methods your issue names. A widget that paints
wrong on one backend is fixed in its shared painter or in the verb, never with a
per-backend branch.

## Code Style

- `rustfmt` defaults; `PascalCase` types, `snake_case` functions/vars.
- Tests in `#[cfg(test)] mod tests` at file bottom.
- Doc comments on public items; `//!` module headers describe intent + invariants.
- **Comments describe the code as it is, never its history.** No issue numbers
  (`#123`) and no history phrases ("used to", "no longer", "before #", "this PR",
  "the reviewer", "adversarial review") in `//`/`#` comments — history goes in the
  commit message. CI's comment-history ratchet (`tools/comment_history_lint.py`)
  fails a PR that raises any module's count; run
  `python3 tools/comment_history_lint.py` before pushing. If it blocks you, run
  `tools/comment_history_lint.py --fix` first — it mechanically strips the easy
  cases (a leading `#1234 review: ...` tag, a trailing `(#1234)` aside, a
  ref-only line) for free; what's left needs a hand rewrite. Only exception: a
  load-bearing workaround reference gated on that issue closing
  (`CONTRIBUTING.md` "Comment policy").

## Commit conventions

`<type>(<scope>): <imperative summary>` — e.g. `feat(quadraui): add TreeView column
headers`, `test(quadraui): TUI tree paint/click round-trip harness`. Scope is
`quadraui` for the library, `kubeui` / `kubeui-gtk` / `kubeui-core` for demos.

## Demos are mandatory for visual features

- A new primitive, new interaction, or visual behaviour change ships a runnable
  demo: `examples/tui_<name>.rs` (+ `examples/gtk_<name>.rs` if GTK is in scope),
  paired via `examples/common/<shape>.rs` (reference: `tui_pipeline.rs` +
  `gtk_pipeline.rs`). Name it after the feature (`tui_list_hscroll.rs`, not
  `tui_issue276.rs`); it must exercise the changed path visually. Verify with
  `cargo run --example <name> --features tui`.
- **Every TUI example also ships an automated black-box test** — the acceptance
  bar. A new or changed `tui_*` example gets a `TuiDriver` end-to-end test in
  `tests/tui_example_driver.rs`: drive the real `event → handle → render` path on
  the headless backend and assert with `find()` + `screen_contains()` — **never
  hardcode coordinates** (`quadraui::tui::testing::{TuiDriver, driver_with_shell}`).
  A new or changed widget gets `tests/widget_contract_<name>.rs`: paint it as
  `Surface::Widget`, route through `FrameTree`, assert the `UiEvent` the app
  receives. GUI examples have the same harness per backend (`GtkDriver`,
  `MacDriver`, `WinDriver` in `tests/{gtk,macos,win}_example_driver.rs`); use
  the one for the backend your issue names. The adversarial reviewer rejects a
  `tui_*` example or widget change without its driver test. Tier details:
  `quadraui/docs/TESTING.md`.

## Oracle acceptance suite (sealed — do not edit)

`quadraui/tests/acceptance.rs` is the sealed entrypoint the oracle loop drives
(`cd quadraui && RUSTC_BOOTSTRAP=1 cargo test --test acceptance --features tui,gtk
-- -Z unstable-options --format json`). Its sealed block, below the `SEALED` banner,
`include!`s per-milestone slices from repo-root `tests/acceptance/<ms>/`.

**Workers must not create, edit, or delete anything under repo-root
`tests/acceptance/`, and must not touch `quadraui/tests/acceptance.rs` below its
`SEALED` marker.** Those slices are authored at a milestone's Gate A, not by a Work
dispatch.

## Branching + releases

- `main` — released/stable; only updated by `develop` → `main` promotion merges
  (a version bump in one triggers the crates.io publish).
- `develop` — integration branch; all feature work merges here first.

## Reference consumer: vimcode

vimcode (`~/src/vimcode`) is where most primitives, rasterisers and hit-test patterns
were first prototyped. Before building a new runtime feature, read vimcode's
equivalent code and extract the pattern rather than reinventing it — the
feature → file map is in `AGENT_REFERENCE.md`.
