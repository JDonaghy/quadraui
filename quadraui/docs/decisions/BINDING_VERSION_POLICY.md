# Binding version policy

**Status:** Decided. 2026-11-11.

**Scope:** How `quadraui-py`'s, `quadraui-node`'s, and
`quadraui-binding-core`'s own version numbers relate to the `quadraui` Rust
crate's version (`quadraui/Cargo.toml`), and how that relationship is
enforced in CI.

---

## Question

Each binding leaf versions independently of `quadraui` — `quadraui-py`'s own
`pyproject.toml` comment already says so, and `quadraui-node`'s
`package.json` has never moved off its initial `0.1.0`. Left fully
unconstrained, that independence can drift arbitrarily far: a binding could
sit on a crate minor from months ago while the crate itself has moved
through several breaking (pre-1.0) minors, with nothing noticing until a
consumer hits a method the binding's pinned `quadraui` dependency doesn't
have yet.

What's the right amount of coupling, and how is it checked?

## Decision

**Binding minor = crate minor. Binding patch is independent.**

For every binding manifest below, the `X.Y` (major.minor) prefix of its own
version must equal `quadraui/Cargo.toml`'s `X.Y` prefix. The trailing `.Z`
(patch) component is free to move on its own in either direction — a
binding-only bug fix bumps its own patch with no crate release required, and
a crate patch release does not force a binding release:

| Manifest | Version field |
|---|---|
| `quadraui-py/pyproject.toml` | `[project] version` |
| `quadraui-py/Cargo.toml` | `[package] version` (kept identical to the `pyproject.toml` value — see that file's own comment) |
| `quadraui-node/package.json` | `"version"` |
| `quadraui-binding-core/Cargo.toml` | `[package] version` |

`quadraui-binding-core` is included even though it is `publish = false` and
has no independent release artifact of its own (see
`BINDING_CORE_PUBLISH_POLICY.md`): it is the shared engine every binding
leaf's own minor tracks, so its own version number should tell the same
story at a glance, and a future extraction (per that policy's two options)
inherits a version that already means something instead of starting from
`0.1.0`.

`quadraui-node/package.json` carries `"private": true` today — `quadraui`
is not yet published to npm (#2239) — but the minor-tracking rule applies to
it unconditionally rather than only once it loses that flag, so there is no
backlog of catch-up bumps waiting for publish day.

Enforced by `scripts/release_decide.py check-binding-versions`, wired into
`ci.yml`'s `changelog-version-check` job (the `develop` -> `main` promotion
gate) alongside that job's existing `version`/`notes` steps: a crate minor
bump that lands without a matching binding bump fails the promotion PR
before `main` moves, the same "catch it before, not after" shape
`check-lock`/`changelog-version-check` already use for the crate's own
version/lockfile/changelog consistency.

## Why this shape

- **A crate minor bump is exactly the event worth coupling to.** Pre-1.0,
  a `quadraui` minor can be breaking (see `CLAUDE.md`'s *Branching +
  releases*); a binding whose declared version doesn't move with it is the
  one most likely to silently depend on a shape that just changed
  underneath it. Patch bumps are explicitly non-breaking by convention, so
  there is nothing to couple a binding's patch to.
- **Patch independence keeps binding-only fixes cheap.** A `quadraui-py`-only
  bug fix (a wrong clipboard decode, a stub type error) doesn't need to wait
  for or trigger a `quadraui` crate release, and vice versa — exactly the
  cadence `quadraui-py/pyproject.toml`'s own existing comment already
  describes for the crate-independence half; this decision adds the minor
  floor that comment didn't have.
- **A CI check, not a convention, because the drift is invisible otherwise.**
  Nothing fails today if `quadraui-node/package.json` sits on `0.1.0` forever
  while the crate climbs through a dozen minors — the mismatch has no other
  observable effect until a consumer actually hits it. A cheap, hermetic,
  checkout-only script step (no cargo, no npm install — see
  `check-binding-versions`'s own module-doc entry in `release_decide.py`)
  catches it at the one moment it's cheapest to fix: before the promotion PR
  that caused the drift merges.

## Revisit when

- A binding leaf is extracted into its own repo (see
  `BINDING_CORE_PUBLISH_POLICY.md`) and needs its own release cadence
  decoupled from `develop` -> `main` promotions; the check would need to move
  with it or be re-scoped to a tag-triggered workflow instead of the
  promotion-PR gate.
- A binding adds a fifth manifest (e.g. a `quadraui-ffi` version field, if
  that leaf ever gains one) — add it to `scripts/release_decide.py`'s
  `BINDING_MANIFESTS` table and this file's table in the same PR.
