# CLAUDE.md — quadraui

Agent-facing rules for the **quadraui** repo. Re-read on every turn, so it holds
only what a diff can violate. Rationale, history and long playbooks live in
[`docs/AGENT_REFERENCE.md`](docs/AGENT_REFERENCE.md) — read a
section of it only when your task touches that area.

quadraui is **self-contained at design time** (no primitive encodes a consumer's
domain model) but **not at delivery time**: published consumers build against it.
Read *Downstream consumers* before changing any `pub` item.

## Codebase navigation — query the graph first

`graphify-out/` holds a knowledge graph of this repo. For "where is this handled /
what calls this" questions, query it (the `graphify` skill or CLI) before grep/Read.

## Background docs — on demand only

No reading chain is required at session start. The background docs (README,
`quadraui/docs/decisions/DECISIONS.md`, `ARCHITECTURE.md`, `TESTING.md`,
`LESSONS.md`, …) are large; open one only when the task needs it — the index is
in `AGENT_REFERENCE.md`. **Exception:** read `quadraui/docs/PRIMITIVE_RULES.md`
rule 8 before removing or renaming anything `pub`.

## Cross-backend portability commitment

Goal: a future agent can write a whole new backend (Windows, macOS) just by
implementing the `Backend` trait — zero consumer-side changes, zero per-example
rewrites. Non-negotiable:

1. **Every primitive MUST have a `Backend` trait method.** TUI + GTK rasterisers
   with no trait method is a bug — add the trait method.
2. **Apps and examples go through `AppLogic` + `quadraui::{tui,gtk}::run`.** Render
   code is backend-generic; one `AppLogic` impl drives every backend.
3. **Examples are paired by shape, not backend:** one `AppLogic` in
   `examples/common/<shape>.rs`, one ~10-line runner per backend.
4. **Bypassing the runner is a smell** — an example with its own event loop means
   the trait is missing something. Fix the trait, not the example.
5. **Layout helpers go through `Backend` too**; consumer click routers stay
   backend-agnostic.
6. **Events are unified at `UiEvent`** before reaching `AppLogic::handle`.

**TUI is a first-class backend.** A user-facing capability (menus, pickers,
dialogs, buttons, text boxes, lists) must *work* on TUI, not return
`Unsupported`/`None` — reserve that for things physically absent on a terminal
(tray icon, dock badge, OS global shortcut). **The crate owns the degrade, in
`compose/`, behind the same call** (pattern: `compose::FolderPickerController`).
When adding a capability, state its TUI story and ship a `tui_*` test.

**One backend per issue.** A quadraui issue targets exactly one backend (`tui`, `gtk`, `macos` or `win`) and is verified only on that backend's host; the Test stage routes it there. A cross-backend feature is split into a backend-neutral seam issue (trait method, primitive, layout, with a `tui_*`/headless test) plus one issue per backend that implements it. TUI issues are verified with the headless `TuiDriver`, which runs on any host; a real-terminal check on Linux, macOS and Windows is needed only when the issue is about terminal behaviour itself. If your issue spans several backends, do the one it names (or the seam) and list the rest in your final message instead of doing them.

**Clipboard events:** `UiEvent::ClipboardPaste(String)` inserts pasted text into
the focused input; `UiEvent::TextCopied(String)` only confirms a copy happened.
Ctrl-C copy in a new backend/primitive emits `TextCopied`, never `ClipboardPaste`.

## Downstream consumers — READ BEFORE CHANGING ANY `pub` ITEM

quadraui is **published on crates.io** (`quadraui/Cargo.toml` `version`, 0.1.x);
`release.yml` publishes when a `develop` → `main` promotion carries a version bump.

| Consumer | Declaration | Breaks when |
|---|---|---|
| `vimcode` — `JDonaghy/vimcode` | `quadraui = { version = "0.1.x", … }` from crates.io | the next 0.1.x it `cargo update`s to — so **a breaking change in a 0.1.x release is a semver violation** |
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
   implement (`ShellApp`, `AppLogic` — *not* `Backend`, which is in-tree-only and
   deliberately has no defaults); `#[non_exhaustive]` on public structs/enums; a
   new `Default` field or builder instead of a new required argument; a new
   function alongside the old one instead of a rename.
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
   a `CHANGELOG.md` `[Unreleased]` entry (`Added` / `Deprecated` / `Removed` /
   `Fixed`). Review sends back a public-API PR missing either.

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

## Win-GUI: building and testing for real

The `--features win` lines above are a type-check on non-Windows hosts, not a test
of Windows code. **Win-GUI milestone work runs for real on `dell64`**: build with
`cargo xwin build` and test with `tools/win-test.sh` (playbook + traps in
`quadraui/docs/TESTING.md` and `AGENT_REFERENCE.md`). Rasteriser issues there all
edit `quadraui/src/win/backend.rs` — touch only the `draw_*` methods your issue
names.

## Code Style

- `rustfmt` defaults; `PascalCase` types, `snake_case` functions/vars.
- Tests in `#[cfg(test)] mod tests` at file bottom.
- Doc comments on public items; `//!` module headers describe intent + invariants.
- **Comments describe the code as it is, never its history.** No issue numbers
  (`#123`) and no history phrases ("used to", "no longer", "before #", "this PR",
  "the reviewer", "adversarial review") in `//`/`#` comments — history goes in the
  commit message. CI's comment-history ratchet (`tools/comment_history_lint.py`)
  fails a PR that raises any module's count; run
  `python3 tools/comment_history_lint.py` before pushing. Only exception: a
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
  Primitive paint/click changes stay covered by the round-trip harness. GTK-example
  coverage waits on `GtkDriver` — **TUI only for now**. The adversarial reviewer
  rejects a `tui_*` example change without its driver test. Tier details:
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
