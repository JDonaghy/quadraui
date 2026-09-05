# Regression note: the vendored vt100 `unicode-width` patch was removed (#795)

**This file used to live at `vendor/vt100-0.16.2-patched/README-PATCH.md`
and document a live `[patch.crates-io]` stanza in the root `Cargo.toml`.
Both the vendor directory and the patch stanza are gone as of #795.** This
note stays at the repo root so the history and the "don't re-add this
blindly" checklist survive the deletion.

## Why the patch existed (quadraui#452)

quadraui bumped vt100 0.15 → 0.16 in commit `96bfafe` (#397) to fix a
`Grid::set_size` cursor-clamp panic. That bump pulled in vt100 0.16's own
`unicode-width` requirement of `^0.2.1` — up from `0.1.10` in vt100 0.15.
`^0.2.1` was mathematically incompatible with a consumer graph that
exactly-pinned `unicode-width = "=0.2.0"` — which is what `ratatui 0.29`
did upstream, and what vimcode's `Cargo.toml` pinned directly at the
time. The fix was a vendored copy of vt100 0.16.2 with exactly one
manifest line changed (`unicode-width = "0.2.1"` → `"0.2"`), pulled in via
`[patch.crates-io]` in the workspace root `Cargo.toml`. No `src/**` files
were ever touched.

## Why it was safe to remove (#795, 2026-09-05)

The pin reason expired. quadraui's own `Cargo.toml` now declares
`ratatui = "0.30"`, which carries no exact `unicode-width` pin at all —
`Cargo.lock` resolves `ratatui 0.30.2` and `unicode-width 0.2.2` cleanly
against unpatched crates-io `vt100 0.16.2`, with no conflict to work
around. `cargo package -p quadraui --dry-run` and the full quality gate
both pass with the patch stanza and the vendor directory gone.

This matters beyond tidiness: `[patch.crates-io]` **never propagates to
consumers of a published crate** — it only takes effect for builds that
include this workspace's own `Cargo.toml` in their resolution. Every
downstream consumer that wanted the fix had to replicate the patch by
hand (vimcode did, in its own `Cargo.toml`). A live `[patch.crates-io]`
stanza is also an unconditional hard blocker to `cargo publish` ever
working for this crate. Removing it once the underlying conflict is gone
is required for quadraui to be publishable at all (parent: #783 "v0.1.0 —
Consumable").

A CI step (see `.github/workflows/ci.yml`, the `patch-guard` job) now
fails the build if a `[patch.crates-io]` stanza is ever reintroduced
without updating this note.

## ⚠️ If you're tempted to re-add a `[patch.crates-io]` for vt100/unicode-width

Don't do it silently. First confirm the conflict actually exists again by
reproducing it: a minimal crate depending on nothing but `vt100 = "0.16"`
plus whatever pinned the transitive `unicode-width` version last time
should fail to resolve. Concretely, one of these needs to be true before
a patch is justified again:

1. quadraui (or a dependency it pulls in) now declares an exact pin on
   `unicode-width` or on a crate that exactly-pins it, that conflicts with
   vt100's own manifest bound, or
2. vt100's own upstream manifest raises its `unicode-width` floor to
   something a consumer's pin can no longer satisfy.

If you do need to re-add it: vendor the crate under `vendor/`, change
only the manifest line that needs relaxing, document the "why" the way
this file did the first time, and update or remove the `patch-guard` CI
job rather than deleting it outright — it exists specifically to make a
silent re-addition impossible.

---

The original patch's checklist, kept verbatim below for reference:

> ## ⚠️ Do not remove this patch without checking first
>
> quadraui already tried "just use crates.io directly" once before for vt100
> (commit `96bfafe` removed the #377 vendor shim, believing the "real" 0.16
> bump was cleaner) — and that's exactly what reintroduced this conflict.
> If you're tempted to drop this `[patch.crates-io]` entry and go back to
> crates.io `vt100`, first confirm **one** of:
>
> 1. Upstream vt100 has relaxed its own `unicode-width` bound below `0.2.1`
>    in a newer release, or
> 2. vimcode (or whichever consumer) no longer exactly-pins
>    `unicode-width` via an exact-pinned `ratatui` (or other) dependency.
>
> Otherwise this patch is load-bearing — removing it silently breaks every
> downstream build that combines quadraui's `tui`/`terminal`/`gtk` features
> with an exact `unicode-width = "=0.2.0"` pin anywhere in its own
> dependency graph.

Both of those conditions are moot now: condition 2 became true when
quadraui's own `ratatui` dependency moved to `"0.30"` (no exact
`unicode-width` pin anywhere in the graph any more), which is what made
this removal safe.
