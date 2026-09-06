# Contributing to quadraui

Thanks for taking a look at quadraui. This doc is for a human sitting down
to write code — it assumes you can read Rust and use `git`, and it skips
the "why does this exist" design history (that lives in
`quadraui/docs/decisions/DECISIONS.md` if you're curious). If you want the fast
path to a running example, start with [`quadraui/docs/GUIDE.md`](quadraui/docs/GUIDE.md)
instead — this file is about *contributing changes*, not *using the crate*.

## What's in this repo

A Cargo workspace with four members:

- **`quadraui`** — the library. Declarative UI primitives (`TreeView`,
  `StatusBar`, `Split`, 40 in total) plus rasterisers for four backends
  (TUI/ratatui, GTK4, macOS, Windows). This is almost certainly the
  crate you're changing.
- **`kubeui-core`**, **`kubeui`**, **`kubeui-gtk`** — a small demo app
  (a Kubernetes resource browser) that exercises quadraui as a real
  consumer would. `kubeui` is the TUI binary, `kubeui-gtk` the GTK one,
  `kubeui-core` the shared backend-agnostic logic.

Two other consumers exist **outside** this repo and build against
`quadraui`'s `develop` branch directly (no crates.io, no version pin on
one of them) — see `CLAUDE.md`'s *Downstream consumers* section before
you change, rename, or remove anything `pub`. That section is required
reading if your change touches the public API; skip it otherwise.

## Getting set up

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui
cargo build --features tui --workspace --exclude kubeui-gtk
```

That builds everything except the GTK demo, which needs `pkg-config` +
GTK4 dev headers (`libgtk-4-dev` on Debian/Ubuntu). If you don't have
those installed, skip GTK-feature work locally and let CI's `gtk` job
cover it — see *Running the quality gate* below for the exact commands
either way.

Run the smallest example to confirm your toolchain works:

```sh
cargo run --example hello --features tui
```

Press any key, then `q` to quit. If that runs, you're set up correctly.

## Finding your way around

- **`quadraui/src/primitives/`** — one module per primitive
  (`tree.rs`, `status_bar.rs`, …). Pure data + layout math, no rendering.
- **`quadraui/src/{tui,gtk,macos,win}/`** — per-backend rasterisers
  (`draw_*` functions) and each backend's `impl Backend`.
- **`quadraui/src/compose/`** — higher-level controllers built from
  primitives (`TreeController`, `ChatController`, `AppShell`) that
  apps use directly instead of hand-rolling event routing.
- **`quadraui/examples/`** — runnable demos. `hello.rs` is the minimal
  one; `examples/common/<shape>.rs` holds the `AppLogic` bodies shared
  between each primitive's TUI/GTK example pair.
- **`quadraui/tests/`** — cross-cutting tests (conformance matrix,
  driver-based example tests, README/doc truth checks). Primitive-local
  tests live in a `#[cfg(test)] mod tests` at the bottom of their own
  file instead.

If you're not sure where something is handled, this repo ships a
`graphify` knowledge graph (`graphify-out/`) that agents query for
"where is X" questions — a human can just grep, but if you're curious
`GRAPH_REPORT.md` is readable too.

## Making a change

1. **Branch off `develop`**, not `main`. `main` only moves via release
   merges. Name the branch `issue-{number}-{short-description}` if
   there's a tracking issue, or `{kind}-{short-description}` otherwise.
2. **Check `quadraui/docs/PRIMITIVE_RULES.md`** before adding or
   changing a primitive — it has 8 rules covering shape, testability,
   and the public-API lifecycle (rule 8 is mandatory if you're removing
   or renaming a `pub` item).
3. **Ship a demo with any visual change.** A new primitive, a new
   interaction, or a behaviour change on an existing one needs a
   runnable example (`examples/tui_<name>.rs`, `examples/gtk_<name>.rs`
   if GTK is in scope) that exercises the change visually — not just
   compiles.
4. **Ship a test with the demo.** Every TUI example needs a `TuiDriver`
   end-to-end test in `quadraui/tests/tui_example_driver.rs`: build the
   example's `AppLogic`, drive real events through it, assert on the
   rendered screen with `find()`/`screen_contains()` — never a
   hardcoded coordinate. See `quadraui/docs/TESTING.md` for the full
   coverage taxonomy (there's also a primitive-level paint↔click
   round-trip tier for lower-level changes).
5. **Run the quality gate** (below) before you open a PR.

## Running the quality gate

These are the exact commands CI runs (`.github/workflows/ci.yml`) — copy
them verbatim, the package-selection flags matter:

```sh
# tui leg — excludes kubeui-gtk (unconditional gtk4 dep, needs GTK4 dev
# headers). This is what lets the tui leg run with no GTK toolchain.
cargo build  --features tui --workspace --exclude kubeui-gtk
cargo test   --features tui --workspace --exclude kubeui-gtk
cargo clippy --features tui --workspace --exclude kubeui-gtk -- -D warnings

# gtk leg — excludes kubeui instead. Needs pkg-config + libgtk-4-dev;
# skip locally if you don't have them and let CI's gtk job cover it.
cargo build  --features gtk,tui --workspace --exclude kubeui
cargo test   --features gtk,tui --workspace --exclude kubeui --no-fail-fast
cargo clippy --features gtk,tui --workspace --exclude kubeui -- -D warnings

# win leg — no GTK, no Windows host required; a type-check of the
# `cfg(target_os = "windows")` arms, not a real test of them.
cargo check -p quadraui --features win
cargo test  -p quadraui --features win

cargo fmt --all --check
```

**Don't run a bare `cargo test --features tui` at the workspace root** —
it pulls in `kubeui-gtk` and its GTK dependency chain, so on a machine
without GTK4 dev headers it fails in a build script before compiling any
quadraui code. Use the `--exclude` forms above.

Compiler warnings count as failures. If clippy or the build emits one,
fix it before you open the PR — don't leave it for review.

## Commit messages

`<type>(<scope>): <imperative summary>`, e.g.:

- `feat(quadraui): add TreeView column headers`
- `fix(quadraui): MSV scrollbar bounds clip body width correctly`
- `test(quadraui): TUI tree paint/click round-trip harness`
- `refactor(quadraui): extract tui_tree_layout helper`

Scope is `quadraui` for library changes, `kubeui` / `kubeui-gtk` /
`kubeui-core` for demo changes.

## Code style

- `rustfmt` defaults (4-space indent) — `cargo fmt` before committing.
- `PascalCase` types, `snake_case` functions/variables.
- Tests live in `#[cfg(test)] mod tests` at the bottom of the file they
  test.
- Doc comments (`///`) on every public type and function; `//!` module
  headers describe the module's intent and invariants, not just what's
  in it.

## Opening a pull request

- Target `develop`, not `main`.
- If your PR touches a `pub` item in `quadraui`, add a `## Downstream
  impact` section naming every consumer file that needs to move (or
  state "no consumer hits" with the grep output that proves it — see
  `CLAUDE.md`'s *Downstream consumers* section for the exact command).
  A public-API PR without that section gets sent back at review.
- If you're removing or renaming something `pub`, it's a two-PR
  deprecation cycle, not one — `CLAUDE.md` rule 3 has the full shape
  and why a single breaking PR isn't acceptable here.
- CI runs the same quality-gate commands above, plus a `downstream` job
  that compiles the two external consumers against your branch. If that
  job fails, your change breaks a real, unpinned consumer — not a
  theoretical one.

## Getting help

Open an issue, or start a discussion if you're not sure a change is
wanted yet before you write it. For anything touching the public API,
reading `CLAUDE.md`'s *Downstream consumers* section first will save you
a review round-trip.
