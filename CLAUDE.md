# CLAUDE.md — quadraui

Agent-facing guide for working in the **quadraui** repo. This file
stays slim — reference docs live in `quadraui/docs/` and are read on
demand.

This repo is **self-contained at design time**: no consumer depends on
quadraui from inside the repo at compile time except the demo apps
(`kubeui*`), and no primitive should encode a specific consumer's domain
model. **It is not self-contained at delivery time** — two external
consumers build against this repo's `develop` tip with no version pin.
Read *Downstream consumers* below before changing any `pub` item.

## Codebase navigation — query the graph first

This repo ships a **graphify** knowledge graph in `graphify-out/` (`graph.json`,
`GRAPH_REPORT.md`), kept current automatically by `post-commit` / `post-checkout`
git hooks. For any architecture / "where is this handled" / "what calls this" /
file-relationship question, **query the graph first** (the `graphify` skill, or the
graphify CLI) before reaching for grep/Read. Grep/Read are for exact-string or
line-level confirmation — not the first move.

## Session Start Protocol

1. Read `README.md` for the high-level shape (workspace, primitives, status).
2. Read `quadraui/docs/DECISIONS.md` for primitive-distinctness principles.
3. Read `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §4 (Backend trait shape) and §9 (resolved decisions log).
4. Read the *Cross-backend portability commitment* below.
5. Run `gh issue list --state open` to see active work.

**Read on demand** (when the task requires it):

- `quadraui/docs/ARCHITECTURE.md` — workspace layout, two-layer split, compose helpers, GTK hosting helpers, backend trait.
- `quadraui/docs/PRIMITIVE_RULES.md` — the 8 rules for adding/changing primitives + maturity levels. **Rule 8 (public-API lifecycle) is mandatory reading before removing or renaming anything `pub`.**
- `quadraui/docs/CONSUMER_PATTERNS.md` — MSV debug-sidebar and SC panel recipes. **Read when working on consumer integrations.**
- `quadraui/docs/TESTING.md` — coverage taxonomy, backend testability requirement, quality gate commands. **Read when writing tests.**
- `quadraui/docs/LESSONS.md` — durable rules from real failures + "What NOT to do." **Read at session start; apply as you work.**

## Cross-backend portability commitment

**The goal: a future agent should be able to write the entire Windows or macOS backend with almost no input — just by implementing the `Backend` trait against Direct2D / Core Graphics. Zero consumer-side changes. Zero per-example rewrites.**

This is non-negotiable. Every architectural decision in this repo serves it.

1. **Every primitive MUST have a `Backend` trait method.** If a primitive has TUI and GTK rasterisers but no trait method, that's a bug — file an issue and add the trait method.
2. **Apps and examples MUST go through `AppLogic` + `quadraui::{tui,gtk}::run`.** Render code is fully backend-generic. The same `AppLogic` impl drives every backend.
3. **Examples are paired by shape, not by backend.** One `AppLogic` impl in `examples/common/<shape>.rs`, one ~10-line runner per backend.
4. **Bypassing the runner is a smell.** If an example writes its own event loop, the `Backend` trait is missing the primitive. Fix the trait, not the example.
5. **Layout helpers go through `Backend` too.** Each backend supplies native metrics internally. Consumer click routers stay backend-agnostic.
6. **Events are unified at the `UiEvent` boundary.** Every backend translates native events into `quadraui::UiEvent` before reaching `AppLogic::handle`.

If you're tempted to take a shortcut — bypass the runner, copy-paste an example across backends, build a per-backend layout helper — **stop and ask: does this violate the portability commitment?** If yes, fix the trait gap first.

## Downstream consumers — READ BEFORE CHANGING ANY `pub` ITEM

quadraui is `publish = false`, `version = "0.0.1"`. **Nothing anywhere pins a published version of this crate.** Two external consumers depend on it by *relative path to a sibling checkout*, and both their CI jobs clone `develop`:

| Consumer | Declaration | Its CI |
|---|---|---|
| `coord-tui` — `JDonaghy/claude-coordinator`, `tui/` | `quadraui = { path = "../../quadraui/quadraui", features = ["tui","terminal"] }` | `cargo-test.yml` clones this repo's **default branch** — which is `develop` |
| `vimcode` — `JDonaghy/vimcode` | `quadraui = { path = "../quadraui/quadraui", … }` **plus** `vt100 = { path = "../quadraui/vendor/vt100-0.16.2-patched" }` | `ci.yml` clones `--branch develop` |

Consequences, all of which have already bitten:

- A breaking change is **live in both repos the instant it merges to `develop`**. Not at their next release — at their next `cargo build`.
- It turns **every open PR in both repos red**, including PRs that touch nothing related, retroactively.
- There is **no version bump to blame and no downstream compile gate in this repo's CI**. `ci.yml` builds quadraui and its own examples only, so quadraui being green proves nothing about consumers. You are the gate.

This is not hypothetical. #476 ("de-coord board.rs") replaced `Stage` with `CardBadge`, renamed `BadgeStatus::RequestChanges` → `Warning`, deleted two `BoardCard` fields and `BoardAction`'s domain verbs — all correct as *design*. Both consumers broke on 2026-08-05 and coord-tui needed a migration PR (`claude-coordinator#1864`). The change shipped believing it was safe partly because this file used to claim consumers "pin a published version externally." They never did.

