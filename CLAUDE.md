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

## Branching + releases

- `main` — released/stable. Only updated by release merges from `develop`.
- `develop` — integration branch. All feature work merges here first.
