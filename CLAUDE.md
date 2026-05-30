# CLAUDE.md — quadraui

Agent-facing guide for working in the **quadraui** repo. This file
stays slim — reference docs live in `quadraui/docs/` and are read on
demand.

This repo is **self-contained**: no consumer depends on quadraui from
inside the repo at compile time except the demo apps (`kubeui*`).
Vimcode and any other downstream consumer pin a published version
externally. Don't introduce assumptions about specific downstream
consumers.

## Session Start Protocol

1. Read `README.md` for the high-level shape (workspace, primitives, status).
2. Read `quadraui/docs/DECISIONS.md` for primitive-distinctness principles.
3. Read `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §4 (Backend trait shape) and §9 (resolved decisions log).
4. Read the *Cross-backend portability commitment* below.
5. Run `gh issue list --state open` to see active work.

**Read on demand** (when the task requires it):

- `quadraui/docs/ARCHITECTURE.md` — workspace layout, two-layer split, compose helpers, GTK hosting helpers, backend trait.
- `quadraui/docs/PRIMITIVE_RULES.md` — the 7 rules for adding/changing primitives + maturity levels. **Read when touching primitives.**
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