**The design rule and the delivery rule point in opposite directions, and both hold.** Keep one consumer's vocabulary *out* of the primitives (that is what #476 was fixing, and it was right). Keep both consumers' *compile status* in mind while landing it.

### Before you change, rename, or remove any `pub` item

1. **Measure the blast radius.** Both consumers are checked out beside this repo:
   ```bash
   grep -rn '<symbol>' ~/src/claude-coordinator/tui/src ~/src/vimcode/src
   ```
   Zero hits in both, and no in-tree use ⇒ free to remove; **paste the grep output in the PR body** rather than asserting it. Any hit ⇒ this is a breaking change, and rules 2–4 apply.
2. **Prefer a shape that isn't breaking at all.** In order:
   - a **default impl on any trait a consumer implements** — today that is `ShellApp` and `AppLogic`, the only quadraui traits `coord-tui` and `vimcode` implement. (`Backend` is in-tree-only: rule 7's deliberate no-default compile error is a to-do list for our own backends and costs consumers nothing. Don't "fix" it with defaults.)
   - `#[non_exhaustive]` on public structs and enums, so later fields and variants are additive;
   - a new field carrying `Default`, or a builder, instead of a new required constructor argument;
   - a new function *alongside* the old one instead of a rename.
3. **If it must break, deprecate first — that is two PRs, not one.**
   - **PR 1** adds the new shape **and keeps the old one compiling** behind `#[deprecated(since = "…", note = "use X instead")]`: a `pub use Old as New` alias, a `From` impl, a forwarding method. Consumers keep building, with a warning that names their fix. Open the consumer migration issue in the same session and link it here.
   - **PR 2** deletes the shim, *after* those migrations merge. Reference them.

   A rename under this rule costs one `pub use` + one attribute. That is the entire difference between a compiler warning and two repos' CI going red.
4. **Don't batch unrelated removals.** #476 removed a type, renamed a variant, deleted two fields and gutted a keymap in one commit, so the consumer migration was all-or-nothing with no partially-compiling state to bisect from. One breaking change per PR.
5. **Declare it.** Any PR touching a `pub` item gets a `## Downstream impact` section naming each consumer file that must move (or stating "no consumer hits", with the grep). **A public-API PR without that section should be sent back at review.**

Mechanics, worked examples, and the deprecation-shim patterns live in `quadraui/docs/PRIMITIVE_RULES.md` rule 8.

## Development Workflow

All non-trivial work should be tracked via GitHub Issues.

**Documentation-only changes** (pure `.md` edits) may be committed directly to `develop` and pushed. No branch, no smoke test.

**For all other changes:**

1. **Always work on a local branch off `develop`.** Never commit code directly to `develop`. Branch naming: `issue-{number}-{short-description}` or `{kind}-{short-description}`.
2. **Run the full quality gate before each commit** (see `quadraui/docs/TESTING.md`).
3. **Do NOT push the branch yet.** Keep it local until smoke tests pass or the user agrees they're not needed. For primitive paint/click changes, the round-trip harness IS the smoke test.
4. **Once approved, ask the user which landing path:**
   - **Path A — merge locally + push.** Small/trivial changes. Fast-forward merge into `develop`, push, delete branch.
   - **Path B — push branch + open PR.** Normal feature work, anything closing an issue. `gh pr create --base develop`.
5. **When a merge closes an issue**, immediately `gh issue close <number> -c "Implemented in PR #N"`.

**When in doubt, default to Path B.** Primitive changes, new rasterisers, harness additions, and public API changes all warrant Path B.

**Creating issues:** at session end, create issues for planned but unstarted work. Include full design context — file paths, primitive shape, expected behavior, harness requirements. Issues should be self-contained.

**Cross-repo prereq tracking:** label blocked issues `blocked` and reference the prereq as `<owner>/<repo>#<N>`.

## Quality Gate

```bash
cargo build --features tui --features gtk
cargo test --features tui
cargo test --features gtk
cargo clippy --features tui -- -D warnings
cargo clippy --features gtk -- -D warnings
cargo fmt --check
```

## Code Style

- `rustfmt` defaults (4-space indent).
- `PascalCase` types, `snake_case` functions/vars.
- Tests in `#[cfg(test)] mod tests` at file bottom.
- Doc comments on public types/functions; `//!` module headers describe intent + invariants.

## Commit conventions

`<type>(<scope>): <imperative summary>`. Examples:

- `feat(quadraui): add TreeView column headers`
- `fix(quadraui): MSV scrollbar bounds clip body width correctly`
- `test(quadraui): TUI tree paint/click round-trip harness`
- `refactor(quadraui): extract tui_tree_layout helper`

Scope is `quadraui` for library changes, `kubeui` / `kubeui-gtk` / `kubeui-core` for demo changes.

## Demos are mandatory for visual features

