# Changelog

All notable changes to `quadraui` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to follow [Semantic Versioning](https://semver.org/)
once it has a first tagged release — see the *Versioning* section below for
what that means in practice pre-1.0.

`quadraui` is a single-crate workspace by publication surface: `kubeui*` are
in-tree demo apps, never published, and are not covered by this file.
Everything here is about the `quadraui` crate itself (`quadraui/Cargo.toml`).

## Scope: this file starts now, not retroactively

quadraui has 1000+ commits of history predating this file (see `git log`);
none of it is backfilled here. `[Unreleased]` below is where changelog
discipline begins — every PR from this point forward that changes `quadraui`'s
public API, in the sense `quadraui/docs/PRIMITIVE_RULES.md` rule 8 defines,
adds an entry here in the same PR. A public-API PR that skips this file is
incomplete for the same reason rule 8 already treats a missing
`## Downstream impact` section as incomplete — this file's `Deprecated` /
`Removed` sections are rule 8's two-PR protocol made visible outside a diff:

- **PR 1 of a rule-8 change** (new shape added, old one kept behind
  `#[deprecated]`) adds an entry under `### Deprecated` naming the old item,
  its replacement, and the tracking issue for PR 2.
- **PR 2** (shim deleted) moves that same line to `### Removed` and closes the
  loop — same wording, one section down, so the deprecation's full lifecycle
  reads as two adjacent lines rather than being lost across two disconnected
  PRs.
- A non-breaking addition (new primitive, new optional field, new `Backend`
  method with no default per rule 7) goes under `### Added`.
- A behavior fix with no API shape change goes under `### Fixed`.

See `quadraui/docs/PRIMITIVE_RULES.md` rule 8 for the full lifecycle policy
(what counts as breaking, the deprecation-shim patterns, the two-PR split)
and `CLAUDE.md`'s *Downstream consumers* section for why any of this matters
to `coord-tui` and `vimcode`.

## Versioning

Pre-1.0, per Cargo's own semver convention: a `0.MINOR.PATCH` bump treats
`MINOR` the way `1.0`+ treats `MAJOR` (breaking), and `PATCH` the way `1.0`+
treats `MINOR`/`PATCH` combined (additive or fix). A rule-8 breaking change
bumps `MINOR`; everything else bumps `PATCH`.

Releases are tagged `vX.Y.Z` against `main`. Tagging is a coordinator release
action (see `quadraui#797`), not something an individual PR does — a PR adds
its entry under `[Unreleased]`, and the coordinator retitles that section to
`[X.Y.Z] - YYYY-MM-DD` (adding a fresh empty `[Unreleased]` above it) at
release time.

## [Unreleased]

### Added

- `rust-version` in `quadraui/Cargo.toml`, pinned to match
  `rust-toolchain.toml` (1.97.1) — an older `cargo` now reports the version
  requirement directly instead of an opaque parse/feature error.
- `[package.metadata.docs.rs]` in `quadraui/Cargo.toml`, building every
  optional backend feature (`tui`, `gtk`, `win`, `terminal`, `macos`) across
  the Linux/macOS/Windows targets, plus `#[cfg_attr(docsrs, doc(cfg(...)))]`
  annotations on each feature-gated public module — a published crate now
  renders all four backends on docs.rs instead of only the always-on core.
- CI gates: `cargo package -p quadraui --locked` (full verification build,
  no vendored patches, proves the manifest + lockfile resolve and build
  standalone off crates.io) and `cargo doc --no-deps -D warnings` (the doc
  build itself is held to the same warnings-as-errors bar as the rest of
  CI; see that job's comment in `.github/workflows/ci.yml` for the narrow,
  named exception carved out for pre-existing intra-doc-link debt).
- This file.
- `quadraui::prelude` — a curated subset of the crate's ~500 flat
  crate-root exports (the two runner traits, event/geometry types, and a
  representative sample of primitives) for a first `AppLogic` app.
  `quadraui/docs/GUIDE.md` (new) walks through building a two-pane app
  with it, including a "which runner do I want?" (`AppLogic` vs
  `ShellApp`) comparison.
- `quadraui::layout` — shared foundation for the `layout()`/`hit_test()`
  convergence (issue #816): `Anchor` + `Side`/`ResolvedSide` (the
  preferred-side/overflow-flip/pin-to-edge resolution `Tooltip`,
  `Completions`, `ContextMenu` and `RichTextPopup` each reimplement via
  their own placement enum today) and `visible_range_walk` +
  `VisibleItem` (the scroll-offset visible-range walk all 19
  `Visible*{idx, bounds}` structs hand-roll today). Purely additive —
  no existing primitive consumes it yet; each primitive converges onto
  it in its own PR behind a `#[deprecated]` shim per
  `quadraui/docs/PRIMITIVE_RULES.md` rule 8.
- `quadraui::testing::RecordingBackend` — a public, fully-implemented
  `Backend` for unit tests that need *some* backend to hand a
  controller (e.g. `TreeController`, `ChatController`), not a specific
  backend's real rendering. Replaces three private, near-identical
  `MockBackend` copies previously duplicated across
  `compose::{tree_controller, chat_controller, app_shell}`'s own test
  modules.
- `quadraui/examples/hello.rs` — a standalone, under-60-line "hello
  world" example with no `examples/common/` dependency (`cargo run
  --example hello --features tui`), plus its `TuiDriver` end-to-end
  test in `quadraui/tests/tui_example_driver.rs`.
- `CONTRIBUTING.md` — human-oriented contributor guide (project layout,
  local setup, the quality-gate commands, PR expectations).

### Changed

- `publish = false` removed from `quadraui/Cargo.toml` — `quadraui` is now
  publishable to crates.io. (The actual `v0.1.0` tag and `cargo publish` are
  a separate, coordinator-run release step — see `quadraui#797`.)
- `quadraui/docs/DECISIONS.md` and `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md`
  moved to `quadraui/docs/decisions/` — archived design-history documents,
  separated from the "read on demand" reference docs `CLAUDE.md` points
  contributors at (`ARCHITECTURE.md`, `PRIMITIVE_RULES.md`,
  `CONSUMER_PATTERNS.md`, `TESTING.md`, `LESSONS.md`). All internal links
  updated; no content changed.