**Any new primitive, new interaction, or visual behaviour change must ship with a runnable demo.**

- New primitive → new `examples/tui_<name>.rs` (and `examples/gtk_<name>.rs` if GTK is in scope)
- New interaction on an existing primitive → extend the relevant existing example or add a new one
- The demo must exercise the changed code path visually — not just compile
- Name demos after the feature: `tui_list_hscroll.rs`, not `tui_issue276.rs`
- Verify with `cargo run --example <name> --features tui` (or `--features gtk`) before declaring done

Examples follow a paired pattern: one `AppLogic` impl in `examples/common/<shape>.rs`, one ~10-line runner per backend. See `examples/tui_pipeline.rs` + `examples/gtk_pipeline.rs` as reference.

**Every TUI example also ships an automated black-box test** (the acceptance bar — #304). A runnable demo proves it compiles + paints; a driver test proves it *behaves* and catches regressions with no human re-running it. A new or changed `tui_*` example → a **`TuiDriver` end-to-end test** in `tests/tui_example_driver.rs` (Tier-1, #300): build the example's `AppLogic` / `ShellApp`, drive the real `event → handle → render` path against the headless `TestBackend`, and assert with `find()` + `screen_contains()` — **never hardcode coordinates**. (`quadraui::tui::testing::{TuiDriver, driver_with_shell}`.) Primitive paint/click changes stay covered by the round-trip harness (Tier-2, above); GTK-example coverage waits on `GtkDriver` (#301) — **TUI only for now**. The tier-by-tier *how* lives in [`quadraui/docs/TESTING.md`](quadraui/docs/TESTING.md); this is the *bar*. The **adversarial reviewer enforces it** — a PR that adds or changes a `tui_*` example without its driver test should be rejected.

## Event model: TextCopied vs ClipboardPaste

Two clipboard events that must not be conflated:

| Event | Meaning |
|---|---|
| `UiEvent::ClipboardPaste(String)` | User pasted text into an input (bracketed paste). Route to focused text field. |
| `UiEvent::TextCopied(String)` | Broadcast after text was copied to clipboard. Used for copy-confirmation UI. |

`ClipboardPaste` inserts text. `TextCopied` confirms a copy happened. When implementing Ctrl-C copy in a new backend or primitive, emit `TextCopied` — not `ClipboardPaste`.

## Branching + releases

- `main` — released/stable. Only updated by release merges from `develop`.
- `develop` — integration branch. All feature work merges here first.

## Reference consumer: vimcode (`~/src/vimcode`)

**vimcode is quadraui's primary consumer and R&D lab.** Every primitive, rasteriser, hit_test pattern, and compose helper in quadraui was first prototyped as per-backend code in vimcode, then extracted. When building new quadraui features — especially the runtime epics (#202 GTK, #203 TUI, #204 macOS) — **read vimcode's existing implementation first:**

| quadraui feature | vimcode reference code |
|-----------------|----------------------|
| `Backend::draw_frame()` (#199) | `src/gtk/draw.rs::draw_editor()` — 3874-line orchestration function that calls each `draw_*` in z-order. This is the spec for what `draw_frame` must do. |
| `FrameHitMap` / unified click dispatch (#197, #198) | `src/gtk/click.rs::pixel_to_click_target()` — zone detection pipeline using `screen_zone_hit_test` + `window_zone_hit_test`. Shows every click zone the hit map must cover. |
| GTK widget tree (#202 Stage 1) | `src/gtk/mod.rs::fn init()` (~2122 lines) — creates every GTK widget, event controller, and draw closure. This is the mechanical boilerplate `AppShell` must generate. |
| Event wiring (#202 Stage 2) | `src/gtk/mod.rs::fn init()` event controller blocks + `enum Msg` (~333 variants) + `fn update()` (~736-line dispatch). Shows every GDK event type that must be translated. |
| TUI event loop (#203) | `src/tui_main/mod.rs` — crossterm poll loop, `handle_mouse()` dispatch, `draw_frame()` calls. Same structure the TUI runtime must own. |
| Cached layout hit-test pattern | `CompletionsLayout::hit_test()`, `ContextMenuLayout::hit_test()`, `BottomPanelGeometry` + `resolve_bottom_panel_zone()` — all proven in vimcode Sessions 379. Cache at paint, hit-test at click. |
| SidebarSystem GTK rasteriser (#200) | `src/gtk/draw.rs::draw_source_control_panel()` (405 lines) — bespoke Cairo rendering that should delegate to quadraui. TUI already delegates via `SidebarSystem`. |
| Per-panel handlers (#202 Stage 5) | `src/gtk/mod.rs::handle_*_msg()` functions (~1500 lines total) — explorer, SC, extensions, debug, settings, terminal, AI, dialog. Shows what engine methods the runtime must call. |

**How to use this:** Before implementing a quadraui runtime feature, `cd ~/src/vimcode` and read the corresponding backend code. The vimcode implementation is the working prototype — extract the pattern, don't reinvent it. The goal is that vimcode's `src/gtk/` shrinks from 16K lines to ~60 lines as each stage lands.
